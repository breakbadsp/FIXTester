# FIXTester proof plan

Depth is `deep`. Wire and order state errors can survive a reconnect or
send the wrong order. Use bounded model checking where event order matters.
Run executable checks against Rust for wire values and UI behavior.

| Component and part | Decision and proof |
| --- | --- |
| Codec framing, length, checksum, and resynchronization | Create a coverage-guided cursor and boundary check. Keep the exact frame and checksum fixtures. |
| Codec tag parsing, binary length/data, group entries, encoding, and timestamps | Fuzz framing and decoding together. Keep the round-trip properties and binary, group, and calendar cases for exact results. |
| Dictionary generator XML, reference resolution, and output | Keep XML and resolution fixtures and the pinned regeneration input. The generator has no runtime peer input. |
| Dictionary versions, schema, group reach, value rules, validation reports, and defaults | Keep tests for each supported version and the field, group, default, and value tables. |
| Venue profile parsing, field and message overrides, group resolution, cycle checks, and CLI | Keep the valid and invalid TOML fixtures and CLI tests. |
| Session Logon and Logout, receive sequence, gap fill, resend, heartbeat, timeout, and reconnect | Create a bounded session receive model. Keep the Rust timer and FIX transition fixtures. Replay a recovery sequence through the public Rust API. |
| Simulator clock, seeded plan, delivery pipe, and two session hosts | Run the default seed range once to check both recovery and fault coverage. Keep replay and lost-recovery-packet tests. |
| Desktop profile and worker lifecycle, socket framing, and TCP dispatch | Keep real loopback for roles, versions, concurrent sessions, checksum recovery, and reset. |
| Desktop SQLite locks, migration, checkpoints, retention, quota, WAL, and export | Keep the send durability model and the storage tests with independent connections, failed writes, and reopen. |
| Order and report construction, grouped legs and lists, references, and additional tags | Keep the version/action tables and validation and wire cases. |
| Automatic order acknowledgement, replace, fill, cancel, duplicate IDs, and order references | Create a generated chain check against the Rust automation and its reports. Keep loopback cases for disconnect, reset, and manual handling. |
| UI workspace, profile selection, timeline, dialogs, and asynchronous completion | Extend the existing egui driver to check an open draft when selection changes. Keep the order and report journeys and wire assertions. |
| Native launcher, package, build tools, and CI | Keep the native smoke command, workspace lint rules, and one check map. Reuse one TLC runner for both models. |

Run the checks through [`.kaizen-checks`](../.kaizen-checks).
Keep an invariant, its model or test, and its listed source paths in step
when behavior changes. Use the [local verification skill](../.claude/skills/verify-fix-session/SKILL.md)
for the model bounds and replay steps.

Run `tools/fuzz-wire.sh` for the fixed-seed campaign used in a pull request.
Set `FUZZ_SECONDS` to search longer. The scheduled check uses 120 seconds.
Keep representative seeds in `fuzz/corpus/wire`. Put minimized failures
that protect an exact wire result in the regular codec tests.
