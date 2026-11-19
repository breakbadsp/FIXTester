# FIXTester

FIXTester is a native desktop tool for manual FIX testing. The first
development change extracts the FIX protocol crates and checks that a
native window can render on the target platforms.

## Version 1 scope

- Up to eight active sessions in any mix of initiator and acceptor modes.
  Either peer can open the TCP connection. FIXTester sends orders and
  displays acknowledgements, rejections, and fills.
- FIX 4.2, 4.3, and 4.4 over plain TCP. Save sequence numbers across
  restarts. Provide an explicit reset. Answer resend requests with gap fills.
- New Order starts with a Limit order. Show Symbol, Side, Quantity, and
  Price. Recall the last values for the selected symbol. Open Replace
  and Cancel from a sent message or enter the original reference manually.
- Display decoded Execution Reports and raw messages. Allow custom tags
  in the tag editor. Generate the session header, body length, and checksum.
- Keep seven days of searchable local message history. Provide export.
- Deliver one native executable for Omarchy and RHEL-compatible 8 and 9
  desktops. Scope ends at FIX session state and message history.

## Development checks

The protocol crates come from FIXSim commit
`ccf9fc1d997df840ea1f2fe9cc6066a7c6d61690`. The existing seeded session
simulation remains part of the workspace tests.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Run `tools/native-smoke.sh host` from a desktop session. Run
`tools/native-smoke.sh alma` with Podman to build on AlmaLinux 8 and run
the same binary under Xvfb and Mesa on AlmaLinux 8 and 9. The probe checks
for a rendered frame. The container checks cover the X11 and software
OpenGL path. Installed GNOME desktops require a separate check.
