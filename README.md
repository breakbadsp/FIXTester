# FIXTester

FIXTester is a native application for manual FIX testing on x86-64 Linux.
It uses FIX 4.2, 4.3, and 4.4 over plain TCP. It sends orders and displays
the messages that the peer returns. It keeps session state and message
history locally with bundled SQLite.

## Start a session

1. Run `fixtester`. Select **Add profile**.
2. Enter the local SenderCompID, the peer TargetCompID, the FIX version,
   and a numeric TCP address and port. Use brackets around IPv6 addresses.
3. Select **Initiator** to open a connection or **Acceptor** to listen.
   Give each running acceptor a different listen endpoint. A listener
   serves one connection at a time.
4. Save the profile, then select **Connect** or **Listen**. The session
   shows **Active** after Logon. The workspace shows the next TX and RX
   sequence numbers. It allows up to eight running sessions.

FIXTester manages Logon, Heartbeat, TestRequest, Logout, and resend
requests. It replies with gap fills and does not replay orders.
A frame with a known boundary and a bad checksum stays in history.
Recovery can continue on the same connection. An unknown frame boundary
closes the connection. Session deadlines use elapsed time. FIX timestamps
use UTC.
If a connection closes, an acceptor resumes listening. For an initiator,
select **Connect** again. Select **Stop** to send Logout and stop the
connection or listener.

The FIX version and CompID pair identify a saved session. To use a
different identity, add a profile. To reset a session, stop it, select
**Reset sequences…**, and confirm **Reset to 1**. Arrange for the peer
to send or echo `141=Y` and `34=1` in the next Logon. The initiator
reserves its reset Logon before it tries to write it. If that exchange
is interrupted, verify the peer's numbers before you confirm another
reset. A reset keeps the old message history.

Use a test peer on a trusted network. The application does not require
a FIXSim process, a database server, or an Internet connection.

## Send and inspect orders

| Key | Action |
|---|---|
| F5 | Connect, listen, or stop the selected session. |
| Ctrl+N | Open New limit order. |
| Ctrl+R | Open Replace. |
| Ctrl+K | Open Cancel order. |
| Ctrl+T | Open the application tag editor. |
| Ctrl+F | Focus timeline search. |
| Escape | Close the current dialog. |

Enter a symbol, Buy or Sell, a positive quantity, and a positive limit
price. Select **Send**. New recalls the most recent values. Change the
symbol and leave its field to recall that symbol's values, if available.

Select a received Execution Report to see its status, quantity, price,
and reject details. Select a sent New or Replace to open **Use for
replace** or **Use for cancel**. The form copies that message's ClOrdID
and order fields. Replace carries valid standard instructions that the
replacement supports. Expand **Carried FIX fields** to inspect them.
For a price change, edit the limit price and send.
Select the sent replacement before opening Cancel to use its ClOrdID.
OrigClOrdID is read-only when the form comes from a sent order.
Select **Enter original details manually** to clear the copied fields and
start a blank draft. Enter OrigClOrdID and the order fields. FIXTester generates each new ClOrdID.

Select **Multileg order** on a FIX 4.3 or 4.4 session to send `35=AB`.
Enter the strategy symbol, quantity, and net limit price. Add 2 to 16
legs. Each leg has a symbol, maturity, side, and ratio. Select Future,
Call, or Put. Calls and puts also require a strike. Enter maturity as
`YYYYMM` or `YYYYMMDD`. FIX 4.4 also accepts `YYYYMMwN`. The sides of
the legs define the strategy direction. If your venue uses a specific
strategy CFI code, enter it in **Strategy CFI override**.

Select **Order list** on any supported FIX version to send `35=E`.
Enter 1 to 16 single Limit orders with no bidding process. FIXTester
assigns the list ID, the order IDs, and the group counts. Use the tag
editor when a venue requires fields outside these dialogs.

Open **Additional FIX tags** in any guided message to add optional
top-level fields. Use semicolons between the pairs, for example
`29=2;58=Test fill` on a report. A final semicolon is permitted.
FIXTester trims ASCII spaces around each tag and value. Other bytes
remain the same. The field only adds tags to the current draft. Edit
existing values in their form fields. FIXTester rejects duplicates,
protected tags, and tags that belong to repeating groups.

### Reply to received orders

On an active session, right-click a received order in the timeline or
select it and use **Execution actions** in the inspector. For a list,
select the child by ClOrdID before you choose an action. The form opens
with the selected fields. Review or edit them before you select **Send**.

- New orders (`D`, `AB`, and children of `E`) support pending new,
  acknowledgement, rejection, partial and full fills, cancel, status,
  expire, done for day, stopped, suspended, calculated, restatement,
  trade correction, and trade bust.
- Cancel (`F`) and replace (`G`, `AC`) requests offer pending, accepted,
  and rejected responses to that request. Rejection uses
  `OrderCancelReject` (`35=9`). Order status requests (`H`) offer a
  status report.
- To correct or bust a trade, select a sent fill or correction to copy
  its ExecID into ExecRefID. You can also enter ExecRefID from a received
  new order or list child. Enter the resulting order totals. For FIX 4.2,
  select the referenced fill type when you enter ExecRefID manually.

FIXTester proposes an editable OrderID from the new order's timeline
record. It proposes a different ID for each list child. On later
requests, FIXTester copies the supplied OrderID. If the request omits it,
enter the ID from the earlier report. If you changed the proposed
OrderID, reuse that value for later reports. FIXTester assigns ExecID
on Send. A status report uses ExecID `0`.

For multileg orders on FIX 4.3 and 4.4, the form builds a strategy report
and carries compatible leg fields in dictionary order. Expand
**Additional FIX fields** to inspect the leg fields. Enter cumulative
values and trade prices and quantities as the scenario requires. Use
the tag editor for individual leg executions, custom venue fields, or
manual changes to the carried leg group.

The tag editor accepts an application MsgType, such as `8` for an
Execution Report, and one `Tag=Value` per line. It accepts custom tags
and application MsgTypes. It handles the standard header, framing, and
checksum. Enter text fields in FIX wire order. Supply the fields that
your peer expects, including any group counts. Session messages,
standard header and trailer tags, and binary length/data fields are
reserved.

To try this on one machine, add two FIX 4.4 profiles with opposite
CompIDs. Use `127.0.0.1:9876` for both. Start the acceptor first.
Send New from the initiator. Select the acceptor and use its tag editor
with MsgType `8`. For an acknowledgement, paste these fields and replace
`REPLACE_WITH_CLORDID` with the received value of tag 11.

```text
37=TEST-ORDER-1
17=TEST-EXEC-1
11=REPLACE_WITH_CLORDID
150=0
39=0
55=WIDGET
54=1
151=40
14=0
6=0
```

Select the initiator to inspect the report. Send Replace and Cancel
from the sent rows as described above.

## History and storage

The timeline searches the selected profile for tags, values, summaries,
or transfer status. Filter by direction or show application messages
only. Use **Raw** to display exact bytes with `\xNN` escapes. **Export…**
writes matching captured bytes from one database snapshot in timeline
order to a new file and keeps the SOH bytes. It includes any captured
fragment. A partial write exports only the prefix that TCP accepted.
An unconfirmed send keeps the planned frame for inspection. Retention
and size trimming skip frames while their write batch is active. After a
batch ends or the app restarts, its remaining unconfirmed rows can be
trimmed. If in-flight frames fill the quota, the next send fails before
it writes to TCP. TCP write success does not prove peer receipt. Export captures at most 256 MiB of
bytes in 1 MiB memory chunks. FIXTester releases the SQLite read
snapshot before it writes to the destination. One export runs at a
time. Its temporary output file is also at most 256 MiB.

Storage is in `$XDG_DATA_HOME/fixtester` or
`$HOME/.local/share/fixtester`. Use `--data-dir PATH` for a separate
database. One instance can use each database at a time. Symlinks share
the database lock. The database file must have a single hard link.

History is searchable for seven days. Maintenance removes expired rows
in batches and trims the oldest captured rows toward the 128 MiB soft
target. Before a session transition can commit, FIXTester trims older
rows as needed to stay at or below the 256 MiB hard cap. The charge per
row is the raw payload length, the byte lengths of MsgType, sequence
text, and summary, at least 16 bytes for status, and 128 bytes for the
six integer columns and a fixed metadata allowance. This charge is
independent of SQLite page allocation. On first open, FIXTester charges
existing rows and trims an over-limit database. If the legacy database
has no room for the quota schema, FIXTester removes oldest history in
small batches to free pages. Trimming keeps the profiles, sequence
checkpoints, and remembered order values.

SQLite limits the main database to 131,072 pages of 4096 bytes
(512 MiB). Deleted pages can be reused. Deletion does not shrink the
main file. The page limit includes all tables and indexes. It excludes
the WAL, the shared-memory index, and export files. SQLite attempts an
automatic checkpoint after 128 WAL pages and aims to leave at most
4 MiB of recycled WAL space. If the WAL reaches 16 MiB, FIXTester tries
a truncate checkpoint before the next write. It checks again under the
SQLite write lock. If a concurrent writer crosses the threshold,
FIXTester releases the lock and retries the checkpoint before it reports
a storage error. One bounded transaction can take the WAL past that
threshold. Export destination writes hold no database read snapshot.

The database supports 128 profiles and remembers up to 256 symbols per
profile. If SQLite cannot save a session transition, FIXTester stops
that session. The frame limit is 16 KiB and the decoder limit is 256
fields per message.

## Build and verify

The original protocol crates came from FIXSim commit
`ccf9fc1d997df840ea1f2fe9cc6066a7c6d61690`. The pinned Rust toolchain and
Cargo lockfile build a single egui/eframe Glow executable. SQLite links
into the executable. The Linux desktop still supplies OpenGL and its
Wayland or X11 libraries.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
tools/native-smoke.sh alma /tmp/fixtester
tools/native-smoke.sh host /tmp/fixtester
```

The workspace tests include real loopback TCP for both roles, all three
versions, eight simultaneous sessions, restart, reset, gaps, and
heartbeat recovery. The egui test drives keyboard and accessibility
events in the real workspace code and checks the New, report, Replace,
and Cancel workflow over TCP. Execution tests use a timeline right-click,
the inspector, a list child, and a multileg request. They inspect the
reports and references received over loopback TCP. The write failure
test checks the saved
sequence and the exported byte prefix. Storage tests cover quota
boundaries, migration, concurrent writes, and a purge and WAL truncate
checkpoint during a blocked export write.

The AlmaLinux command builds the release on AlmaLinux 8, runs the
desktop crate tests there, checks the glibc ceiling of 2.28, and runs the
finished binary with Xvfb and Mesa software OpenGL. The optional path
keeps that binary. The host command verifies a rendered frame in the
current desktop session. With no binary path, it builds a local debug
binary. The v1 platform checks cover Omarchy Wayland and AlmaLinux 8
X11 under Xvfb. Full installed GNOME and hardware OpenGL on AlmaLinux 8
remain unverified. AlmaLinux 9 testing is deferred to v2.
