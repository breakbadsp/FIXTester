use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use fix_codec::{Decoder, UtcMillis, tag};
use fix_dict::Version;
use fix_session::{Role, SequenceNumbers};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

use crate::model::{FIELDS_MAX, OrderValues, Profile, now};
use crate::{Result, invalid};

const RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const PAGE_ROWS: u32 = 200;
const PROFILES_MAX: i64 = 128;
const SCHEMA: &str = include_str!("schema.sql");

#[derive(Debug)]
pub struct Store(Connection);

#[derive(Debug, Clone)]
pub struct SavedProfile {
    pub id: i64,
    pub profile: Profile,
}

#[derive(Debug, Clone, Copy)]
pub struct Checkpoint {
    pub seq: SequenceNumbers,
    pub reset: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Received,
    Sent,
}

#[derive(Debug, Default, Clone)]
pub struct Filter {
    pub direction: Option<Direction>,
    pub text: String,
    pub application_only: bool,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: i64,
    pub time: UtcMillis,
    pub direction: Direction,
    pub kind: String,
    pub seq: String,
    pub summary: String,
    pub status: String,
    pub transferred: usize,
    pub raw: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct Packet<'a> {
    pub raw: &'a [u8],
    pub direction: Direction,
    pub status: &'a str,
}

pub(crate) struct Transition<'a> {
    pub time: UtcMillis,
    pub checkpoint: Checkpoint,
    pub received: Option<Packet<'a>>,
    pub outgoing: &'a [&'a [u8]],
    pub values: Option<&'a OrderValues>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch(SCHEMA)?;
        Ok(Self(connection))
    }

    pub fn profiles(&self) -> Result<Vec<SavedProfile>> {
        let mut query = self.0.prepare(
            "SELECT id,name,version,sender,target,role,address,heartbeat FROM profile ORDER BY name,id",
        )?;
        let rows = query.query_map([], read_profile)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_profile(&mut self, id: Option<i64>, profile: &Profile) -> Result<i64> {
        profile.validate()?;
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let role = i64::from(profile.role == Role::Acceptor);
        let address = profile.address.to_string();
        if let Some(id) = id {
            let changed = tx.execute(
                "UPDATE profile SET name=?1,role=?2,address=?3,heartbeat=?4
                 WHERE id=?5 AND version=?6 AND sender=?7 AND target=?8",
                params![
                    profile.name,
                    role,
                    address,
                    profile.heartbeat,
                    id,
                    profile.version.begin_string(),
                    profile.sender,
                    profile.target
                ],
            )?;
            if changed != 1 {
                return Err(invalid(
                    "The FIX version and CompIDs of a saved profile are fixed.",
                ));
            }
            tx.commit()?;
            return Ok(id);
        }
        let count: i64 = tx.query_row("SELECT count(*) FROM profile", [], |row| row.get(0))?;
        if count >= PROFILES_MAX {
            return Err(invalid("The database already has 128 profiles."));
        }
        let added = tx.execute(
            "INSERT INTO profile(name,version,sender,target,role,address,heartbeat)
             VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(version,sender,target) DO NOTHING",
            params![
                profile.name,
                profile.version.begin_string(),
                profile.sender,
                profile.target,
                role,
                address,
                profile.heartbeat
            ],
        )?;
        if added != 1 {
            return Err(invalid(
                "A profile already uses this FIX version and CompID pair.",
            ));
        }
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    pub fn checkpoint(&self, id: i64) -> Result<Checkpoint> {
        Ok(self.0.query_row(
            "SELECT next_send,next_receive,reset FROM profile WHERE id=?1",
            [id],
            |row| {
                Ok(Checkpoint {
                    seq: SequenceNumbers {
                        next_sender: row.get(0)?,
                        next_target: row.get(1)?,
                    },
                    reset: row.get(2)?,
                })
            },
        )?)
    }

    pub(crate) fn arm_reset(&self, id: i64) -> Result<()> {
        let updated = self.0.execute(
            "UPDATE profile SET next_send=1,next_receive=1,reset=1 WHERE id=?1",
            [id],
        )?;
        if updated != 1 {
            return Err(invalid("Select a saved profile."));
        }
        Ok(())
    }

    pub fn last_values(&self, id: i64, symbol: Option<&str>) -> Result<Option<OrderValues>> {
        Ok(self
            .0
            .query_row(
                "SELECT symbol,side,quantity,price FROM order_values WHERE profile=?1
             AND (?2 IS NULL OR symbol=?2) ORDER BY recent DESC LIMIT 1",
                params![id, symbol],
                |row| {
                    Ok(OrderValues {
                        symbol: row.get(0)?,
                        side: row.get(1)?,
                        quantity: row.get(2)?,
                        price: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub(crate) fn next_order_id(&self, time: UtcMillis) -> Result<String> {
        let number: u64 = self.0.query_row(
            "UPDATE serial SET value=value+1 RETURNING value",
            [],
            |row| row.get(0),
        )?;
        Ok(format!("FT-{}-{number}", time.0))
    }

    pub(crate) fn commit(&mut self, id: i64, change: Transition<'_>) -> Result<Vec<i64>> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Checkpoint { seq, reset } = change.checkpoint;
        tx.execute(
            "UPDATE profile SET next_send=?1,next_receive=?2,reset=?3 WHERE id=?4",
            params![seq.next_sender, seq.next_target, reset, id],
        )?;
        if let Some(packet) = change.received {
            insert(&tx, id, change.time, &packet)?;
        }
        let mut outgoing = Vec::with_capacity(change.outgoing.len());
        for raw in change.outgoing {
            outgoing.push(insert(
                &tx,
                id,
                change.time,
                &Packet {
                    raw,
                    direction: Direction::Sent,
                    status: "Unconfirmed",
                },
            )?);
        }
        if let Some(values) = change.values {
            let recent = outgoing
                .first()
                .copied()
                .ok_or_else(|| invalid("Order frame missing."))?;
            remember(&tx, id, recent, values)?;
        }
        tx.commit()?;
        Ok(outgoing)
    }

    pub(crate) fn finish_write(&self, id: i64, count: usize, complete: bool) -> Result<()> {
        let status = if complete { "Sent" } else { "Write failed" };
        self.0.execute(
            "UPDATE history SET status=?1,transferred=?2 WHERE id=?3",
            params![status, count, id],
        )?;
        Ok(())
    }

    pub fn purge(&self) -> Result<()> {
        // Delete in batches so that other connections can commit between batches.
        self.0.execute(
            "DELETE FROM history WHERE id IN (SELECT id FROM history WHERE time_ms<?1 LIMIT 4096)",
            [now().0.saturating_sub(RETENTION_MS)],
        )?;
        Ok(())
    }

    pub fn history(&self, id: i64, filter: &Filter, before: Option<i64>) -> Result<Vec<Entry>> {
        let mut query = self.0.prepare(&format!(
            "SELECT id,time_ms,direction,kind,seq,summary,status,transferred,raw {WHERE_HISTORY}
             AND (?6 IS NULL OR id<?6) ORDER BY id DESC LIMIT {PAGE_ROWS}",
        ))?;
        let rows = query.query_map(
            params![
                id,
                direction_param(filter),
                filter.text,
                filter.application_only,
                retention_cutoff(),
                before
            ],
            read_entry,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn export(&self, id: i64, filter: &Filter, path: &Path) -> Result<usize> {
        let parent = path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        let count = self.write_export(id, filter, file.as_file_mut())?;
        file.as_file().sync_all()?;
        file.persist_noclobber(path).map_err(|err| err.error)?;
        Ok(count)
    }

    fn write_export(&self, id: i64, filter: &Filter, file: &mut File) -> Result<usize> {
        let last: i64 = self
            .0
            .query_row("SELECT coalesce(max(id),0) FROM history", [], |row| {
                row.get(0)
            })?;
        let cutoff = retention_cutoff();
        let mut query = self.0.prepare(&format!(
            "SELECT id,substr(raw,1,transferred) {WHERE_HISTORY}
             AND transferred>0 AND id>?6 AND id<=?7 ORDER BY id LIMIT {PAGE_ROWS}",
        ))?;
        let mut count = 0;
        let mut after = 0;
        loop {
            // Release the read snapshot before writing to disk so WAL checkpoints can run.
            let (next, bytes, length) = {
                let mut rows = query.query(params![
                    id,
                    direction_param(filter),
                    filter.text,
                    filter.application_only,
                    cutoff,
                    after,
                    last,
                ])?;
                let mut bytes = Vec::new();
                let mut length = 0;
                let mut next = after;
                while let Some(row) = rows.next()? {
                    next = row.get(0)?;
                    bytes.extend_from_slice(
                        row.get_ref(1)?
                            .as_blob()
                            .map_err(|err| invalid(err.to_string()))?,
                    );
                    length += 1;
                }
                (next, bytes, length)
            };
            file.write_all(&bytes)?;
            count += length;
            after = next;
            if length < PAGE_ROWS as usize {
                return Ok(count);
            }
        }
    }
}

const WHERE_HISTORY: &str = "FROM history WHERE profile=?1 AND (?2 IS NULL OR direction=?2)
 AND (?3='' OR instr(lower(CAST(raw AS TEXT)),lower(?3))>0 OR instr(lower(summary),lower(?3))>0
     OR instr(lower(status),lower(?3))>0)
 AND (?4=0 OR application=1) AND time_ms>=?5";

fn retention_cutoff() -> u64 {
    now().0.saturating_sub(RETENTION_MS)
}

fn direction_param(filter: &Filter) -> Option<bool> {
    filter.direction.map(|dir| dir == Direction::Sent)
}

fn read_entry(row: &Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: row.get(0)?,
        time: UtcMillis(row.get(1)?),
        direction: if row.get(2)? {
            Direction::Sent
        } else {
            Direction::Received
        },
        kind: row.get(3)?,
        seq: row.get(4)?,
        summary: row.get(5)?,
        status: row.get(6)?,
        transferred: row.get(7)?,
        raw: row.get(8)?,
    })
}

fn read_profile(row: &Row<'_>) -> rusqlite::Result<SavedProfile> {
    let version: String = row.get(2)?;
    let address: String = row.get(6)?;
    Ok(SavedProfile {
        id: row.get(0)?,
        profile: Profile {
            name: row.get(1)?,
            version: Version::from_begin_string(&version).ok_or(rusqlite::Error::InvalidQuery)?,
            sender: row.get(3)?,
            target: row.get(4)?,
            role: if row.get(5)? {
                Role::Acceptor
            } else {
                Role::Initiator
            },
            address: address.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
            heartbeat: row.get(7)?,
        },
    })
}

fn remember(tx: &Transaction<'_>, id: i64, recent: i64, values: &OrderValues) -> Result<()> {
    tx.execute(
        "INSERT INTO order_values(profile,symbol,side,quantity,price,recent) VALUES(?1,?2,?3,?4,?5,?6)
         ON CONFLICT(profile,symbol) DO UPDATE SET side=excluded.side, quantity=excluded.quantity,
         price=excluded.price, recent=excluded.recent",
        params![id,values.symbol,values.side,values.quantity,values.price,recent],
    )?;
    tx.execute(
        "DELETE FROM order_values WHERE profile=?1 AND symbol IN (
         SELECT symbol FROM order_values WHERE profile=?1 ORDER BY recent DESC LIMIT -1 OFFSET 256)",
        [id],
    )?;
    Ok(())
}

fn insert(tx: &Transaction<'_>, id: i64, time: UtcMillis, packet: &Packet<'_>) -> Result<i64> {
    let mut decoder = Decoder::new(FIELDS_MAX);
    let message = decoder.decode(packet.raw).ok();
    let fields = message.map(|msg| msg.fields());
    let kind = message.map_or(&[][..], |msg| msg.msg_type());
    let seq = fields
        .and_then(|f| f.get(tag::MSG_SEQ_NUM))
        .unwrap_or_default();
    let summary = fields.map_or_else(String::new, |fields| {
        [11, 55, 39, 150, 58]
            .into_iter()
            .filter_map(|tag| fields.get(tag))
            .map(|value| escape(value.get(..160).unwrap_or(value)))
            .collect::<Vec<_>>()
            .join("  ·  ")
    });
    let sent = packet.direction == Direction::Sent;
    let transferred = if sent { 0 } else { packet.raw.len() };
    tx.execute(
        "INSERT INTO history(profile,time_ms,direction,kind,seq,summary,status,raw,transferred,application)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![id,time.0,sent,escape(kind),escape(seq),summary,packet.status,packet.raw,
            transferred,!kind.is_empty() && !tag::msg_type::is_admin(kind)],
    )?;
    Ok(tx.last_insert_rowid())
}

pub fn escape(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(bytes.len());
    for byte in bytes {
        match byte {
            b'\\' => text.push_str("\\\\"),
            32..=126 => text.push(char::from(*byte)),
            _ => {
                let _ = write!(text, "\\x{byte:02X}");
            }
        }
    }
    text
}
