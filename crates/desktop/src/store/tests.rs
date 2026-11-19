#![allow(clippy::unwrap_used)]

use std::sync::{Barrier, mpsc};
use std::thread;

use super::*;

fn profile(index: usize) -> Profile {
    Profile {
        name: format!("History {index}"),
        version: Version::Fix44,
        sender: format!("TEST{index}"),
        target: "PEER".into(),
        role: Role::Initiator,
        address: "127.0.0.1:9876".parse().unwrap(),
        heartbeat: 30,
    }
}

fn append(store: &mut Store, id: i64, time: UtcMillis, raw: &[u8], sequence: u64) -> Result<()> {
    store.commit(
        id,
        &Transition {
            time,
            checkpoint: Checkpoint {
                seq: SequenceNumbers {
                    next_sender: sequence,
                    next_target: sequence,
                },
                reset: false,
            },
            received: Some(Packet {
                raw,
                direction: Direction::Received,
                status: "Incomplete frame",
            }),
            outgoing: &[],
            values: None,
        },
    )?;
    Ok(())
}

fn stored_bytes(store: &Store, id: i64) -> Vec<Vec<u8>> {
    store
        .history(id, &Filter::default(), None)
        .unwrap()
        .into_iter()
        .map(|row| row.raw)
        .collect()
}

struct BlockedWriter {
    entered: Option<mpsc::Sender<()>>,
    resume: mpsc::Receiver<()>,
    output: Vec<u8>,
}

impl Write for BlockedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(entered) = self.entered.take() {
            entered.send(()).map_err(std::io::Error::other)?;
            self.resume
                .recv_timeout(Duration::from_secs(10))
                .map_err(std::io::Error::other)?;
        }
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn truncate_checkpoint(store: &Store) -> (u32, u32, u32) {
    store
        .connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap()
}

#[test]
fn purge_and_checkpoint_complete_while_export_destination_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("test.db");
    let mut exporter = Store::open(&database).unwrap();
    let purger = Store::open(&database).unwrap();
    let id = exporter.save_profile(None, &profile(1)).unwrap();
    let time = now();
    let mut expected = Vec::new();
    let count = (PAGE_ROWS * 2 + 1) as usize;
    for i in 0..count {
        let raw = format!("Captured bytes {i:08000}\x01");
        append(&mut exporter, id, time, raw.as_bytes(), 1).unwrap();
        expected.extend_from_slice(raw.as_bytes());
    }

    let (entered, blocked) = mpsc::channel();
    let (resume, wait) = mpsc::channel();
    let writer = thread::spawn(move || {
        let mut sink = BlockedWriter {
            entered: Some(entered),
            resume: wait,
            output: Vec::new(),
        };
        let records = exporter
            .write_export(id, &Filter::default(), &mut sink)
            .unwrap();
        (records, sink.output)
    });
    blocked.recv_timeout(Duration::from_secs(5)).unwrap();
    while !purger.purge_before(time.0 + 1).unwrap() {}
    assert_eq!(stored_bytes(&purger, id).len(), 0);
    assert!(purger.wal_len().unwrap() > 0);
    assert_eq!(truncate_checkpoint(&purger), (0, 0, 0));
    assert_eq!(purger.wal_len().unwrap(), 0);
    resume.send(()).unwrap();
    assert_eq!(writer.join().unwrap(), (count, expected));
}

const FIXTURE_CHARGE: usize = 144 + 8;
const SMALL: HistoryLimits = HistoryLimits {
    soft: 2 * FIXTURE_CHARGE,
    hard: 3 * FIXTURE_CHARGE,
    wal_pressure: LIMITS.wal_pressure,
};

#[test]
fn hard_admission_and_soft_maintenance_remove_oldest_without_resetting_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_with_limits(&dir.path().join("quota.db"), SMALL).unwrap();
    let id = store.save_profile(None, &profile(1)).unwrap();
    let other = store.save_profile(None, &profile(2)).unwrap();
    store.arm_reset(other).unwrap();
    let time = now();
    for step in 1..=3 {
        append(
            &mut store,
            id,
            time,
            format!("row{step:05}").as_bytes(),
            step + 1,
        )
        .unwrap();
    }
    assert_eq!(history_bytes(&store.connection).unwrap(), SMALL.hard);
    assert_eq!(stored_bytes(&store, id).len(), 3);
    append(&mut store, id, time, b"row00004", 5).unwrap();
    assert_eq!(
        stored_bytes(&store, id),
        [b"row00004", b"row00003", b"row00002"]
    );
    assert!(store.purge().unwrap());
    assert_eq!(history_bytes(&store.connection).unwrap(), SMALL.soft);
    assert_eq!(stored_bytes(&store, id), [b"row00004", b"row00003"]);

    let huge = vec![b'X'; SMALL.hard];
    assert!(append(&mut store, id, time, &huge, 6).is_err());
    assert_eq!(stored_bytes(&store, id), [b"row00004", b"row00003"]);
    assert_eq!(store.checkpoint(id).unwrap().seq.next_sender, 5);
    assert_eq!(store.checkpoint(id).unwrap().seq.next_target, 5);
    assert!(store.checkpoint(other).unwrap().reset);
    assert_eq!(store.profiles().unwrap().len(), 2);
}

#[test]
fn history_ledger_and_checkpoint_agree_across_concurrent_writers() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("quota.db");
    let mut store = Store::open_with_limits(&db, SMALL).unwrap();
    let ids = [
        store.save_profile(None, &profile(1)).unwrap(),
        store.save_profile(None, &profile(2)).unwrap(),
    ];
    let mut workers = Vec::new();
    let barrier = std::sync::Arc::new(Barrier::new(ids.len()));
    for id in ids {
        let db = db.clone();
        let barrier = barrier.clone();
        workers.push(thread::spawn(move || {
            let mut connection = Store::open_with_limits(&db, SMALL).unwrap();
            barrier.wait();
            for step in 1..=24 {
                append(&mut connection, id, now(), b"row00000", step + 1).unwrap();
                assert!(history_bytes(&connection.connection).unwrap() <= SMALL.hard);
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    let count: usize = ids.iter().map(|&id| stored_bytes(&store, id).len()).sum();
    assert_eq!(count, 3);
    assert_eq!(
        history_bytes(&store.connection).unwrap(),
        count * FIXTURE_CHARGE
    );
    for id in ids {
        assert_eq!(store.checkpoint(id).unwrap().seq.next_target, 25);
    }
}

#[test]
fn version_zero_migration_charges_existing_rows_and_preserves_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("old.db");
    let legacy = Connection::open(&db).unwrap();
    legacy.execute_batch(SCHEMA).unwrap();
    legacy.execute(
        "INSERT INTO profile(id,name,version,sender,target,role,address,heartbeat,next_send,next_receive,reset)
         VALUES(1,'Old profile','FIX.4.4','US','THEM',0,'127.0.0.1:9876',30,88,93,1)", [],
    ).unwrap();
    for step in 1..=4 {
        legacy.execute(
            "INSERT INTO history(profile,time_ms,direction,kind,seq,summary,status,raw,transferred,application)
             VALUES(1,?1,0,'','','','Incomplete frame',?2,8,0)",
            params![now().0, format!("row{step:05}").as_bytes()],
        ).unwrap();
    }
    drop(legacy);

    for _ in 0..2 {
        let store = Store::open_with_limits(&db, SMALL).unwrap();
        assert_eq!(history_bytes(&store.connection).unwrap(), SMALL.soft);
        assert_eq!(stored_bytes(&store, 1), [b"row00004", b"row00003"]);
        let checkpoint = store.checkpoint(1).unwrap();
        assert_eq!(checkpoint.seq.next_sender, 88);
        assert_eq!(checkpoint.seq.next_target, 93);
        assert!(checkpoint.reset);
    }
}

#[test]
fn wal_pressure_stops_writes_until_a_pinned_reader_releases() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("wal.db");
    let limits = HistoryLimits {
        wal_pressure: 96 * 1024,
        ..LIMITS
    };
    let mut store = Store::open_with_limits(&db, limits).unwrap();
    store
        .connection
        .busy_timeout(Duration::from_millis(50))
        .unwrap();
    let id = store.save_profile(None, &profile(1)).unwrap();
    assert_eq!(truncate_checkpoint(&store), (0, 0, 0));
    let reader = Connection::open(&db).unwrap();
    let snapshot = reader.unchecked_transaction().unwrap();
    let read_sequence = || {
        snapshot
            .query_row("SELECT next_send FROM profile", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap()
    };
    assert_eq!(read_sequence(), 1);
    let stop = (2..100)
        .find(|&seq| append(&mut store, id, now(), b"received", seq).is_err())
        .unwrap();
    let pinned_size = store.wal_len().unwrap();
    assert!(pinned_size >= limits.wal_pressure);
    assert!(pinned_size < 2 * limits.wal_pressure);
    assert_eq!(read_sequence(), 1);
    assert_eq!(store.checkpoint(id).unwrap().seq.next_sender, stop - 1);
    assert!(append(&mut store, id, now(), b"received", stop).is_err());
    assert_eq!(store.wal_len().unwrap(), pinned_size);
    drop(snapshot);

    append(&mut store, id, now(), b"received", stop).unwrap();
    assert_eq!(store.checkpoint(id).unwrap().seq.next_sender, stop);
    assert!(store.wal_len().unwrap() < limits.wal_pressure);
}

#[test]
fn wire_text_and_payload_share_the_quota_with_transfer_metadata() {
    let mut encoder = fix_codec::Encoder::new(1024);
    let mut body = encoder.start(b"D");
    body.field_u64(34, 7).field(11, b"DEMO");
    let raw = body.finish(b"FIX.4.4").unwrap();
    let charge = 144 + raw.len() + "D".len() + "7".len() + "DEMO".len();
    let limits = HistoryLimits {
        soft: charge,
        hard: charge,
        ..LIMITS
    };
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_with_limits(&dir.path().join("metadata.db"), limits).unwrap();
    let id = store.save_profile(None, &profile(1)).unwrap();
    let rows = store
        .commit(
            id,
            &Transition {
                time: now(),
                checkpoint: store.checkpoint(id).unwrap(),
                received: None,
                outgoing: &[raw],
                values: None,
            },
        )
        .unwrap();
    assert_eq!(history_bytes(&store.connection).unwrap(), charge);
    store.finish_write(rows[0], 5, false).unwrap();
    assert_eq!(history_bytes(&store.connection).unwrap(), charge);
    let history = store.history(id, &Filter::default(), None).unwrap();
    assert_eq!(history[0].transferred, 5);
    assert_eq!(history[0].raw, raw);
}
