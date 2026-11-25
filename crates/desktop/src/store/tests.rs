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
                reset: Reset::None,
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
    ..LIMITS
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
    assert_eq!(store.checkpoint(other).unwrap().reset, Reset::Armed);
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
        assert_eq!(checkpoint.reset, Reset::Armed);
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
    store.finish_write(rows.ids()[0], 5, false).unwrap();
    assert_eq!(history_bytes(&store.connection).unwrap(), charge);
    let history = store.history(id, &Filter::default(), None).unwrap();
    assert_eq!(history[0].transferred, 5);
    assert_eq!(history[0].raw, raw);
}

#[test]
fn in_flight_rows_survive_retention_and_quota_from_another_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pinned.db");
    let mut sender = Store::open_with_limits(&path, SMALL).unwrap();
    let mut other = sender.connector()().unwrap();
    let id = sender.save_profile(None, &profile(1)).unwrap();
    let other_id = other.save_profile(None, &profile(2)).unwrap();
    let checkpoint = sender.checkpoint(id).unwrap();
    let time = now();
    let pending = sender
        .commit(
            id,
            &Transition {
                time,
                checkpoint,
                received: None,
                outgoing: &[b"pending1", b"pending2", b"pending3"],
                values: None,
            },
        )
        .unwrap();
    assert_eq!(history_bytes(&sender.connection).unwrap(), SMALL.hard);
    assert!(other.purge_before(time.0 + 1).unwrap());
    other.purge().unwrap();
    assert_eq!(
        stored_bytes(&other, id),
        [b"pending3", b"pending2", b"pending1"]
    );
    let error = append(&mut other, other_id, time, b"new-data", 2).unwrap_err();
    assert!(error.to_string().contains("Pending FIX writes"));
    assert_eq!(other.checkpoint(other_id).unwrap().seq.next_sender, 1);

    sender.finish_write(pending.ids()[0], 8, true).unwrap();
    assert!(sender.finish_write(pending.ids()[0], 8, true).is_err());
    // The other frames did not reach TCP. Dropping the failed batch unpins them.
    drop(pending);
    append(&mut other, other_id, time, b"new-data", 2).unwrap();
    let rows = other.history(id, &Filter::default(), None).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.status == "Unconfirmed"));
    assert!(other.purge_before(time.0 + 1).unwrap());
    assert_eq!(stored_bytes(&sender, id).len(), 0);
    assert!(sender.finish_write(-1, 0, false).is_err());
}

/// An age purge on another connection, and its result after the next WAL
/// commit on this thread.
#[derive(Default)]
struct PurgeAfterCommit {
    other: Option<(Store, u64)>,
    result: Option<Result<bool>>,
}

thread_local! {
    static PURGE_AFTER_COMMIT: std::cell::RefCell<PurgeAfterCommit> =
        std::cell::RefCell::default();
}

/// SQLite calls the WAL hook after a commit releases the write lock and
/// before `Store::commit` returns: the first moment another connection can
/// purge the new rows.
#[allow(clippy::unnecessary_wraps)] // The type of a rusqlite WAL hook.
fn purge_after_commit(_: &rusqlite::hooks::Wal, _: std::ffi::c_int) -> rusqlite::Result<()> {
    PURGE_AFTER_COMMIT.with_borrow_mut(|purge| {
        if let Some((other, cutoff)) = &purge.other {
            purge.result = Some(other.purge_before(*cutoff));
        }
    });
    Ok(())
}

/// `LiveRowsSurvive`: `Store::commit` pins its outgoing rows before the
/// commit makes them visible to a purge on another connection.
#[test]
fn outgoing_rows_are_pinned_before_the_commit_releases_the_write_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("publication.db");
    let mut sender = Store::open_with_limits(&path, SMALL).unwrap();
    let other = sender.connector()().unwrap();
    let id = sender.save_profile(None, &profile(1)).unwrap();
    let time = now();
    PURGE_AFTER_COMMIT.set(PurgeAfterCommit {
        other: Some((other, time.0 + 1)),
        result: None,
    });
    sender.connection.wal_hook(Some(purge_after_commit));
    let pending = sender
        .commit(
            id,
            &Transition {
                time,
                checkpoint: sender.checkpoint(id).unwrap(),
                received: None,
                outgoing: &[b"pending1"],
                values: None,
            },
        )
        .unwrap();
    sender.connection.wal_hook(None);
    let purge = PURGE_AFTER_COMMIT.take();
    assert!(
        purge.result.unwrap().unwrap(),
        "the purge ran after the commit"
    );
    assert_eq!(stored_bytes(&sender, id), [b"pending1"]);

    // The same purge removes the row when it has no pin.
    drop(pending);
    let (other, cutoff) = purge.other.unwrap();
    assert!(other.purge_before(cutoff).unwrap());
    assert_eq!(stored_bytes(&sender, id).len(), 0);
}

#[test]
fn interrupted_send_is_not_pinned_after_the_app_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("restart.db");
    let time = now();
    let mut old = Store::open_with_limits(&db, SMALL).unwrap();
    let id = old.save_profile(None, &profile(1)).unwrap();
    let pending = old
        .commit(
            id,
            &Transition {
                time,
                checkpoint: old.checkpoint(id).unwrap(),
                received: None,
                outgoing: &[b"pending1"],
                values: None,
            },
        )
        .unwrap();
    drop(old);

    // A new app instance has no previous in-memory pins.
    let reopened = Store::open_with_limits(&db, SMALL).unwrap();
    assert_eq!(
        reopened.history(id, &Filter::default(), None).unwrap()[0].status,
        "Unconfirmed"
    );
    assert!(reopened.purge_before(time.0 + 1).unwrap());
    assert_eq!(stored_bytes(&reopened, id).len(), 0);
    drop(pending);
}

#[test]
fn full_legacy_database_reclaims_history_pages_before_quota_setup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("full-legacy.db");
    let limits = HistoryLimits {
        max_pages: 32,
        ..LIMITS
    };
    let legacy = Connection::open(&path).unwrap();
    legacy.pragma_update(None, "page_size", 4096).unwrap();
    legacy
        .pragma_update(None, "max_page_count", limits.max_pages)
        .unwrap();
    legacy.execute_batch(SCHEMA).unwrap();
    legacy.execute(
        "INSERT INTO profile(id,name,version,sender,target,role,address,heartbeat,next_send,next_receive,reset)
         VALUES(1,'Old profile','FIX.4.4','US','THEM',0,'127.0.0.1:9876',30,21,51,1)", [],
    ).unwrap();
    let insert = "INSERT INTO history(profile,time_ms,direction,kind,seq,summary,status,raw,transferred,application)
                  VALUES(1,?1,0,'','','','Incomplete frame',?2,4096,0)";
    let raw = vec![b'x'; 4096];
    let mut total = 0;
    for _ in 0..limits.max_pages {
        match legacy.execute(insert, params![now().0, raw]) {
            Ok(_) => total += 1,
            Err(error) => {
                assert_eq!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DiskFull)
                );
                break;
            }
        }
    }
    assert!(total > MIGRATION_TRIM_ROWS);
    let first_attempt = legacy.unchecked_transaction().unwrap();
    assert_eq!(
        first_attempt
            .execute_batch(QUOTA_SCHEMA)
            .unwrap_err()
            .sqlite_error_code(),
        Some(rusqlite::ErrorCode::DiskFull)
    );
    drop(first_attempt);
    assert_eq!(schema_version(&legacy).unwrap(), 0);
    drop(legacy);

    for _ in 0..2 {
        let store = Store::open_with_limits(&path, limits).unwrap();
        assert_eq!(schema_version(&store.connection).unwrap(), 1);
        let retained = store.history(1, &Filter::default(), None).unwrap();
        assert_eq!(retained.len(), total - MIGRATION_TRIM_ROWS);
        assert_eq!(
            retained.last().unwrap().id,
            i64::try_from(MIGRATION_TRIM_ROWS + 1).unwrap()
        );
        let saved = store.checkpoint(1).unwrap();
        assert_eq!(saved.seq.next_sender, 21);
        assert_eq!(saved.seq.next_target, 51);
        assert_eq!(saved.reset, Reset::Armed);
        assert_eq!(store.profiles().unwrap()[0].profile.name, "Old profile");
    }
}

#[test]
fn symlink_uses_sqlites_wal_filename() {
    let dir = tempfile::tempdir().unwrap();
    let actual = dir.path().join("actual.db");
    let alias = dir.path().join("alias.db");
    let real = Store::open(&actual).unwrap();
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    let mut linked = Store::open(&alias).unwrap();
    linked.save_profile(None, &profile(1)).unwrap();
    assert_eq!(linked.wal, real.wal);
    assert!(linked.wal_len().unwrap() > 0);
    assert_eq!(linked.wal_len().unwrap(), real.wal_len().unwrap());
}

#[test]
fn wal_metadata_fault_has_a_database_error_and_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("metadata.db")).unwrap();
    let id = store.save_profile(None, &profile(1)).unwrap();
    let broken = dir.path().join("metadata-loop");
    std::os::unix::fs::symlink(&broken, &broken).unwrap();
    store.wal = broken;
    let err = append(&mut store, id, now(), b"received", 2).unwrap_err();
    let crate::Error::Database(error) = err else {
        panic!("Expected a database error, got {err}")
    };
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::SystemIoFailure)
    );
    assert!(error.to_string().contains("metadata-loop"));
    assert!(error.to_string().contains("os error"));
    assert_eq!(store.checkpoint(id).unwrap().seq.next_sender, 1);
}

#[test]
fn wal_crossing_threshold_before_write_lock_retries_after_checkpoint() {
    use std::sync::atomic::{AtomicBool, Ordering};

    static WAITING: AtomicBool = AtomicBool::new(false);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("race.db");
    let limits = HistoryLimits {
        wal_pressure: 48 * 1024,
        ..LIMITS
    };
    let holder = Store::open(&db).unwrap();
    let mut waiter = Store::open_with_limits(&db, limits).unwrap();
    let id = truncate_checkpoint(&holder);
    assert_eq!(id, (0, 0, 0));
    waiter
        .connection
        .busy_handler(Some(|n| {
            WAITING.store(true, Ordering::Release);
            thread::sleep(Duration::from_millis(1));
            n < 3000
        }))
        .unwrap();
    let lock = holder.write_transaction().unwrap();
    let thread = thread::spawn(move || waiter.save_profile(None, &profile(1)));
    let start = std::time::Instant::now();
    while !WAITING.load(Ordering::Acquire) {
        assert!(start.elapsed() < Duration::from_secs(3));
        thread::sleep(Duration::from_millis(1));
    }
    // Dirty enough pages to move the WAL past the gate before the waiter gets the lock.
    lock.execute_batch(
        "CREATE TABLE wal_race_fixture(bytes BLOB);
        INSERT INTO wal_race_fixture VALUES(zeroblob(65536));",
    )
    .unwrap();
    lock.commit().unwrap();
    let saved = thread.join().unwrap().unwrap();
    assert_eq!(holder.profiles().unwrap()[0].id, saved);
    assert!(holder.wal_len().unwrap() < limits.wal_pressure);
}

#[test]
fn dropping_one_batch_keeps_the_reused_id_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("pins.db")).unwrap();
    let failed = store.protect(vec![7]);
    let active = store.protect(vec![7]);
    drop(failed);
    assert_eq!(store.pinned_ids(), "[7]");
    drop(active);
    assert_eq!(store.pinned_ids(), "[]");
}
