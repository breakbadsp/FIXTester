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
For a manual reference, open Replace or Cancel and enter OrigClOrdID and
the order fields. FIXTester generates each new ClOrdID.

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
order to a new file and keeps the SOH bytes. It includes any captured fragment. A partial write
exports only the prefix that TCP accepted. An unconfirmed send keeps the
planned frame for inspection. TCP write success does not prove peer receipt.

Storage is in `$XDG_DATA_HOME/fixtester` or
`$HOME/.local/share/fixtester`. Use `--data-dir PATH` for a separate
database. One instance can use each data directory at a time. The
database retains seven days of history, 128 profiles, and the last
values for up to 256 symbols per profile. The database limit is 512 MiB
with the default page size. If SQLite cannot save a session transition,
FIXTester stops that session. The frame limit is 16 KiB and the decoder
limit is 256 fields per message.

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
and Cancel workflow over TCP. The write failure test checks the saved
sequence and the exported byte prefix.

The AlmaLinux command builds the release on AlmaLinux 8, runs the
desktop crate tests there, checks the glibc ceiling of 2.28, and runs the
finished binary with Xvfb and Mesa software OpenGL. The optional path
keeps that binary. The host command verifies a rendered frame in the
current desktop session. With no binary path, it builds a local debug
binary. The v1 platform checks cover Omarchy Wayland and AlmaLinux 8
X11 under Xvfb. Full installed GNOME and hardware OpenGL on AlmaLinux 8
remain unverified. AlmaLinux 9 testing is deferred to v2.
