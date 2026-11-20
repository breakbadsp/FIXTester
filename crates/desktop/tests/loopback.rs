#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use fix_codec::{Decoder, write_utc_timestamp};
use fix_dict::Version;
use fix_session::{Role, State};
use fixtester_desktop::model::{Application, Order, OrderKind, OrderValues, Profile, now};
use fixtester_desktop::store::{Direction, Filter, SavedProfile, Store};
use fixtester_desktop::transport::{Phase, Send, Sessions};

#[path = "support/wire.rs"]
mod wire;
use wire::Wire;

fn wait(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(start.elapsed() < Duration::from_secs(6), "Timed out.");
        thread::sleep(Duration::from_millis(15));
    }
}

fn profile(role: Role, version: Version, address: SocketAddr, index: usize) -> Profile {
    Profile {
        name: format!("Test {index}"),
        version,
        sender: format!("TEST{index}"),
        target: format!("PEER{index}"),
        role,
        address,
        heartbeat: 30,
    }
}

fn reserve() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").unwrap()
}

fn setup(root: &Path, profile: Profile) -> (PathBuf, Store, SavedProfile) {
    let db = root.join("test.db");
    let mut store = Store::open(&db).unwrap();
    let id = store.save_profile(None, &profile).unwrap();
    (db, store, SavedProfile { id, profile })
}

fn connect(sessions: &Sessions, saved: &SavedProfile, listener: Option<&TcpListener>) -> Wire {
    let stream = if let Some(listener) = listener {
        listener.accept().unwrap().0
    } else {
        wait(|| {
            sessions
                .status(saved.id)
                .is_some_and(|s| s.phase == Phase::Listening)
        });
        TcpStream::connect(saved.profile.address).unwrap()
    };
    Wire::new(stream, &saved.profile)
}

fn handshake(sessions: &Sessions, id: i64, wire: &mut Wire, send: u64, receive: u64, reset: bool) {
    if wire.profile.role == Role::Acceptor {
        wire.logon(receive, reset);
    }
    let raw = wire.receive(b"A", send);
    let mut decoder = Decoder::new(256);
    assert_eq!(
        decoder.decode(&raw).unwrap().fields().get_bool(141),
        Some(reset)
    );
    if wire.profile.role == Role::Initiator {
        wire.logon(receive, reset);
    }
    wait(|| {
        sessions
            .status(id)
            .is_some_and(|s| s.phase == Phase::Link(State::Active))
    });
}

#[test]
fn order_flow_over_tcp_in_every_version_and_role() {
    for version in Version::ALL {
        for role in [Role::Initiator, Role::Acceptor] {
            let dir = tempfile::tempdir().unwrap();
            let listener = reserve();
            let address = listener.local_addr().unwrap();
            let listener = (role == Role::Initiator).then_some(listener);
            let (db, store, saved) = setup(dir.path(), profile(role, version, address, 1));
            let id = saved.id;
            let mut sessions = Sessions::default();
            sessions.start(&store, saved.clone()).unwrap();
            let mut wire = connect(&sessions, &saved, listener.as_ref());
            handshake(&sessions, id, &mut wire, 1, 1, false);
            let mut values = OrderValues {
                symbol: "DEMO".into(),
                side: "1".into(),
                quantity: "25".into(),
                price: "12.50".into(),
            };
            sessions
                .send(
                    id,
                    Send::Order(Order::new(OrderKind::New, values.clone(), String::new())),
                )
                .unwrap();
            let new = wire.receive(b"D", 2);
            let mut decoder = Decoder::new(256);
            let fields = decoder.decode(&new).unwrap().fields();
            for &tag in version.dictionary().message("D").unwrap().body.required {
                assert!(
                    fields.get(tag).is_some(),
                    "Missing required tag {tag} in {version:?} D"
                );
            }
            assert_eq!(fields.get(40), Some(&b"2"[..]));
            assert_eq!(fields.get(44), Some(&b"12.50"[..]));
            assert_eq!(
                fields.get(21),
                (version != Version::Fix44).then_some(&b"1"[..])
            );
            let new_id = fields.get(11).unwrap().to_vec();
            let report = wire.send(
                b"8",
                2,
                &[(11, &new_id), (55, b"DEMO"), (39, b"0"), (150, b"0")],
            );
            wait(|| store.checkpoint(id).unwrap().seq.next_target == 3);
            values.price = "12.55".into();
            sessions
                .send(
                    id,
                    Send::Order(Order::new(
                        OrderKind::Replace,
                        values.clone(),
                        String::from_utf8_lossy(&new_id).into_owned(),
                    )),
                )
                .unwrap();
            let replace = wire.receive(b"G", 3);
            let fields = decoder.decode(&replace).unwrap().fields();
            for &tag in version.dictionary().message("G").unwrap().body.required {
                assert!(
                    fields.get(tag).is_some(),
                    "Missing required tag {tag} in {version:?} G"
                );
            }
            assert_eq!(fields.get(41), Some(new_id.as_slice()));
            assert_eq!(fields.get(44), Some(&b"12.55"[..]));
            assert_eq!(fields.get(38), Some(&b"25"[..]));
            let replace_id = fields.get(11).unwrap().to_vec();
            assert_ne!(replace_id, new_id);
            sessions
                .send(
                    id,
                    Send::Order(Order::new(
                        OrderKind::Cancel,
                        values.clone(),
                        String::from_utf8_lossy(&replace_id).into_owned(),
                    )),
                )
                .unwrap();
            let cancel = wire.receive(b"F", 4);
            let fields = decoder.decode(&cancel).unwrap().fields();
            for &tag in version.dictionary().message("F").unwrap().body.required {
                assert!(
                    fields.get(tag).is_some(),
                    "Missing required tag {tag} in {version:?} F"
                );
            }
            assert_eq!(fields.get(41), Some(replace_id.as_slice()));
            assert_eq!(fields.get(44), None);
            sessions
                .send(
                    id,
                    Send::Tags(Application::tags(version, "U123", "9001=demo").unwrap()),
                )
                .unwrap();
            let custom = wire.receive(b"U123", 5);
            assert_eq!(
                decoder.decode(&custom).unwrap().fields().get(9001),
                Some(&b"demo"[..])
            );
            wire.send(b"1", 3, &[(112, b"peer-check")]);
            let heartbeat = wire.receive(b"0", 6);
            assert_eq!(
                decoder.decode(&heartbeat).unwrap().fields().get(112),
                Some(&b"peer-check"[..])
            );
            sessions.stop(id);
            wire.receive(b"5", 7);
            wire.send(b"5", 4, &[]);
            wait(|| !sessions.is_running(id));
            let reopened = Store::open(&db).unwrap();
            assert_eq!(
                reopened.last_values(id, Some("DEMO")).unwrap(),
                Some(values)
            );
            let filtered = Filter {
                text: "demo".into(),
                direction: Some(Direction::Received),
                application_only: true,
            };
            let history = reopened.history(id, &filtered, None).unwrap();
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].raw, report);
            let export = dir.path().join("report.fix");
            assert_eq!(reopened.export(id, &filtered, &export).unwrap(), 1);
            assert_eq!(std::fs::read(export).unwrap(), report);
        }
    }
}

#[test]
fn restart_gap_fill_peer_reset_rejection_and_confirmed_reset() {
    let dir = tempfile::tempdir().unwrap();
    let listener = reserve();
    let (db, store, saved) = setup(
        dir.path(),
        profile(
            Role::Initiator,
            Version::Fix44,
            listener.local_addr().unwrap(),
            1,
        ),
    );
    let id = saved.id;
    let mut sessions = Sessions::default();
    sessions.start(&store, saved.clone()).unwrap();
    let mut wire = connect(&sessions, &saved, Some(&listener));
    handshake(&sessions, id, &mut wire, 1, 1, false);
    sessions
        .send(
            id,
            Send::Tags(Application::tags(Version::Fix44, "D", "11=NeverReplay").unwrap()),
        )
        .unwrap();
    wire.receive(b"D", 2);
    wire.stream.write_all(b"8=FIX.4").unwrap();
    drop(wire);
    wait(|| !sessions.is_running(id));
    assert!(
        store
            .history(id, &Filter::default(), None)
            .unwrap()
            .iter()
            .any(|row| row.raw == b"8=FIX.4" && row.status == "Incomplete frame")
    );
    drop(sessions);
    drop(store);

    let store = Store::open(&db).unwrap();
    let mut sessions = Sessions::default();
    sessions.start(&store, saved.clone()).unwrap();
    let mut wire = connect(&sessions, &saved, Some(&listener));
    handshake(&sessions, id, &mut wire, 3, 2, false);
    wire.send(b"2", 3, &[(7, b"2"), (16, b"2")]);
    let gap = wire.receive(b"4", 2);
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&gap).unwrap().fields();
    assert_eq!(fields.get_bool(123), Some(true));
    assert_eq!(fields.get_bool(43), Some(true));
    assert_eq!(fields.get_u64(36), Some(3));
    wire.send(b"0", 5, &[]);
    let resend = wire.receive(b"2", 4);
    assert_eq!(
        decoder.decode(&resend).unwrap().fields().get_u64(7),
        Some(4)
    );
    let mut timestamp = [0; 21];
    wire.send(
        b"4",
        4,
        &[
            (123, b"Y"),
            (36, b"6"),
            (43, b"Y"),
            (122, write_utc_timestamp(now(), &mut timestamp)),
        ],
    );
    wait(|| store.checkpoint(id).unwrap().seq.next_target == 6);
    assert!(sessions.reset(&store, id).is_err());
    drop(wire);
    wait(|| !sessions.is_running(id));

    sessions.start(&store, saved.clone()).unwrap();
    let mut wire = connect(&sessions, &saved, Some(&listener));
    wire.receive(b"A", 5);
    wire.logon(1, true);
    wait(|| !sessions.is_running(id));
    assert_eq!(store.checkpoint(id).unwrap().seq.next_target, 6);
    sessions.reset(&store, id).unwrap();
    assert!(store.checkpoint(id).unwrap().reset);
    sessions.start(&store, saved.clone()).unwrap();
    let mut wire = connect(&sessions, &saved, Some(&listener));
    handshake(&sessions, id, &mut wire, 1, 1, true);
    let checkpoint = store.checkpoint(id).unwrap();
    assert!(!checkpoint.reset);
    assert_eq!(checkpoint.seq.next_sender, 2);
    assert_eq!(checkpoint.seq.next_target, 2);
}

#[test]
fn eight_active_sessions_in_a_mix_and_unique_identity() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db");
    let mut store = Store::open(&db).unwrap();
    let mut sessions = Sessions::default();
    let mut peers = Vec::new();
    for index in 0..8 {
        let role = if index % 2 == 0 {
            Role::Acceptor
        } else {
            Role::Initiator
        };
        let listener = reserve();
        let profile = profile(role, Version::Fix43, listener.local_addr().unwrap(), index);
        let listener = (role == Role::Initiator).then_some(listener);
        let id = store.save_profile(None, &profile).unwrap();
        assert!(store.save_profile(None, &profile).is_err());
        let saved = SavedProfile { id, profile };
        sessions.start(&store, saved.clone()).unwrap();
        let mut wire = connect(&sessions, &saved, listener.as_ref());
        handshake(&sessions, id, &mut wire, 1, 1, false);
        peers.push(wire);
    }
    assert_eq!(sessions.count(), 8);
    let nine = profile(
        Role::Acceptor,
        Version::Fix44,
        reserve().local_addr().unwrap(),
        9,
    );
    let id = store.save_profile(None, &nine).unwrap();
    assert!(
        sessions
            .start(&store, SavedProfile { id, profile: nine })
            .is_err()
    );
}

#[test]
fn acceptor_requires_armed_reset_and_reconnects_after_heartbeat_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let mut profile = profile(
        Role::Acceptor,
        Version::Fix44,
        reserve().local_addr().unwrap(),
        1,
    );
    profile.heartbeat = 1;
    let (_db, store, saved) = setup(dir.path(), profile);
    let id = saved.id;
    let mut sessions = Sessions::default();
    sessions.reset(&store, id).unwrap();
    sessions.start(&store, saved.clone()).unwrap();
    let mut wrong_peer = connect(&sessions, &saved, None);
    wrong_peer.logon(1, false);
    let mut bytes = [0; 128];
    assert_eq!(
        std::io::Read::read(&mut wrong_peer.stream, &mut bytes).unwrap(),
        0
    );
    assert!(store.checkpoint(id).unwrap().reset);
    assert_eq!(store.checkpoint(id).unwrap().seq.next_target, 1);

    let mut peer = connect(&sessions, &saved, None);
    handshake(&sessions, id, &mut peer, 1, 1, true);
    assert!(!store.checkpoint(id).unwrap().reset);
    peer.receive(b"0", 2);
    let test = peer.receive(b"1", 3);
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&test).unwrap().fields();
    peer.send(b"0", 2, &[(112, fields.get(112).unwrap())]);
    peer.receive(b"0", 4);
    peer.receive(b"1", 5);
    wait(|| {
        sessions.status(id).is_some_and(|status| {
            status.phase == Phase::Listening && status.notice.contains("HeartbeatTimeout")
        })
    });
    let mut peer = connect(&sessions, &saved, None);
    handshake(&sessions, id, &mut peer, 6, 3, false);
    assert_eq!(store.checkpoint(id).unwrap().seq.next_target, 4);
}

#[test]
fn checksum_error_preserves_the_following_frame_and_requests_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let listener = reserve();
    let (_db, store, saved) = setup(
        dir.path(),
        profile(
            Role::Initiator,
            Version::Fix44,
            listener.local_addr().unwrap(),
            1,
        ),
    );
    let id = saved.id;
    let mut sessions = Sessions::default();
    sessions.start(&store, saved.clone()).unwrap();
    let mut peer = connect(&sessions, &saved, Some(&listener));
    handshake(&sessions, id, &mut peer, 1, 1, false);

    let mut bad = peer.message(b"8", 2, &[(11, b"CORRUPTED"), (150, b"0")]);
    let checksum_digit = bad.len() - 2;
    bad[checksum_digit] = if bad[checksum_digit] == b'0' {
        b'1'
    } else {
        b'0'
    };
    let good = peer.message(b"8", 3, &[(11, b"NEXT"), (150, b"0")]);
    peer.stream
        .write_all(&[bad.as_slice(), good.as_slice()].concat())
        .unwrap();
    let resend = peer.receive(b"2", 2);
    let mut decoder = Decoder::new(256);
    assert_eq!(
        decoder.decode(&resend).unwrap().fields().get_u64(7),
        Some(2)
    );
    let rows = store
        .history(
            id,
            &Filter {
                direction: Some(Direction::Received),
                text: String::new(),
                application_only: true,
            },
            None,
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].raw, good);
    assert_eq!(rows[0].status, "Received");
    assert_eq!(rows[1].raw, bad);
    assert_eq!(rows[1].status, "Invalid checksum");
    assert_eq!(store.checkpoint(id).unwrap().seq.next_target, 2);

    let mut timestamp = [0; 21];
    peer.send(
        b"4",
        2,
        &[
            (43, b"Y"),
            (122, write_utc_timestamp(now(), &mut timestamp)),
            (123, b"Y"),
            (36, b"4"),
        ],
    );
    wait(|| store.checkpoint(id).unwrap().seq.next_target == 4);
    assert!(sessions.is_running(id));

    let malformed = [b"8=FIX.4.4\x019=X\x01".as_slice(), &good].concat();
    peer.stream.write_all(&malformed).unwrap();
    wait(|| !sessions.is_running(id));
    let rows = store.history(id, &Filter::default(), None).unwrap();
    assert_eq!(rows[0].status, "Invalid frame");
    assert_eq!(store.checkpoint(id).unwrap().seq.next_target, 4);
}

type ExpectedLeg<'a> = (
    &'a [u8],
    &'a [u8],
    &'a [u8],
    &'a [u8],
    Option<&'a [u8]>,
    &'a [u8],
);

#[test]
fn grouped_orders_send_versioned_counts_and_entries() {
    use fixtester_desktop::model::grouped::{Contract, GroupedOrder, Leg, MultilegOrder, Side};

    for version in Version::ALL {
        let dir = tempfile::tempdir().unwrap();
        let listener = reserve();
        let profile = profile(Role::Initiator, version, listener.local_addr().unwrap(), 1);
        let (_, store, saved) = setup(dir.path(), profile);
        let mut sessions = Sessions::default();
        sessions.start(&store, saved.clone()).unwrap();
        let mut peer = connect(&sessions, &saved, Some(&listener));
        handshake(&sessions, saved.id, &mut peer, 1, 1, false);

        let orders = ["ALPHA", "BETA"].map(|symbol| OrderValues {
            symbol: symbol.into(),
            quantity: "10".into(),
            price: "5.50".into(),
            ..OrderValues::default()
        });
        sessions
            .send(
                saved.id,
                Send::GroupedOrder(GroupedOrder::List(orders.into())),
            )
            .unwrap();

        let raw = peer.receive(b"E", 2);
        let mut decoder = Decoder::new(256);
        let list = decoder.decode(&raw).unwrap().fields();
        assert_eq!(list.get(394), Some(b"3".as_slice()));
        assert_eq!(list.get_u64(68), Some(2));
        let definition = version.dictionary().message("E").unwrap().body.groups[0];
        let group = list.group(definition).unwrap().unwrap();
        assert_eq!(group.len(), 2);
        let list_id = std::str::from_utf8(list.get(66).unwrap()).unwrap();
        for (index, (entry, symbol)) in group.entries().zip(["ALPHA", "BETA"]).enumerate() {
            let id = format!("{list_id}-{}", index + 1);
            let number = (index + 1).to_string();
            let time = entry.get(60).unwrap();
            assert!(fix_codec::parse_utc_timestamp(time).is_some());
            let mut expected = vec![(11, id.as_bytes()), (67, number.as_bytes())];
            if version != Version::Fix44 {
                expected.push((21, b"1"));
            }
            expected.extend([
                (55, symbol.as_bytes()),
                (54, b"1"),
                (60, time),
                (38, b"10"),
                (40, b"2"),
                (44, b"5.50"),
            ]);
            assert_eq!(entry.iter().collect::<Vec<_>>(), expected);
        }

        let multileg = GroupedOrder::Multileg(MultilegOrder {
            symbol: "DEMO-SPREAD".into(),
            quantity: "5".into(),
            price: "-0.25".into(),
            cfi: String::new(),
            legs: [
                (Contract::Future, "FUT-Z", Side::Buy, "", "202712"),
                (Contract::Call, "OPT-C", Side::Sell, "100", "20271217"),
                (Contract::Put, "OPT-P", Side::Buy, "95", "202712"),
            ]
            .map(|(contract, symbol, side, strike, maturity)| Leg {
                contract,
                symbol: symbol.into(),
                side,
                strike: strike.into(),
                maturity: maturity.into(),
                ratio: "2".into(),
            })
            .into(),
        });
        sessions
            .send(saved.id, Send::GroupedOrder(multileg))
            .unwrap();
        if version == Version::Fix42 {
            wait(|| {
                sessions
                    .status(saved.id)
                    .is_some_and(|state| state.send_error.is_some())
            });
            assert_eq!(store.checkpoint(saved.id).unwrap().seq.next_sender, 3);
        } else {
            let raw = peer.receive(b"AB", 3);
            let order = decoder.decode(&raw).unwrap().fields();
            assert_eq!(order.get(54), Some(&b"B"[..]));
            assert_eq!(order.get(44), Some(&b"-0.25"[..]));
            assert_eq!(
                order.get(21),
                (version == Version::Fix43).then_some(&b"1"[..])
            );
            let definition = version
                .dictionary()
                .message("AB")
                .unwrap()
                .body
                .groups
                .iter()
                .find(|group| group.count_tag == 555)
                .unwrap();
            let group = order.group(definition).unwrap().unwrap();
            assert_eq!(group.len(), 3);
            assert_eq!(order.get(167), Some(b"MLEG".as_slice()));
            assert_eq!(order.get(461), Some(b"OMXXXS".as_slice()));
            let expected_legs: [ExpectedLeg<'_>; 3] = [
                (b"FUT-Z", b"FXXXXX", b"FUT", b"202712", None, b"1"),
                (b"OPT-C", b"OCXXXX", b"OPT", b"20271217", Some(b"100"), b"2"),
                (b"OPT-P", b"OPXXXX", b"OPT", b"202712", Some(b"95"), b"1"),
            ];
            for (leg, (symbol, cfi, security, maturity, strike, side)) in
                group.entries().zip(expected_legs)
            {
                let mut expected = vec![(600, symbol), (608, cfi)];
                if version == Version::Fix44 {
                    expected.push((609, security));
                }
                let maturity_tag = if version == Version::Fix43 && maturity.len() == 8 {
                    611
                } else {
                    610
                };
                expected.push((maturity_tag, maturity));
                if let Some(strike) = strike {
                    expected.push((612, strike));
                }
                expected.extend([(623, b"2".as_slice()), (624, side)]);
                assert_eq!(leg.iter().collect::<Vec<_>>(), expected);
            }
        }
    }
}
