---
name: verify-fix-session
description: Check FIXTester send durability and receive recovery when session, transport, sequence allocation, or SQLite history changes.
disable-model-invocation: true
---

# Verify FIX session and send durability

## Properties and proof plan

| Decision | Property or check | Proof and reason |
| --- | --- | --- |
| Create | `WireHasReservedSequence`: SQLite reserves a sequence before TCP can accept any bytes for it. A restart could otherwise reuse the sequence. | TLC checks commits, attempts, rollbacks, and restarts. `failed_reservation_sends_no_logon` rejects a history insert and checks the TCP stream and SQLite checkpoint. |
| Create | `LiveRowsSurvive`: History cleanup keeps each committed outgoing row until its batch ends. The worker needs that row to record the write result. | TLC checks the order of pin publication, commit, and cleanup. The Rust tests check the same rule with two SQLite connections. |
| Keep | Partial writes, cleanup, and restart tests. | `partial_write_preserves_the_reserved_sequence_and_captured_prefix`, `in_flight_rows_survive_retention_and_quota_from_another_connection`, and `interrupted_send_is_not_pinned_after_the_app_reopens` check the real byte prefix, storage admission, and pin lifetime. |
| Keep | Codec properties, session tests, desktop loopback and egui tests. | They exercise Rust behavior for malformed input, FIX transitions, and user workflows. A storage model would add no coverage for those behaviors. |
| Update | CI check selection. | [`.kaizen-checks`](../../../.kaizen-checks) reruns TLC when a covered path changes. A push to `main` runs all checks. |

## Run and read the results

From the repository root, run `tools/check-tla.sh verification/send-durability/SendDurability.tla`. Use Java 11 or
later. Set `JAVA` to select a Java executable or `TLA2TOOLS` to use a local
TLA+ v1.7.4 jar. The script checks the jar against a pinned SHA-256 digest.
Run `cargo test --locked -p fixtester-desktop --lib` for the linked Rust tests.

Use the shared `statechart` skill to read each TLC result. For a failure,
replay its shortest path against Rust with a test or fault injection. A
trace that Rust cannot reproduce can indicate a model error. Check the code
and the scope before you change a property.

To check that the tests can fail, use scratch copies. Let `AttemptWrite`
emit the staged row before commit in the model, and insert an early TCP write
before `Store::commit` in Rust. The failed reservation must expose the write.
Then allow the model to commit before `PublishPins`. In Rust, move
`Store::protect` after `tx.commit()` and call age purge between the two steps.
Both checks must detect the loss of a live row.

## Coverage and scope

Last confirmed against production code at
`d3fc19c25c8bccb9a5a3c51c58bf2eb289120ce5`.

- [Transport](../../../crates/desktop/src/transport/peer.rs) uses
  `Peer::commit_transition` to commit before `write_outgoing`. The
  `AttemptWrite` action represents zero or more bytes accepted by TCP.
  `RecordResult` and `ReleaseBatch` represent recording the result and
  dropping `PendingWrites`.
- [Store](../../../crates/desktop/src/store.rs) uses an immediate SQLite
  transaction for `Store::commit`, `purge_before`, and `trim_batch`.
  `PublishPins` occurs while that transaction holds the write lock.
  `CommitReservation` makes the checkpoint and pending rows visible together.
  `Restart` keeps committed SQLite state and clears memory.
- [Session](../../../crates/fix-session/src/session.rs) allocates fresh
  sequence numbers in `Session::write_message`. Callers reopen with the
  saved checkpoint in `peer::connect`.

The [TLA+ model](../../../verification/send-durability/SendDurability.tla)
checks two profiles, two fresh sequence numbers per profile, and one row per
batch. It allows cleanup to delete any unpinned row to cover both age and
quota cleanup. It assumes SQLite transaction atomicity and successful
recovery of committed state. The model excludes manual resets, resend gap
fills, exact quota accounting, peer receipt, and progress guarantees.

When covered code changes, check each mapped action against the implementation.
Update the model and linked tests if behavior changes. Move the sync commit
only after you confirm the mapping and rerun both checks. Use
`maintain-formal-verification-skill` to review changes since the sync commit.

## Receive recovery

Run `tools/check-tla.sh verification/session-recovery/SessionRecovery.tla`.
The model checks one receive session and peer sequence numbers 1 through 5.
The receive checkpoint can reach 6. It treats timers as timeout events.
The model covers ordinary Logon and reconnect, in-order and high application
messages, reset-mode messages, and in-order gap fills. The Rust fixtures
and simulator cover exact timers, low sequence numbers, malformed headers,
Logon with ResetSeqNumFlag, Logout, and outgoing resend traffic.

- `DeliveryIsInSequence` permits application delivery only at the expected
  sequence after Logon. A premature delivery could apply an order out of order.
  See `Session::on_message_at` and `sequence_is_next`.
- `RejectedResetDoesNotJump` keeps the checkpoint on a rejected reset-mode
  message and advances by exactly one on a rejected in-sequence gap fill.
  An incorrect jump can skip messages the application still needs.
  See `Session::on_sequence_reset` and `advance_target`.

The linked Rust replay is
`cargo test --locked -p fix-session receive_recovery_rejects_jumps_across_disconnect_and_reconnect`.
Use `statechart` for TLC results. To check sensitivity in a scratch model,
permit an ahead application delivery, then separately make a rejected
reset-mode message rewind the checkpoint. Replay both faults in Rust.
The sync commit above also applies to [`session.rs`](../../../crates/fix-session/src/session.rs)
for receive recovery.
