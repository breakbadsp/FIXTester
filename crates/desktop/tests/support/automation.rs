use super::*;
use fixtester_desktop::model::execution::{
    Source,
    automation::{Response, Rule, Settings},
};

fn config(new: Response, replace: Response, cancel: Response) -> Settings {
    Settings {
        enabled: true,
        rules: [new, replace, cancel].map(|response| Rule {
            response,
            quantity: "2.5".into(),
            text: "Simulation reject".into(),
            ..Rule::default()
        }),
    }
}

struct Fixture {
    wire: Wire,
    store: Store,
    sessions: Sessions,
    saved: SavedProfile,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn start(role: Role, version: Version, settings: &Settings) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let listener = reserve();
        let address = listener.local_addr().unwrap();
        let listener = (role == Role::Initiator).then_some(listener);
        let (_, store, saved) = setup(dir.path(), profile(role, version, address, 1));
        let mut sessions = Sessions::default();
        sessions
            .start_with_automation(&store, saved.clone(), settings)
            .unwrap();
        let mut wire = connect(&sessions, &saved, listener.as_ref());
        handshake(&sessions, saved.id, &mut wire, 1, 1, false);
        Self {
            wire,
            store,
            sessions,
            saved,
            _dir: dir,
        }
    }

    fn silent(&mut self, tx: u64, rx: u64) {
        self.wire.send(b"1", tx, &[(112, b"automation-check")]);
        self.expect(b"0", rx, &[(112, b"automation-check")]);
    }

    fn expect(&mut self, kind: &[u8], seq: u64, expected: &[(u32, &[u8])]) -> Vec<u8> {
        let raw = self.wire.receive(kind, seq);
        let mut decoder = Decoder::new(256);
        let fields = decoder.decode(&raw).unwrap().fields();
        for &(tag, value) in expected {
            assert_eq!(fields.get(tag), Some(value), "tag {tag} on {raw:?}");
        }
        if kind == b"8" || kind == b"9" {
            let version = self.saved.profile.version;
            let dictionary = version.dictionary();
            let def = dictionary
                .message(std::str::from_utf8(kind).unwrap())
                .unwrap();
            for &tag in def.body.required {
                assert!(
                    fields.get(tag).is_some(),
                    "Missing required tag {tag} in {version:?}"
                );
            }
            assert_eq!(
                fields.get(20),
                (kind == b"8" && version == Version::Fix42).then_some(&b"0"[..])
            );
        }
        raw
    }

    fn expect_manual(&self, seq: &str, reason: &str) {
        let rows = self
            .store
            .history(self.saved.id, &Filter::default(), None)
            .unwrap();
        let row = rows
            .iter()
            .find(|row| row.direction == Direction::Received && row.seq == seq)
            .unwrap();
        assert!(row.status.contains(reason), "{}", row.status);
        assert!(Source::identify(self.saved.profile.version, row).is_some());
    }
}

fn request(
    wire: &mut Wire,
    kind: &[u8],
    seq: u64,
    id: &[u8],
    original: &[u8],
    quantity: &[u8],
    price: &[u8],
) {
    let mut time = [0; 21];
    let timestamp = write_utc_timestamp(now(), &mut time);
    let mut fields: Vec<(u32, &[u8])> = vec![
        (11, id),
        (55, b"DEMO"),
        (54, b"1"),
        (38, quantity),
        (60, timestamp),
    ];
    if kind != b"D" {
        fields.push((41, original));
    }
    if kind != b"F" {
        fields.push((40, if price.is_empty() { b"1" } else { b"2" }));
        if !price.is_empty() {
            fields.push((44, price));
        }
        if wire.profile.version != Version::Fix44 {
            fields.push((21, b"1"));
        }
    }
    wire.send(kind, seq, &fields);
}

fn order_id(report: &[u8]) -> Vec<u8> {
    Decoder::new(256)
        .decode(report)
        .unwrap()
        .fields()
        .get(37)
        .unwrap()
        .to_vec()
}

fn assert_no_original(report: &[u8]) {
    assert_eq!(
        Decoder::new(256).decode(report).unwrap().fields().get(41),
        None
    );
}

#[test]
fn partial_new_replace_and_cancel_carry_chain_totals() {
    for version in Version::ALL {
        let mut f = Fixture::start(
            Role::Acceptor,
            version,
            &config(
                Response::PartialFill,
                Response::PartialFill,
                Response::PartialFill,
            ),
        );
        let trade: &[u8] = if version == Version::Fix42 {
            b"1"
        } else {
            b"F"
        };
        request(&mut f.wire, b"D", 2, b"new", b"", b"10", b"12.5");
        let ack = f.expect(
            b"8",
            2,
            &[
                (11, b"new"),
                (150, b"0"),
                (39, b"0"),
                (14, b"0"),
                (151, b"10"),
            ],
        );
        let id = order_id(&ack);
        f.expect(
            b"8",
            3,
            &[
                (37, &id),
                (11, b"new"),
                (150, trade),
                (39, b"1"),
                (32, b"2.5"),
                (31, b"12.5"),
                (14, b"2.5"),
                (151, b"7.5"),
                (6, b"12.5"),
            ],
        );
        request(&mut f.wire, b"G", 3, b"replace", b"new", b"15", b"13.5");
        f.expect(
            b"8",
            4,
            &[
                (37, &id),
                (11, b"replace"),
                (41, b"new"),
                (150, b"5"),
                (39, b"1"),
                (38, b"15"),
                (14, b"2.5"),
                (151, b"12.5"),
                (6, b"12.5"),
            ],
        );
        let fill = f.expect(
            b"8",
            5,
            &[
                (37, &id),
                (11, b"replace"),
                (150, trade),
                (39, b"1"),
                (31, b"13.5"),
                (14, b"5"),
                (151, b"10"),
                (6, b"13"),
            ],
        );
        assert_no_original(&fill);
        request(&mut f.wire, b"F", 4, b"cancel", b"replace", b"15", b"");
        let fill = f.expect(
            b"8",
            6,
            &[
                (37, &id),
                (11, b"replace"),
                (150, trade),
                (39, b"1"),
                (32, b"2.5"),
                (14, b"7.5"),
                (151, b"7.5"),
                (6, b"13.166667"),
            ],
        );
        assert_no_original(&fill);
        f.expect(
            b"8",
            7,
            &[
                (37, &id),
                (11, b"cancel"),
                (41, b"replace"),
                (150, b"4"),
                (39, b"4"),
                (14, b"7.5"),
                (151, b"0"),
                (6, b"13.166667"),
            ],
        );
    }
}

#[test]
fn full_fill_uses_correct_request_and_fill_versions() {
    for version in Version::ALL {
        let trade: &[u8] = if version == Version::Fix42 {
            b"2"
        } else {
            b"F"
        };
        let status: &[u8] = if version == Version::Fix42 {
            b"5"
        } else {
            b"0"
        };
        let mut f = Fixture::start(
            Role::Acceptor,
            version,
            &config(
                Response::Acknowledge,
                Response::FullFill,
                Response::FullFill,
            ),
        );
        request(&mut f.wire, b"D", 2, b"new", b"", b"10", b"12");
        let id = order_id(&f.expect(b"8", 2, &[(150, b"0")]));
        request(&mut f.wire, b"G", 3, b"replace", b"new", b"15", b"13");
        f.expect(
            b"8",
            3,
            &[
                (37, &id),
                (11, b"replace"),
                (41, b"new"),
                (150, b"5"),
                (39, status),
                (14, b"0"),
                (151, b"15"),
            ],
        );
        f.expect(
            b"8",
            4,
            &[
                (37, &id),
                (11, b"replace"),
                (150, trade),
                (39, b"2"),
                (32, b"15"),
                (14, b"15"),
                (151, b"0"),
                (6, b"13"),
            ],
        );
        request(&mut f.wire, b"D", 4, b"second", b"", b"10", b"14");
        let second = order_id(&f.expect(b"8", 5, &[(150, b"0")]));
        assert_ne!(second, id);
        request(&mut f.wire, b"F", 5, b"cancel", b"second", b"10", b"");
        let fill = f.expect(
            b"8",
            6,
            &[
                (37, &second),
                (11, b"second"),
                (150, trade),
                (39, b"2"),
                (31, b"14"),
                (14, b"10"),
                (151, b"0"),
            ],
        );
        assert_no_original(&fill);
        f.expect(
            b"9",
            7,
            &[
                (37, &second),
                (11, b"cancel"),
                (41, b"second"),
                (39, b"2"),
                (102, b"0"),
                (434, b"1"),
            ],
        );
    }
}

#[test]
fn new_full_fill_needs_an_explicit_market_price() {
    for override_price in ["", "11.125"] {
        let mut settings = config(Response::FullFill, Response::Manual, Response::Manual);
        override_price.clone_into(&mut settings.rules[0].price);
        let mut f = Fixture::start(Role::Acceptor, Version::Fix44, &settings);
        request(&mut f.wire, b"D", 2, b"market", b"", b"10", b"");
        if override_price.is_empty() {
            f.silent(3, 2);
            f.expect_manual("2", "Set a fill price override");
        } else {
            let id = order_id(&f.expect(b"8", 2, &[(150, b"0"), (151, b"10")]));
            f.expect(
                b"8",
                3,
                &[
                    (37, &id),
                    (150, b"F"),
                    (39, b"2"),
                    (32, b"10"),
                    (31, b"11.125"),
                    (14, b"10"),
                    (151, b"0"),
                ],
            );
        }
    }
}

#[test]
fn rejects_use_request_ids_and_keep_the_live_order() {
    for version in Version::ALL {
        let mut f = Fixture::start(
            Role::Acceptor,
            version,
            &config(Response::Reject, Response::Manual, Response::Manual),
        );
        request(&mut f.wire, b"D", 2, b"bad", b"", b"10", b"12");
        f.expect(
            b"8",
            2,
            &[
                (11, b"bad"),
                (150, b"8"),
                (39, b"8"),
                (103, b"0"),
                (58, b"Simulation reject"),
                (14, b"0"),
                (151, b"0"),
            ],
        );

        let mut f = Fixture::start(
            Role::Acceptor,
            version,
            &config(Response::Acknowledge, Response::Reject, Response::Reject),
        );
        request(&mut f.wire, b"D", 2, b"new", b"", b"10", b"12");
        let id = order_id(&f.expect(b"8", 2, &[(150, b"0")]));
        for (seq, kind, response) in [(3, b"G", b"2"), (4, b"F", b"1")] {
            let request_id = seq.to_string();
            request(
                &mut f.wire,
                kind,
                seq,
                request_id.as_bytes(),
                b"new",
                b"10",
                b"12",
            );
            f.expect(
                b"9",
                seq,
                &[
                    (37, &id),
                    (11, request_id.as_bytes()),
                    (41, b"new"),
                    (39, b"0"),
                    (102, b"2"),
                    (434, response),
                    (58, b"Simulation reject"),
                ],
            );
        }
    }
}

#[test]
fn automation_falls_back_on_unknown_reference_and_excess_partial_quantity() {
    let mut f = Fixture::start(
        Role::Acceptor,
        Version::Fix44,
        &config(
            Response::PartialFill,
            Response::Acknowledge,
            Response::Acknowledge,
        ),
    );
    request(&mut f.wire, b"D", 2, b"small", b"", b"2.5", b"12");
    request(&mut f.wire, b"F", 3, b"unknown", b"small", b"2.5", b"");
    f.silent(4, 2);
    f.expect_manual("2", "Partial quantity must be less");
    f.expect_manual("3", "no live automated order");
}

#[test]
fn rules_are_off_by_default_and_initiators_never_automate() {
    for (role, settings) in [
        (Role::Acceptor, Settings::default()),
        (
            Role::Initiator,
            config(
                Response::Acknowledge,
                Response::Acknowledge,
                Response::Acknowledge,
            ),
        ),
        (
            Role::Acceptor,
            Settings {
                enabled: false,
                ..config(
                    Response::Acknowledge,
                    Response::Acknowledge,
                    Response::Acknowledge,
                )
            },
        ),
        (
            Role::Acceptor,
            config(
                Response::Manual,
                Response::Acknowledge,
                Response::Acknowledge,
            ),
        ),
    ] {
        let mut f = Fixture::start(role, Version::Fix44, &settings);
        request(&mut f.wire, b"D", 2, b"manual", b"", b"10", b"12");
        f.silent(3, 2);
        f.expect_manual("2", "Received");
    }
}

#[test]
fn automation_ignores_discarded_sequences_duplicate_ids_and_resent_requests() {
    let mut f = Fixture::start(
        Role::Acceptor,
        Version::Fix44,
        &config(Response::Acknowledge, Response::Manual, Response::Manual),
    );
    request(&mut f.wire, b"D", 3, b"gap", b"", b"10", b"12");
    f.expect(b"2", 2, &[(7, b"2")]);
    request(&mut f.wire, b"D", 2, b"new", b"", b"10", b"12");
    f.expect(b"8", 3, &[(11, b"new")]);
    request(&mut f.wire, b"D", 3, b"new", b"", b"10", b"12");
    f.silent(4, 4);
    f.expect_manual("3", "already in use");
    f.wire.send(
        b"D",
        5,
        &[
            (43, b"Y"),
            (122, b"20200101-00:00:00.000"),
            (11, b"resent"),
            (55, b"DEMO"),
            (54, b"1"),
            (38, b"10"),
            (40, b"2"),
            (44, b"12"),
        ],
    );
    f.silent(6, 5);
    f.expect_manual("5", "Handle resent requests manually");
    f.wire.send(
        b"D",
        7,
        &[
            (97, b"Y"),
            (11, b"possible-resend"),
            (55, b"DEMO"),
            (54, b"1"),
            (38, b"10"),
            (40, b"2"),
            (44, b"12"),
        ],
    );
    f.silent(8, 6);
    f.expect_manual("7", "Handle resent requests manually");
}

#[test]
fn automation_drops_state_when_the_peer_disconnects() {
    let mut f = Fixture::start(
        Role::Acceptor,
        Version::Fix44,
        &config(
            Response::Acknowledge,
            Response::Acknowledge,
            Response::Acknowledge,
        ),
    );
    request(&mut f.wire, b"D", 2, b"old", b"", b"10", b"12");
    f.expect(b"8", 2, &[(11, b"old")]);
    f.wire.stream.shutdown(std::net::Shutdown::Both).unwrap();
    f.wire = connect(&f.sessions, &f.saved, None);
    handshake(&f.sessions, f.saved.id, &mut f.wire, 3, 3, false);
    request(&mut f.wire, b"F", 4, b"cancel", b"old", b"10", b"");
    f.silent(5, 4);
    f.expect_manual("4", "no live automated order");
}

#[test]
fn acknowledge_relinks_replace_and_cancels_only_the_active_id() {
    for version in Version::ALL {
        let mut f = Fixture::start(
            Role::Acceptor,
            version,
            &config(
                Response::Acknowledge,
                Response::Acknowledge,
                Response::Acknowledge,
            ),
        );
        request(&mut f.wire, b"D", 2, b"new", b"", b"20", b"10");
        let id = order_id(&f.expect(b"8", 2, &[(150, b"0")]));
        request(&mut f.wire, b"G", 3, b"replace", b"new", b"10", b"11");
        f.expect(
            b"8",
            3,
            &[
                (37, &id),
                (11, b"replace"),
                (41, b"new"),
                (150, b"5"),
                (151, b"10"),
            ],
        );
        request(&mut f.wire, b"F", 4, b"stale", b"new", b"10", b"");
        f.silent(5, 4);
        f.expect_manual("4", "no live automated order");
        request(&mut f.wire, b"F", 6, b"cancel", b"replace", b"10", b"");
        f.expect(
            b"8",
            5,
            &[
                (37, &id),
                (11, b"cancel"),
                (41, b"replace"),
                (150, b"4"),
                (14, b"0"),
                (151, b"0"),
            ],
        );
    }
}

#[test]
fn manual_execution_invalidates_automated_order_state() {
    let mut f = Fixture::start(
        Role::Acceptor,
        Version::Fix44,
        &config(
            Response::Acknowledge,
            Response::Manual,
            Response::Acknowledge,
        ),
    );
    request(&mut f.wire, b"D", 2, b"new", b"", b"10", b"12");
    let id = order_id(&f.expect(b"8", 2, &[(150, b"0")]));
    let tags = format!(
        "37={}\n11=new\n17=manual\n150=F\n39=1\n55=DEMO\n54=1\n38=10\n32=2\n31=12\n14=2\n151=8\n6=12\n60=20260101-00:00:00.000",
        std::str::from_utf8(&id).unwrap()
    );
    let manual = Application::tags(Version::Fix44, "8", &tags).unwrap();
    f.sessions.send(f.saved.id, Send::Tags(manual)).unwrap();
    f.expect(b"8", 3, &[(17, b"manual"), (14, b"2")]);
    request(&mut f.wire, b"F", 3, b"cancel", b"new", b"10", b"");
    f.silent(4, 4);
    f.expect_manual("3", "no live automated order");
}

#[test]
fn only_an_accepted_sequence_reset_clears_the_live_order() {
    let mut f = Fixture::start(
        Role::Acceptor,
        Version::Fix44,
        &config(
            Response::Acknowledge,
            Response::Acknowledge,
            Response::Acknowledge,
        ),
    );
    request(&mut f.wire, b"D", 2, b"new", b"", b"10", b"12");
    f.expect(b"8", 2, &[(11, b"new")]);
    f.wire.send(b"4", 3, &[(36, b"2")]);
    f.expect(b"3", 3, &[(45, b"3")]);
    request(&mut f.wire, b"G", 3, b"replace", b"new", b"10", b"11");
    f.expect(b"8", 4, &[(11, b"replace"), (150, b"5")]);

    f.wire.send(b"4", 4, &[(36, b"9")]);
    request(&mut f.wire, b"F", 9, b"cancel", b"replace", b"10", b"");
    f.silent(10, 5);
    f.expect_manual("9", "no live automated order");
}
