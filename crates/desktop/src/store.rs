use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fix_codec::{Decoder, UtcMillis, tag};
use fix_dict::Version;
use fix_session::{Role, SequenceNumbers};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, ffi, params};

use crate::model::{FIELDS_MAX, OrderValues, Profile, now};
use crate::{Result, invalid};

const RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const PAGE_ROWS: u32 = 200;
const PROFILES_MAX: i64 = 128;
const SCHEMA: &str = include_str!("schema.sql");
const QUOTA_SCHEMA: &str = include_str!("store/history-quota.sql");
const TRIM_ROWS: usize = 256;
const MIGRATION_TRIM_ROWS: usize = 8;
const MIB: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy)]
struct HistoryLimits {
    soft: usize,
    hard: usize,
    wal_pressure: u64,
    max_pages: u32,
}

const LIMITS: HistoryLimits = HistoryLimits {
    soft: 128 * MIB,
    hard: 256 * MIB,
    wal_pressure: 16 * MIB as u64,
    max_pages: 131_072,
};

#[derive(Debug)]
pub struct Store {
    connection: Connection,
    database: PathBuf,
    wal: PathBuf,
    limits: HistoryLimits,
    pins: Arc<Mutex<BTreeMap<i64, usize>>>,
}

#[derive(Debug)]
pub(crate) struct PendingWrites {
    ids: Vec<i64>,
    pins: Arc<Mutex<BTreeMap<i64, usize>>>,
}

impl PendingWrites {
    pub(crate) fn ids(&self) -> &[i64] {
        &self.ids
    }
}

impl Drop for PendingWrites {
    fn drop(&mut self) {
        if !self.ids.is_empty() {
            let mut pins = self
                .pins
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for id in &self.ids {
                if let std::collections::btree_map::Entry::Occupied(mut pin) = pins.entry(*id) {
                    if *pin.get() == 1 {
                        pin.remove();
                    } else {
                        *pin.get_mut() -= 1;
                    }
                }
            }
        }
    }
}

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
        Self::open_with_limits(path, LIMITS)
    }

    fn open_with_limits(path: &Path, limits: HistoryLimits) -> Result<Self> {
        Self::connect(path, limits, Arc::default())
    }

    /// Create another connection for the same locked app instance.
    pub fn connector(&self) -> impl FnOnce() -> Result<Self> + Send + 'static + use<> {
        let path = self.database.clone();
        let limits = self.limits;
        let pins = Arc::clone(&self.pins);
        move || Self::connect(&path, limits, pins)
    }

    fn connect(
        path: &Path,
        limits: HistoryLimits,
        pins: Arc<Mutex<BTreeMap<i64, usize>>>,
    ) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA page_size=4096;
             PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             PRAGMA wal_autocheckpoint=128;
             PRAGMA journal_size_limit=4194304;
             PRAGMA cache_spill=OFF;",
        )?;
        let page_size: u32 = connection.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        if page_size != 4096 {
            return Err(invalid(
                "The FIXTester database requires 4096-byte SQLite pages.",
            ));
        }

        connection.pragma_update(None, "max_page_count", limits.max_pages)?;
        let database = std::fs::canonicalize(path)?;
        let mut wal = database.as_os_str().to_os_string();
        wal.push("-wal");
        let store = Self {
            connection,
            database,
            wal: PathBuf::from(wal),
            limits,
            pins,
        };
        store.migrate()?;
        if history_bytes(&store.connection)? > limits.hard {
            store.trim_to(limits.soft)?;
        }
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        while schema_version(&self.connection)? != 1 {
            let tx = self.write_transaction()?;
            match schema_version(&tx)? {
                0 => {}
                1 => return Ok(()),
                _ => return Err(invalid("This FIXTester database uses a newer schema.")),
            }
            let result = tx
                .execute_batch(SCHEMA)
                .and_then(|()| tx.execute_batch(QUOTA_SCHEMA));
            match result {
                Ok(()) => return Ok(tx.commit()?),
                Err(error) if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull) => {
                    // SQLITE_FULL can roll back the transaction itself.
                    drop(tx);
                    if !self.reclaim_legacy_pages()? {
                        return Err(error.into());
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn reclaim_legacy_pages(&self) -> Result<bool> {
        let tx = self.write_transaction()?;
        if schema_version(&tx)? != 0 {
            return Ok(true);
        }
        // The next migration attempt can reuse reclaimed pages. A crash can
        // leave this batch committed with schema version 0, which is safe to retry.
        let removed = tx.execute(
            "DELETE FROM history WHERE id IN (SELECT id FROM history ORDER BY id LIMIT ?1)",
            [MIGRATION_TRIM_ROWS],
        )?;
        tx.commit()?;
        Ok(removed != 0)
    }

    fn wal_len(&self) -> Result<u64> {
        match std::fs::metadata(&self.wal) {
            Ok(metadata) => Ok(metadata.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(database_fault(
                ffi::SQLITE_IOERR,
                &format!("Cannot inspect WAL {}: {error}", self.wal.display()),
            )),
        }
    }

    fn write_transaction(&self) -> Result<Transaction<'_>> {
        for _ in 0..2 {
            if self.wal_len()? >= self.limits.wal_pressure {
                self.connection
                    .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
            }
            let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
            if self.wal_len()? < self.limits.wal_pressure {
                return Ok(tx);
            }
            // Another writer can cross the gate before BEGIN gets the lock.
            // Release the lock before trying a truncate checkpoint again.
            tx.rollback()?;
        }
        Err(storage_pressure(
            "The SQLite WAL remains above the write threshold after checkpoint.",
        ))
    }

    fn pinned_ids(&self) -> String {
        let ids = self
            .pins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        format!(
            "[{}]",
            ids.keys().map(i64::to_string).collect::<Vec<_>>().join(",")
        )
    }

    fn protect(&self, ids: Vec<i64>) -> PendingWrites {
        if !ids.is_empty() {
            let mut pins = self
                .pins
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for id in &ids {
                *pins.entry(*id).or_default() += 1;
            }
        }
        PendingWrites {
            ids,
            pins: Arc::clone(&self.pins),
        }
    }

    pub fn profiles(&self) -> Result<Vec<SavedProfile>> {
        let mut query = self.connection.prepare(
            "SELECT id,name,version,sender,target,role,address,heartbeat FROM profile ORDER BY name,id",
        )?;
        let rows = query.query_map([], read_profile)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_profile(&mut self, id: Option<i64>, profile: &Profile) -> Result<i64> {
        profile.validate()?;
        let tx = self.write_transaction()?;
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
        Ok(self.connection.query_row(
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
        let tx = self.write_transaction()?;
        let updated = tx.execute(
            "UPDATE profile SET next_send=1,next_receive=1,reset=1 WHERE id=?1",
            [id],
        )?;
        if updated != 1 {
            return Err(invalid("Select a saved profile."));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn last_values(&self, id: i64, symbol: Option<&str>) -> Result<Option<OrderValues>> {
        Ok(self
            .connection
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
        let tx = self.write_transaction()?;
        let number: u64 = tx.query_row(
            "UPDATE serial SET value=value+1 RETURNING value",
            [],
            |row| row.get(0),
        )?;
        tx.commit()?;
        Ok(format!("FT-{}-{number}", time.0))
    }

    pub(crate) fn commit(&mut self, id: i64, change: &Transition<'_>) -> Result<PendingWrites> {
        for _ in 0..4 {
            let tx = self.write_transaction()?;
            let before = history_bytes(&tx)?;
            let last_existing: i64 =
                tx.query_row("SELECT coalesce(max(id),0) FROM history", [], |row| {
                    row.get(0)
                })?;
            let outgoing = Self::record(&tx, id, change)?;
            let after = history_bytes(&tx)?;
            let added = after - before;
            let available = self.limits.hard.checked_sub(added).ok_or_else(|| {
                storage_pressure("One FIX session transition exceeds the history hard cap.")
            })?;
            if trim_oldest(
                &tx,
                after,
                self.limits.hard,
                last_existing,
                &self.pinned_ids(),
            )? <= self.limits.hard
            {
                // Publish the pins before SQLite releases the lock to another trimmer.
                let pending = self.protect(outgoing);
                tx.commit()?;
                return Ok(pending);
            }
            tx.rollback()?;
            self.trim_to(available)?;
        }
        Err(storage_pressure(
            "Concurrent FIX traffic exhausted the history quota.",
        ))
    }

    fn record(tx: &Transaction<'_>, id: i64, change: &Transition<'_>) -> Result<Vec<i64>> {
        let Checkpoint { seq, reset } = change.checkpoint;
        tx.execute(
            "UPDATE profile SET next_send=?1,next_receive=?2,reset=?3 WHERE id=?4",
            params![seq.next_sender, seq.next_target, reset, id],
        )?;
        if let Some(packet) = &change.received {
            insert(tx, id, change.time, packet)?;
        }
        let mut outgoing = Vec::with_capacity(change.outgoing.len());
        for raw in change.outgoing {
            outgoing.push(insert(
                tx,
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
            remember(tx, id, recent, values)?;
        }
        Ok(outgoing)
    }

    pub(crate) fn finish_write(&self, id: i64, count: usize, complete: bool) -> Result<()> {
        let status = if complete { "Sent" } else { "Write failed" };
        let tx = self.write_transaction()?;
        let changed = tx.execute(
            "UPDATE history SET status=?1,transferred=?2
             WHERE id=?3 AND direction=1 AND status='Unconfirmed'",
            params![status, count, id],
        )?;
        if changed != 1 {
            return Err(database_fault(
                ffi::SQLITE_CORRUPT,
                &format!("Cannot record write result for history row {id}."),
            ));
        }
        tx.commit()?;
        Ok(())
    }

    /// Returns true when retention and size maintenance have caught up.
    pub fn purge(&self) -> Result<bool> {
        let age_done = self.purge_before(retention_cutoff())?;
        let (_, remaining) = self.trim_batch(self.limits.soft)?;
        Ok(age_done && remaining <= self.limits.soft)
    }

    fn purge_before(&self, cutoff: u64) -> Result<bool> {
        let tx = self.write_transaction()?;
        let removed = tx.execute(
            "DELETE FROM history WHERE id IN (SELECT id FROM history
             WHERE time_ms<?1 AND id NOT IN (SELECT value FROM json_each(?2))
             ORDER BY time_ms LIMIT ?3)",
            params![cutoff, self.pinned_ids(), TRIM_ROWS],
        )?;
        tx.commit()?;
        Ok(removed < TRIM_ROWS)
    }

    fn trim_to(&self, target: usize) -> Result<()> {
        // Another writer can replenish history between batches. Bound the work in one call.
        let excess = history_bytes(&self.connection)?.saturating_sub(target);
        let batches = excess.div_ceil(144 * TRIM_ROWS) + 4;
        for _ in 0..batches {
            let (before, remaining) = self.trim_batch(target)?;
            if remaining <= target {
                return Ok(());
            }
            if remaining == before {
                return Err(storage_pressure(
                    "Pending FIX writes occupy the available history quota.",
                ));
            }
        }
        Err(storage_pressure(
            "History trim did not reach its target. Retry after traffic slows.",
        ))
    }

    fn trim_batch(&self, target: usize) -> Result<(usize, usize)> {
        let tx = self.write_transaction()?;
        let before = history_bytes(&tx)?;
        let remaining = trim_oldest(&tx, before, target, i64::MAX, &self.pinned_ids())?;
        tx.commit()?;
        Ok((before, remaining))
    }

    pub fn history(&self, id: i64, filter: &Filter, before: Option<i64>) -> Result<Vec<Entry>> {
        let mut query = self.connection.prepare(&format!(
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

    fn write_export(&self, id: i64, filter: &Filter, file: &mut impl Write) -> Result<usize> {
        let (count, chunks) = self.capture_export(id, filter)?;
        for chunk in &chunks {
            file.write_all(chunk)?;
        }
        Ok(count)
    }

    fn capture_export(&self, id: i64, filter: &Filter) -> Result<(usize, Vec<Vec<u8>>)> {
        // A single SELECT keeps one snapshot. Its cursor drops before destination writes.
        let mut query = self.connection.prepare(&format!(
            "SELECT raw,transferred {WHERE_HISTORY} AND transferred>0 ORDER BY id",
        ))?;
        let mut rows = query.query(params![
            id,
            direction_param(filter),
            filter.text,
            filter.application_only,
            retention_cutoff(),
        ])?;
        let mut count = 0;
        let mut remaining = self.limits.hard;
        let mut chunks = Vec::<Vec<u8>>::new();
        let mut chunk_limit = 0;
        while let Some(row) = rows.next()? {
            let raw = row
                .get_ref(0)?
                .as_blob()
                .map_err(|err| invalid(err.to_string()))?;
            let mut prefix = raw
                .get(..row.get::<_, usize>(1)?)
                .ok_or_else(|| invalid("Invalid captured byte count."))?;
            while !prefix.is_empty() {
                if let Some(chunk) = chunks.last_mut() {
                    let added = prefix.len().min(chunk_limit - chunk.len());
                    chunk.extend_from_slice(&prefix[..added]);
                    prefix = &prefix[added..];
                    if prefix.is_empty() {
                        break;
                    }
                }
                if remaining == 0 {
                    return Err(storage_pressure("The export exceeds the history hard cap."));
                }
                chunk_limit = remaining.min(MIB);
                let mut chunk = Vec::new();
                chunk
                    .try_reserve_exact(chunk_limit)
                    .map_err(|err| invalid(format!("Cannot allocate export buffer: {err}")))?;
                remaining -= chunk_limit;
                chunks.push(chunk);
            }
            count += 1;
        }
        Ok((count, chunks))
    }
}

fn trim_oldest(
    tx: &Transaction<'_>,
    mut bytes: usize,
    target: usize,
    last_eligible: i64,
    pins: &str,
) -> Result<usize> {
    if bytes <= target {
        return Ok(bytes);
    }
    let last = {
        let mut query = tx.prepare(
            "SELECT id,charge FROM history WHERE id<=?1
            AND id NOT IN (SELECT value FROM json_each(?2)) ORDER BY id LIMIT ?3",
        )?;
        let mut rows = query.query(params![last_eligible, pins, TRIM_ROWS])?;
        let mut last = None;
        while let Some(row) = rows.next()? {
            last = Some(row.get::<_, i64>(0)?);
            bytes -= row.get::<_, usize>(1)?;
            if bytes <= target {
                break;
            }
        }
        last
    };
    if let Some(last) = last {
        tx.execute(
            "DELETE FROM history WHERE id<=?1 AND id NOT IN (SELECT value FROM json_each(?2))",
            params![last, pins],
        )?;
    }
    Ok(bytes)
}

fn schema_version(connection: &Connection) -> rusqlite::Result<u32> {
    connection.query_row("PRAGMA user_version", [], |row| row.get(0))
}

fn database_fault(code: i32, message: &str) -> crate::Error {
    rusqlite::Error::SqliteFailure(ffi::Error::new(code), Some(message.into())).into()
}

fn storage_pressure(message: &str) -> crate::Error {
    database_fault(ffi::SQLITE_FULL, message)
}

fn history_bytes(connection: &Connection) -> Result<usize> {
    Ok(
        connection.query_row("SELECT bytes FROM history_quota WHERE id=1", [], |row| {
            row.get(0)
        })?,
    )
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

#[cfg(test)]
mod tests;
