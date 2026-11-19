//! Scenario tests for the FIX session. Each test drives the public API with
//! concrete messages. Expected values come from the FIX 4.2 and 4.4 session
//! specification, not from a run of the code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use fix_codec::{Decoder, Encoder, UtcMillis, frame, write_utc_timestamp};
use fix_session::{
    Config, DisconnectReason, Disposition, Event, Role, SendError, SequenceNumbers, Session, State,
};

const T0: UtcMillis = UtcMillis(1_700_000_000_000);
const V42: &str = "FIX.4.2";
const V43: &str = "FIX.4.3";
const V44: &str = "FIX.4.4";
/// The default heartbeat interval of `Config::new`.
const HB: u64 = 30_000;
/// The default SendingTime tolerance of `Config::new`.
const TOLERANCE: u64 = 120_000;

/// One message that the session wrote, decoded for the assertions.
#[derive(Debug)]
struct Wire {
    begin: String,
    kind: String,
    fields: Vec<(u32, String)>,
}

impl Wire {
    fn get(&self, tag: u32) -> Option<&str> {
        self.fields
            .iter()
            .find(|(field_tag, _)| *field_tag == tag)
            .map(|(_, value)| value.as_str())
    }

    fn num(&self, tag: u32) -> Option<u64> {
        self.get(tag).and_then(|value| value.parse().ok())
    }

    fn has(&self, tag: u32) -> bool {
        self.get(tag).is_some()
    }
}

/// Takes all output of the session and decodes each message.
fn drain(session: &mut Session) -> Vec<Wire> {
    let mut bytes = Vec::new();
    session.take_output(&mut bytes);
    let mut decoder = Decoder::new(64);
    let mut wires = Vec::new();
    while !bytes.is_empty() {
        let len = frame(&bytes, 4096)
            .expect("the session writes valid frames")
            .expect("the session writes whole messages");
        let message = decoder.decode(&bytes[..len]).expect("valid message");
        wires.push(Wire {
            begin: String::from_utf8_lossy(message.begin_string()).into_owned(),
            kind: String::from_utf8_lossy(message.msg_type()).into_owned(),
            fields: message
                .fields()
                .iter()
                .map(|(tag, value)| (tag, String::from_utf8_lossy(value).into_owned()))
                .collect(),
        });
        bytes.drain(..len);
    }
    wires
}

fn events(session: &mut Session) -> Vec<Event> {
    session.take_events().collect()
}

/// Builds a message with the header that the specification requires.
fn build(
    begin: &str,
    (sender, target): (&str, &str),
    kind: &str,
    seq: u64,
    time: UtcMillis,
    extra: &[(u32, &str)],
) -> Vec<u8> {
    let mut encoder = Encoder::new(4096);
    let mut stamp = [0u8; 21];
    let mut body = encoder.start(kind.as_bytes());
    body.field(49, sender.as_bytes())
        .field(56, target.as_bytes())
        .field_u64(34, seq)
        .field(52, write_utc_timestamp(time, &mut stamp));
    for (tag, value) in extra {
        body.field(*tag, value.as_bytes());
    }
    body.finish(begin.as_bytes()).expect("fits").to_vec()
}

/// A message from the peer THEM to us (US) in FIX 4.4, sent at `now`.
fn from_peer(kind: &str, seq: u64, now: UtcMillis, extra: &[(u32, &str)]) -> Vec<u8> {
    build(V44, ("THEM", "US"), kind, seq, now, extra)
}

fn feed(session: &mut Session, now: UtcMillis, bytes: &[u8]) -> Disposition {
    let mut decoder = Decoder::new(64);
    let message = decoder.decode(bytes).expect("valid message");
    session.on_message(now, &message)
}

fn config(role: Role, version: &str) -> Config {
    Config::new(role, version.as_bytes(), b"US", b"THEM")
}

fn logon_reply(version: &str, seq: u64, heartbeat_s: &str) -> Vec<u8> {
    build(
        version,
        ("THEM", "US"),
        "A",
        seq,
        T0,
        &[(98, "0"), (108, heartbeat_s)],
    )
}

/// An initiator that has logged on at `T0`. Both sequence numbers are 2.
fn active_initiator(version: &str) -> Session {
    let mut session = Session::new(config(Role::Initiator, version), SequenceNumbers::default());
    session.on_connected(T0);
    feed(&mut session, T0, &logon_reply(version, 1, "30"));
    assert_eq!(session.state(), State::Active);
    drain(&mut session);
    events(&mut session);
    session
}

/// An initiator that has logged on and received heartbeats up to `last_seq`,
/// so that it expects `last_seq + 1`.
fn active_initiator_at(last_seq: u64) -> Session {
    let mut session = active_initiator(V44);
    for seq in 2..=last_seq {
        feed(&mut session, T0, &from_peer("0", seq, T0, &[]));
    }
    drain(&mut session);
    events(&mut session);
    assert_eq!(session.sequence_numbers().next_target, last_seq + 1);
    session
}

fn app_order(seq: u64, now: UtcMillis, id: &str) -> Vec<u8> {
    from_peer("D", seq, now, &[(11, id)])
}

fn kinds(wires: &[Wire]) -> Vec<&str> {
    wires.iter().map(|wire| wire.kind.as_str()).collect()
}

fn timestamp(time: UtcMillis) -> String {
    let mut stamp = [0u8; 21];
    String::from_utf8_lossy(write_utc_timestamp(time, &mut stamp)).into_owned()
}

// ---- Logon ----

#[test]
fn initiator_logon_has_required_fields() {
    let mut config = config(Role::Initiator, V44);
    config.username = Some(b"trader".to_vec());
    config.password = Some(b"secret".to_vec());
    let mut session = Session::new(config, SequenceNumbers::default());
    session.on_connected(T0);

    assert_eq!(session.state(), State::LogonSent);
    let wires = drain(&mut session);
    assert_eq!(wires.len(), 1);
    let logon = &wires[0];
    assert_eq!(logon.kind, "A");
    assert_eq!(logon.begin, V44);
    assert_eq!(logon.get(49), Some("US"));
    assert_eq!(logon.get(56), Some("THEM"));
    assert_eq!(logon.num(34), Some(1));
    assert_eq!(logon.get(52), Some(timestamp(T0).as_str()));
    assert_eq!(logon.get(98), Some("0"));
    assert_eq!(logon.get(108), Some("30"));
    assert_eq!(logon.get(553), Some("trader"));
    assert_eq!(logon.get(554), Some("secret"));
    assert!(!logon.has(141), "no reset without reset_on_logon");
    assert_eq!(session.sequence_numbers().next_sender, 2);
}

#[test]
fn logon_carries_credentials_only_in_fix_43_and_44() {
    for (version, carries) in [(V42, false), (V43, true), (V44, true)] {
        let mut config = config(Role::Initiator, version);
        config.username = Some(b"trader".to_vec());
        config.password = Some(b"secret".to_vec());
        let mut session = Session::new(config, SequenceNumbers::default());
        session.on_connected(T0);
        let logon = &drain(&mut session)[0];
        assert_eq!(logon.begin, version);
        assert_eq!(logon.has(553), carries, "{version} Username");
        assert_eq!(logon.has(554), carries, "{version} Password");
    }
}

#[test]
fn initiator_with_reset_on_logon_restarts_sequences_and_sets_141() {
    let mut config = config(Role::Initiator, V44);
    config.reset_on_logon = true;
    let mut session = Session::new(
        config,
        SequenceNumbers {
            next_sender: 7,
            next_target: 9,
        },
    );
    session.on_connected(T0);

    let logon = &drain(&mut session)[0];
    assert_eq!(logon.num(34), Some(1));
    assert_eq!(logon.get(141), Some("Y"));

    let reply = from_peer("A", 1, T0, &[(98, "0"), (108, "30"), (141, "Y")]);
    feed(&mut session, T0, &reply);
    assert_eq!(session.state(), State::Active);
    assert_eq!(
        session.sequence_numbers(),
        SequenceNumbers {
            next_sender: 2,
            next_target: 2
        }
    );
}

#[test]
fn logon_reply_activates_initiator() {
    let mut session = Session::new(config(Role::Initiator, V44), SequenceNumbers::default());
    session.on_connected(T0);
    drain(&mut session);
    events(&mut session);

    let disposition = feed(&mut session, T0, &logon_reply(V44, 1, "30"));

    assert_eq!(disposition, Disposition::Drop);
    assert_eq!(session.state(), State::Active);
    assert_eq!(events(&mut session), vec![Event::LoggedOn]);
    assert_eq!(
        session.sequence_numbers(),
        SequenceNumbers {
            next_sender: 2,
            next_target: 2
        }
    );
    assert!(drain(&mut session).is_empty(), "no reply to a Logon reply");
}

#[test]
fn acceptor_answers_logon_and_adopts_the_peer_heartbeat_interval() {
    let mut session = Session::new(config(Role::Acceptor, V44), SequenceNumbers::default());
    session.on_connected(T0);
    assert_eq!(session.state(), State::AwaitingLogon);
    assert!(drain(&mut session).is_empty(), "the acceptor waits");

    let logon = from_peer("A", 1, T0, &[(98, "0"), (108, "20")]);
    feed(&mut session, T0, &logon);

    assert_eq!(session.state(), State::Active);
    assert_eq!(events(&mut session), vec![Event::LoggedOn]);
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["A"]);
    assert_eq!(wires[0].num(34), Some(1));
    assert_eq!(wires[0].get(98), Some("0"));
    assert_eq!(wires[0].get(108), Some("20"));
    assert_eq!(
        session.sequence_numbers(),
        SequenceNumbers {
            next_sender: 2,
            next_target: 2
        }
    );

    // The interval is 20 s, not the configured 30 s.
    assert_eq!(session.next_deadline(), Some(UtcMillis(T0.0 + 20_000)));
    session.on_timer(UtcMillis(T0.0 + 19_999));
    assert!(drain(&mut session).is_empty());
    session.on_timer(UtcMillis(T0.0 + 20_000));
    assert_eq!(kinds(&drain(&mut session)), ["0"]);
}

#[test]
fn acceptor_with_reset_flag_restarts_both_sequences() {
    let mut session = Session::new(
        config(Role::Acceptor, V44),
        SequenceNumbers {
            next_sender: 10,
            next_target: 10,
        },
    );
    session.on_connected(T0);
    let logon = from_peer("A", 1, T0, &[(98, "0"), (108, "30"), (141, "Y")]);
    feed(&mut session, T0, &logon);

    let wires = drain(&mut session);
    assert_eq!(wires[0].kind, "A");
    assert_eq!(wires[0].num(34), Some(1));
    assert_eq!(wires[0].get(141), Some("Y"));
    assert_eq!(
        session.sequence_numbers(),
        SequenceNumbers {
            next_sender: 2,
            next_target: 2
        }
    );
}

#[test]
fn logon_with_a_lower_sequence_than_expected_is_refused() {
    let mut session = Session::new(
        config(Role::Acceptor, V44),
        SequenceNumbers {
            next_sender: 1,
            next_target: 10,
        },
    );
    session.on_connected(T0);
    feed(
        &mut session,
        T0,
        &from_peer("A", 3, T0, &[(98, "0"), (108, "30")]),
    );

    assert!(session.disconnect_requested());
    assert_eq!(
        events(&mut session),
        vec![Event::DisconnectRequested(DisconnectReason::SequenceTooLow)]
    );
    assert_eq!(kinds(&drain(&mut session)), ["5"]);
}

#[test]
fn each_fix_version_logs_on_as_initiator_and_acceptor() {
    for version in [V42, V43, V44] {
        let mut initiator =
            Session::new(config(Role::Initiator, version), SequenceNumbers::default());
        initiator.on_connected(T0);
        let logon = &drain(&mut initiator)[0];
        assert_eq!(logon.begin, version);
        feed(&mut initiator, T0, &logon_reply(version, 1, "30"));
        assert_eq!(initiator.state(), State::Active, "{version} initiator");

        let mut acceptor =
            Session::new(config(Role::Acceptor, version), SequenceNumbers::default());
        acceptor.on_connected(T0);
        let logon = build(
            version,
            ("THEM", "US"),
            "A",
            1,
            T0,
            &[(98, "0"), (108, "30")],
        );
        feed(&mut acceptor, T0, &logon);
        assert_eq!(acceptor.state(), State::Active, "{version} acceptor");
        assert_eq!(drain(&mut acceptor)[0].begin, version);
    }
}

#[test]
fn first_message_that_is_not_logon_disconnects_the_acceptor() {
    let mut session = Session::new(config(Role::Acceptor, V44), SequenceNumbers::default());
    session.on_connected(T0);

    let disposition = feed(&mut session, T0, &from_peer("0", 1, T0, &[]));

    assert_eq!(disposition, Disposition::Drop);
    assert!(session.disconnect_requested());
    assert_eq!(
        events(&mut session),
        vec![Event::DisconnectRequested(DisconnectReason::BadLogon)]
    );
}

#[test]
fn logout_in_reply_to_our_logon_disconnects() {
    let mut session = Session::new(config(Role::Initiator, V44), SequenceNumbers::default());
    session.on_connected(T0);

    let logout = from_peer("5", 1, T0, &[(58, "bad credentials")]);
    feed(&mut session, T0, &logout);

    assert!(session.disconnect_requested());
    assert_ne!(session.state(), State::Active);
}

#[test]
fn logon_times_out_when_the_peer_does_not_answer() {
    let mut session = Session::new(config(Role::Initiator, V44), SequenceNumbers::default());
    session.on_connected(T0);
    assert_eq!(session.next_deadline(), Some(UtcMillis(T0.0 + 10_000)));

    session.on_timer(UtcMillis(T0.0 + 9_999));
    assert!(!session.disconnect_requested());
    session.on_timer(UtcMillis(T0.0 + 10_000));
    assert!(session.disconnect_requested());
    assert_eq!(
        events(&mut session),
        vec![Event::DisconnectRequested(DisconnectReason::LogonTimeout)]
    );
}

#[test]
fn acceptor_gives_up_when_no_logon_arrives() {
    let mut session = Session::new(config(Role::Acceptor, V44), SequenceNumbers::default());
    session.on_connected(T0);
    session.on_timer(UtcMillis(T0.0 + 9_999));
    assert!(!session.disconnect_requested());
    session.on_timer(UtcMillis(T0.0 + 10_000));
    assert!(session.disconnect_requested());
}

// ---- Heartbeat and TestRequest ----

#[test]
fn heartbeat_is_sent_after_one_idle_interval() {
    let mut session = active_initiator(V44);
    assert_eq!(session.next_deadline(), Some(UtcMillis(T0.0 + HB)));

    session.on_timer(UtcMillis(T0.0 + HB - 1));
    assert!(drain(&mut session).is_empty());
    session.on_timer(UtcMillis(T0.0 + HB));

    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["0"]);
    assert_eq!(wires[0].num(34), Some(2));
    assert!(!wires[0].has(112), "an idle heartbeat has no TestReqID");
}

/// Runs the heartbeat at 30 s and the TestRequest at 36 s. Returns the
/// session and the TestReqID.
fn silent_peer_session() -> (Session, String) {
    let mut session = active_initiator(V44);
    session.on_timer(UtcMillis(T0.0 + HB));
    drain(&mut session);
    session.on_timer(UtcMillis(T0.0 + 35_999));
    assert!(
        drain(&mut session).is_empty(),
        "no TestRequest before 1.2 intervals of silence"
    );
    session.on_timer(UtcMillis(T0.0 + 36_000));
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["1"]);
    assert_eq!(wires[0].num(34), Some(3));
    let id = wires[0].get(112).expect("TestReqID").to_owned();
    (session, id)
}

#[test]
fn test_request_follows_silence_of_one_and_a_fifth_intervals() {
    let (session, id) = silent_peer_session();
    assert_ne!(id, "");
    assert!(!session.disconnect_requested());
}

#[test]
fn unanswered_test_request_disconnects_after_one_interval() {
    let (mut session, _) = silent_peer_session();

    session.on_timer(UtcMillis(T0.0 + 36_000 + HB - 1));
    assert!(!session.disconnect_requested());
    session.on_timer(UtcMillis(T0.0 + 36_000 + HB));

    assert!(session.disconnect_requested());
    assert_eq!(
        events(&mut session).last(),
        Some(&Event::DisconnectRequested(
            DisconnectReason::HeartbeatTimeout
        ))
    );
}

#[test]
fn heartbeat_with_the_matching_test_req_id_clears_the_test_request() {
    let (mut session, id) = silent_peer_session();

    let reply = from_peer("0", 2, UtcMillis(T0.0 + 37_000), &[(112, &id)]);
    assert_eq!(
        feed(&mut session, UtcMillis(T0.0 + 37_000), &reply),
        Disposition::Drop
    );
    session.on_timer(UtcMillis(T0.0 + 36_000 + HB));

    assert!(!session.disconnect_requested());
    assert_eq!(session.state(), State::Active);
}

#[test]
fn peer_test_request_gets_a_heartbeat_that_echoes_the_test_req_id() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);

    let request = from_peer("1", 2, now, &[(112, "PING-7")]);
    assert_eq!(feed(&mut session, now, &request), Disposition::Drop);

    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["0"]);
    assert_eq!(wires[0].get(112), Some("PING-7"));
    assert_eq!(wires[0].num(34), Some(2));
    assert_eq!(session.sequence_numbers().next_target, 3);
}

// ---- Gaps ----

#[test]
fn higher_sequence_asks_for_a_resend_and_drops_the_message() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);

    // The session expects 2, the peer sends 5.
    let disposition = feed(&mut session, now, &app_order(5, now, "LATE"));

    assert_eq!(disposition, Disposition::Drop);
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["2"]);
    assert_eq!(wires[0].num(7), Some(2));
    assert_eq!(wires[0].num(16), Some(0));
    assert_eq!(
        events(&mut session),
        vec![Event::GapDetected {
            begin: 2,
            received: 5
        }]
    );
    assert_eq!(session.sequence_numbers().next_target, 2);
}

#[test]
fn second_message_above_an_open_gap_does_not_send_another_resend_request() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);
    feed(&mut session, now, &app_order(5, now, "A"));
    drain(&mut session);

    let disposition = feed(&mut session, now, &app_order(6, now, "B"));

    assert_eq!(disposition, Disposition::Drop);
    assert!(drain(&mut session).is_empty());
    assert_eq!(session.sequence_numbers().next_target, 2);
}

#[test]
fn gap_fill_to_beyond_the_dropped_message_closes_the_gap() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);
    feed(&mut session, now, &app_order(5, now, "LATE"));
    drain(&mut session);
    events(&mut session);

    // The peer answers with one gap fill that covers 2..=5.
    let fill = from_peer(
        "4",
        2,
        now,
        &[(43, "Y"), (122, &timestamp(now)), (123, "Y"), (36, "6")],
    );
    assert_eq!(feed(&mut session, now, &fill), Disposition::Drop);

    assert_eq!(session.sequence_numbers().next_target, 6);
    let got = events(&mut session);
    assert!(got.contains(&Event::GapClosed), "events: {got:?}");
    assert_eq!(
        feed(&mut session, now, &app_order(6, now, "NEXT")),
        Disposition::Deliver
    );
    assert_eq!(session.sequence_numbers().next_target, 7);
}

#[test]
fn gap_stays_open_until_the_dropped_message_is_resent() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);
    feed(&mut session, now, &app_order(5, now, "LATE"));
    drain(&mut session);
    events(&mut session);

    // The peer fills 2..=4 and sends message 5 again with PossDupFlag=Y.
    let fill = from_peer(
        "4",
        2,
        now,
        &[(43, "Y"), (122, &timestamp(now)), (123, "Y"), (36, "5")],
    );
    feed(&mut session, now, &fill);
    assert_eq!(session.sequence_numbers().next_target, 5);
    assert!(!events(&mut session).contains(&Event::GapClosed));

    let resent = from_peer(
        "D",
        5,
        now,
        &[(43, "Y"), (122, &timestamp(now)), (11, "LATE")],
    );
    assert_eq!(feed(&mut session, now, &resent), Disposition::Deliver);
    assert_eq!(session.sequence_numbers().next_target, 6);
    assert!(events(&mut session).contains(&Event::GapClosed));
}

#[test]
fn peer_resend_request_gets_one_gap_fill_for_the_whole_range() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);
    for id in ["A", "B", "C"] {
        session
            .send_app(now, b"D", |body| {
                body.field(11, id.as_bytes());
            })
            .unwrap();
    }
    drain(&mut session);
    assert_eq!(session.sequence_numbers().next_sender, 5);

    let request = from_peer("2", 2, now, &[(7, "2"), (16, "0")]);
    assert_eq!(feed(&mut session, now, &request), Disposition::Drop);

    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["4"], "one message, no old messages");
    let fill = &wires[0];
    assert_eq!(fill.num(34), Some(2), "MsgSeqNum is BeginSeqNo");
    assert_eq!(fill.get(43), Some("Y"));
    assert!(fill.has(122), "OrigSendingTime");
    assert_eq!(fill.get(123), Some("Y"));
    assert_eq!(fill.num(36), Some(5), "NewSeqNo is our next sequence");
    assert_eq!(session.sequence_numbers().next_sender, 5);
    assert_eq!(session.sequence_numbers().next_target, 3);
    assert_eq!(
        events(&mut session),
        vec![Event::ResendRequested {
            begin: 2,
            end: 0,
            new_seq: 5
        }]
    );
    // The next message continues at 5.
    assert_eq!(session.send_app(now, b"D", |_| {}), Ok(5));
}

#[test]
fn resend_request_from_the_middle_of_the_range_starts_the_gap_fill_there() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);
    for _ in 0..3 {
        session.send_app(now, b"D", |_| {}).unwrap();
    }
    drain(&mut session);

    feed(
        &mut session,
        now,
        &from_peer("2", 2, now, &[(7, "3"), (16, "4")]),
    );

    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["4"]);
    assert_eq!(wires[0].num(34), Some(3));
    assert_eq!(wires[0].num(36), Some(5));
}

// ---- SequenceReset ----

#[test]
fn sequence_reset_without_gap_fill_flag_ignores_msg_seq_num_and_moves_expected_up() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);

    // MsgSeqNum 99 would be a gap for any other message.
    let reset = from_peer("4", 99, now, &[(36, "10")]);
    assert_eq!(feed(&mut session, now, &reset), Disposition::Drop);

    assert_eq!(session.sequence_numbers().next_target, 10);
    assert_eq!(
        events(&mut session),
        vec![Event::SequenceReset { new_seq: 10 }]
    );
    assert!(drain(&mut session).is_empty(), "no ResendRequest");
    assert_eq!(
        feed(&mut session, now, &app_order(10, now, "NEXT")),
        Disposition::Deliver
    );
}

#[test]
fn sequence_reset_to_a_lower_number_is_rejected_with_value_incorrect() {
    let mut session = active_initiator_at(4);
    let now = UtcMillis(T0.0 + 1_000);

    feed(&mut session, now, &from_peer("4", 7, now, &[(36, "3")]));

    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["3"]);
    assert_eq!(wires[0].num(373), Some(5));
    assert_eq!(wires[0].num(45), Some(7));
    assert_eq!(
        session.sequence_numbers().next_target,
        5,
        "never moves back"
    );
    assert!(!session.disconnect_requested());
}

// ---- Sequence too low ----

#[test]
fn sequence_too_low_without_poss_dup_logs_out_with_text_and_disconnects() {
    let mut session = active_initiator_at(4);
    let now = UtcMillis(T0.0 + 1_000);

    let disposition = feed(&mut session, now, &app_order(2, now, "OLD"));

    assert_eq!(disposition, Disposition::Drop);
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["5"]);
    assert!(
        wires[0].get(58).is_some_and(|text| !text.is_empty()),
        "Logout carries Text"
    );
    assert!(session.disconnect_requested());
    assert_eq!(
        events(&mut session),
        vec![Event::DisconnectRequested(DisconnectReason::SequenceTooLow)]
    );
}

#[test]
fn sequence_too_low_with_poss_dup_is_dropped_silently() {
    let mut session = active_initiator_at(4);
    let now = UtcMillis(T0.0 + 1_000);

    let old = from_peer(
        "D",
        2,
        now,
        &[(43, "Y"), (122, &timestamp(now)), (11, "OLD")],
    );
    assert_eq!(feed(&mut session, now, &old), Disposition::Drop);

    assert!(drain(&mut session).is_empty());
    assert_eq!(events(&mut session), Vec::<Event>::new());
    assert_eq!(session.state(), State::Active);
    assert_eq!(session.sequence_numbers().next_target, 5);
}

// ---- Header checks ----

#[test]
fn wrong_comp_id_is_rejected_with_reason_9_then_logout_and_disconnect() {
    let cases = [("EVIL", "US"), ("THEM", "OTHER")];
    for ids in cases {
        let mut session = active_initiator(V44);
        let now = UtcMillis(T0.0 + 1_000);

        let message = build(V44, ids, "D", 2, now, &[(11, "X")]);
        assert_eq!(feed(&mut session, now, &message), Disposition::Drop);

        let wires = drain(&mut session);
        assert_eq!(kinds(&wires), ["3", "5"], "ids {ids:?}");
        assert_eq!(wires[0].num(373), Some(9));
        assert_eq!(wires[0].num(45), Some(2));
        assert!(session.disconnect_requested());
        assert_eq!(
            events(&mut session).last(),
            Some(&Event::DisconnectRequested(DisconnectReason::BadHeader))
        );
    }
}

#[test]
fn sending_time_beyond_the_tolerance_is_rejected_with_reason_10_then_logout() {
    for offset in [-1_i64, 1] {
        let mut session = active_initiator(V44);
        let now = UtcMillis(T0.0 + 1_000);
        let off = i64::try_from(TOLERANCE).unwrap() + 1;
        let time = UtcMillis(now.0.checked_add_signed(offset * off).unwrap());

        let message = app_order(2, time, "X");
        assert_eq!(feed(&mut session, now, &message), Disposition::Drop);

        let wires = drain(&mut session);
        assert_eq!(kinds(&wires), ["3", "5"], "offset {offset}");
        assert_eq!(wires[0].num(373), Some(10));
        assert_eq!(wires[0].num(45), Some(2));
        assert!(session.disconnect_requested());
    }
}

#[test]
fn sending_time_exactly_at_the_tolerance_is_accepted() {
    for offset in [-1_i64, 1] {
        let mut session = active_initiator(V44);
        let now = UtcMillis(T0.0 + 1_000);
        let off = i64::try_from(TOLERANCE).unwrap();
        let time = UtcMillis(now.0.checked_add_signed(offset * off).unwrap());

        let message = app_order(2, time, "X");
        assert_eq!(feed(&mut session, now, &message), Disposition::Deliver);
        assert!(!session.disconnect_requested());
    }
}

#[test]
fn wrong_begin_string_gets_a_logout_and_a_disconnect() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);

    let message = build(V42, ("THEM", "US"), "0", 2, now, &[]);
    assert_eq!(feed(&mut session, now, &message), Disposition::Drop);

    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["5"]);
    assert_eq!(wires[0].begin, V44);
    assert!(session.disconnect_requested());
    assert_eq!(
        events(&mut session),
        vec![Event::DisconnectRequested(DisconnectReason::BadHeader)]
    );
}

// ---- Logout ----

#[test]
fn logout_started_by_us_waits_for_the_reply_then_disconnects() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);

    session.logout(now, b"done for today");

    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["5"]);
    assert_eq!(wires[0].get(58), Some("done for today"));
    assert_eq!(wires[0].num(34), Some(2));
    assert_eq!(session.state(), State::LogoutSent);
    assert!(!session.disconnect_requested());

    let reply = from_peer("5", 2, now, &[]);
    assert_eq!(feed(&mut session, now, &reply), Disposition::Drop);

    assert!(session.disconnect_requested());
    assert!(drain(&mut session).is_empty(), "no second Logout");
    assert_eq!(
        events(&mut session),
        vec![Event::DisconnectRequested(DisconnectReason::LoggedOut)]
    );
}

#[test]
fn logout_started_by_the_peer_is_answered_and_disconnects() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);

    let disposition = feed(&mut session, now, &from_peer("5", 2, now, &[(58, "bye")]));

    assert_eq!(disposition, Disposition::Drop);
    assert_eq!(kinds(&drain(&mut session)), ["5"]);
    assert!(session.disconnect_requested());
    assert_eq!(
        events(&mut session),
        vec![Event::DisconnectRequested(DisconnectReason::LoggedOut)]
    );
}

#[test]
fn logout_times_out_when_the_peer_does_not_reply() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);
    session.logout(now, b"bye");
    assert_eq!(session.next_deadline(), Some(UtcMillis(now.0 + 10_000)));
    events(&mut session);

    session.on_timer(UtcMillis(now.0 + 9_999));
    assert!(!session.disconnect_requested());
    session.on_timer(UtcMillis(now.0 + 10_000));

    assert!(session.disconnect_requested());
    assert_eq!(
        events(&mut session),
        vec![Event::DisconnectRequested(DisconnectReason::LogoutTimeout)]
    );
}

// ---- Application messages ----

#[test]
fn send_app_before_logon_is_refused_and_writes_nothing() {
    let mut initiator = Session::new(config(Role::Initiator, V44), SequenceNumbers::default());
    let mut acceptor = Session::new(config(Role::Acceptor, V44), SequenceNumbers::default());

    assert_eq!(
        initiator.send_app(T0, b"D", |_| {}),
        Err(SendError::NotActive)
    );
    initiator.on_connected(T0);
    drain(&mut initiator);
    assert_eq!(
        initiator.send_app(T0, b"D", |_| {}),
        Err(SendError::NotActive)
    );
    assert!(drain(&mut initiator).is_empty());
    assert_eq!(initiator.sequence_numbers().next_sender, 2, "Logon only");

    acceptor.on_connected(T0);
    assert_eq!(
        acceptor.send_app(T0, b"D", |_| {}),
        Err(SendError::NotActive)
    );
}

#[test]
fn send_app_writes_the_header_the_body_and_advances_the_sequence() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);

    let first = session.send_app(now, b"D", |body| {
        body.field(11, b"ORD-1").field(55, b"IBM");
    });
    let second = session.send_app(now, b"D", |body| {
        body.field(11, b"ORD-2");
    });

    assert_eq!((first, second), (Ok(2), Ok(3)));
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["D", "D"]);
    assert_eq!(wires[0].num(34), Some(2));
    assert_eq!(wires[0].get(49), Some("US"));
    assert_eq!(wires[0].get(56), Some("THEM"));
    assert_eq!(wires[0].get(52), Some(timestamp(now).as_str()));
    assert_eq!(wires[0].get(11), Some("ORD-1"));
    assert_eq!(wires[0].get(55), Some("IBM"));
    assert_eq!(wires[1].num(34), Some(3));
    assert_eq!(wires[1].get(11), Some("ORD-2"));
    assert_eq!(session.sequence_numbers().next_sender, 4);
}

#[test]
fn app_messages_are_delivered_in_order_and_advance_the_expected_sequence() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);

    assert_eq!(
        feed(&mut session, now, &app_order(2, now, "A")),
        Disposition::Deliver
    );
    assert_eq!(session.sequence_numbers().next_target, 3);
    assert_eq!(
        feed(&mut session, now, &app_order(3, now, "B")),
        Disposition::Deliver
    );
    assert_eq!(session.sequence_numbers().next_target, 4);
    assert!(drain(&mut session).is_empty(), "no reply to app messages");
    assert_eq!(events(&mut session), Vec::<Event>::new());
}

#[test]
fn session_level_reject_from_the_peer_is_delivered_to_the_application() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);

    let reject = from_peer(
        "3",
        2,
        now,
        &[(45, "1"), (372, "D"), (373, "5"), (58, "bad value")],
    );

    assert_eq!(feed(&mut session, now, &reject), Disposition::Deliver);
    assert_eq!(session.sequence_numbers().next_target, 3);
}

#[test]
fn session_messages_other_than_reject_are_not_delivered() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);
    assert_eq!(
        feed(&mut session, now, &from_peer("0", 2, now, &[])),
        Disposition::Drop
    );
    assert_eq!(session.sequence_numbers().next_target, 3);
}

#[test]
fn disconnect_and_reconnect_keep_the_sequence_numbers() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);
    session.send_app(now, b"D", |_| {}).unwrap();
    session.on_disconnected(now);
    assert_eq!(session.state(), State::Disconnected);

    session.on_connected(UtcMillis(now.0 + 5_000));

    let logon = &drain(&mut session)[0];
    assert_eq!(logon.kind, "A");
    assert_eq!(logon.num(34), Some(3), "after Logon 1 and order 2");
    assert!(!logon.has(141));
}

// ---- Known bugs ----

/// A ResendRequest that never reached the peer must be sent again. On TCP the
/// loss happens when a host drops a message, for example in a full send queue.
/// The session keeps `gap_until` set, so it never sends a second
/// ResendRequest, and it drops every later message as "above the gap".
#[test]
fn lost_resend_request_is_asked_again_within_a_few_intervals() {
    let mut session = active_initiator(V44);
    let now = UtcMillis(T0.0 + 1_000);
    // The peer sent 2..=4 (all lost) and 5. The session asks for a resend.
    feed(&mut session, now, &app_order(5, now, "A"));
    assert_eq!(kinds(&drain(&mut session)), ["2"]);
    // The host loses that ResendRequest. The peer stays alive and sends a
    // Heartbeat each interval. The session never gets a gap fill.
    let mut asked_again = false;
    for round in 1..=3_u64 {
        let later = UtcMillis(now.0 + round * HB);
        feed(&mut session, later, &from_peer("0", 5 + round, later, &[]));
        session.on_timer(later);
        asked_again |= drain(&mut session)
            .iter()
            .any(|wire| wire.kind == "2" && wire.num(7) == Some(2));
    }
    assert!(asked_again, "no ResendRequest after 3 intervals");
}

// Regression tests for the PR 2 review findings.

#[test]
fn logon_with_a_huge_heart_bt_int_is_refused_without_overflow() {
    let mut session = Session::new(config(Role::Acceptor, V44), SequenceNumbers::default());
    session.on_connected(T0);
    let logon = from_peer("A", 1, T0, &[(98, "0"), (108, "9999999999999999999")]);
    feed(&mut session, T0, &logon);
    assert_eq!(
        events(&mut session),
        vec![Event::DisconnectRequested(DisconnectReason::BadLogon)]
    );
    assert_eq!(kinds(&drain(&mut session)), ["5"]);
}

#[test]
fn logon_without_encrypt_method_or_with_a_stale_sending_time_is_refused() {
    let stale = UtcMillis(T0.0 - TOLERANCE - 1);
    for logon in [
        from_peer("A", 1, T0, &[(108, "30")]),
        from_peer("A", 1, stale, &[(98, "0"), (108, "30")]),
    ] {
        let mut session = Session::new(config(Role::Acceptor, V44), SequenceNumbers::default());
        session.on_connected(T0);
        feed(&mut session, T0, &logon);
        assert_ne!(session.state(), State::Active);
        assert_eq!(
            events(&mut session),
            vec![Event::DisconnectRequested(DisconnectReason::BadLogon)]
        );
    }
}

#[test]
fn gap_fill_for_a_finite_range_stops_after_end_seq_no() {
    let mut session = active_initiator(V44);
    for id in 0..8 {
        let sent = session.send_app(T0, b"D", |body| {
            body.field_u64(11, id);
        });
        assert!(sent.is_ok());
    }
    assert_eq!(session.sequence_numbers().next_sender, 10);
    drain(&mut session);
    feed(
        &mut session,
        T0,
        &from_peer("2", 2, T0, &[(7, "3"), (16, "4")]),
    );
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["4"]);
    assert_eq!(wires[0].num(34), Some(3));
    assert_eq!(wires[0].num(36), Some(5), "messages 5 to 9 are not skipped");
    assert_eq!(session.sequence_numbers().next_sender, 10);
}

#[test]
fn resend_request_with_end_below_begin_is_rejected() {
    let mut session = active_initiator_at(2);
    session
        .send_app(T0, b"D", |body| {
            body.field(11, b"X");
        })
        .ok();
    drain(&mut session);
    feed(
        &mut session,
        T0,
        &from_peer("2", 3, T0, &[(7, "3"), (16, "2")]),
    );
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["3"]);
    assert_eq!(wires[0].num(373), Some(5));
}

#[test]
fn sequence_reset_with_an_invalid_gap_fill_flag_is_rejected_not_applied() {
    let mut session = active_initiator(V44);
    let reset = from_peer("4", 9, T0, &[(123, "X"), (36, "50")]);
    feed(&mut session, T0, &reset);
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["3"]);
    assert_eq!(wires[0].num(373), Some(5));
    assert_eq!(
        session.sequence_numbers().next_target,
        2,
        "the reset is not applied"
    );
}

#[test]
fn poss_dup_without_orig_sending_time_is_rejected_not_delivered() {
    let mut session = active_initiator(V44);
    let order = from_peer("D", 2, T0, &[(43, "Y"), (11, "A")]);
    assert_eq!(feed(&mut session, T0, &order), Disposition::Drop);
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["3"]);
    assert_eq!(wires[0].num(373), Some(1));
    assert_eq!(session.sequence_numbers().next_target, 3);
}

#[test]
fn poss_dup_with_orig_sending_time_after_sending_time_is_rejected() {
    let mut session = active_initiator(V44);
    let later = timestamp(UtcMillis(T0.0 + 1000));
    let order = from_peer("D", 2, T0, &[(43, "Y"), (122, &later), (11, "A")]);
    assert_eq!(feed(&mut session, T0, &order), Disposition::Drop);
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["3", "5"]);
    assert_eq!(wires[0].num(373), Some(10));
    assert!(session.disconnect_requested());
    assert_eq!(
        session.sequence_numbers().next_target,
        3,
        "the rejected message used seq 2"
    );
}

#[test]
fn poss_dup_with_valid_orig_sending_time_is_delivered() {
    let mut session = active_initiator(V44);
    let earlier = timestamp(UtcMillis(T0.0 - 600_000));
    let order = from_peer("D", 2, T0, &[(43, "Y"), (122, &earlier), (11, "A")]);
    assert_eq!(feed(&mut session, T0, &order), Disposition::Deliver);
}

#[test]
fn a_burst_of_rejects_does_not_panic_and_counts_lost_events() {
    let mut session = active_initiator(V44);
    for seq in 2..=66 {
        feed(&mut session, T0, &from_peer("1", seq, T0, &[]));
    }
    assert_eq!(session.events_lost(), 1, "65 events, 64 fit in the queue");
    assert_eq!(events(&mut session).len(), 64);
    assert_eq!(session.sequence_numbers().next_target, 67);
}

#[test]
fn send_app_refuses_a_message_that_is_too_large() {
    let mut session = active_initiator(V44);
    let before = session.sequence_numbers();
    let text = vec![b'x'; 20_000];
    let sent = session.send_app(T0, b"D", |body| {
        body.field(58, &text);
    });
    assert_eq!(sent, Err(SendError::TooLarge));
    assert_eq!(session.sequence_numbers(), before);
    assert!(drain(&mut session).is_empty());
}

#[test]
fn send_app_refuses_when_the_host_does_not_take_output() {
    let mut cfg = config(Role::Initiator, V44);
    cfg.message_len_max = 512;
    cfg.output_len_max = 2048;
    let mut session = Session::new(cfg, SequenceNumbers::default());
    session.on_connected(T0);
    feed(&mut session, T0, &logon_reply(V44, 1, "30"));
    let mut results = Vec::new();
    for id in 0..20u64 {
        results.push(session.send_app(T0, b"D", |body| {
            body.field_u64(11, id);
        }));
    }
    assert!(results.contains(&Err(SendError::OutputFull)));
    let accepted = results.iter().filter(|result| result.is_ok()).count() as u64;
    assert_eq!(session.sequence_numbers().next_sender, 2 + accepted);
}

// Regression tests for the PR 2 round 2 findings.

fn frames_fit(wires_bytes: &[u8], len_max: usize) -> bool {
    let mut rest = wires_bytes;
    while !rest.is_empty() {
        match frame(rest, len_max) {
            Ok(Some(len)) => rest = &rest[len..],
            _ => return false,
        }
    }
    true
}

#[test]
fn a_session_message_above_the_limit_is_not_sent() {
    let mut cfg = config(Role::Initiator, V44);
    cfg.message_len_max = 512;
    cfg.output_len_max = 2048;
    let mut session = Session::new(cfg, SequenceNumbers::default());
    session.on_connected(T0);
    feed(&mut session, T0, &logon_reply(V44, 1, "30"));
    drain(&mut session);
    events(&mut session);
    let long_id = "x".repeat(600);
    feed(&mut session, T0, &from_peer("1", 2, T0, &[(112, &long_id)]));
    let mut bytes = Vec::new();
    session.take_output(&mut bytes);
    assert!(
        frames_fit(&bytes, 512),
        "no frame is longer than message_len_max"
    );
    assert_eq!(
        events(&mut session),
        vec![Event::DisconnectRequested(
            DisconnectReason::MessageTooLarge
        )]
    );
}

#[test]
fn acceptor_that_cannot_send_its_logon_reply_does_not_become_active() {
    let mut cfg = config(Role::Acceptor, V44);
    cfg.message_len_max = 512;
    cfg.output_len_max = 2048;
    cfg.username = Some(vec![b'u'; 600]);
    let mut session = Session::new(cfg, SequenceNumbers::default());
    session.on_connected(T0);
    feed(
        &mut session,
        T0,
        &from_peer("A", 1, T0, &[(98, "0"), (108, "30")]),
    );
    assert_eq!(session.state(), State::Disconnecting);
    let all = events(&mut session);
    assert!(!all.contains(&Event::LoggedOn));
    assert!(drain(&mut session).is_empty());
}

#[test]
fn logout_that_cannot_be_sent_disconnects_instead_of_waiting() {
    let mut cfg = config(Role::Initiator, V44);
    cfg.message_len_max = 512;
    cfg.output_len_max = 2048;
    let mut session = Session::new(cfg, SequenceNumbers::default());
    session.on_connected(T0);
    feed(&mut session, T0, &logon_reply(V44, 1, "30"));
    while session
        .send_app(T0, b"D", |body| {
            body.field(11, b"A");
        })
        .is_ok()
    {}
    session.logout(T0, b"bye");
    assert_eq!(session.state(), State::Disconnecting);
}

#[test]
fn poss_dup_with_a_stale_sending_time_is_rejected() {
    let mut session = active_initiator(V44);
    let stale = UtcMillis(T0.0 - TOLERANCE - 60_000);
    let orig = timestamp(UtcMillis(stale.0 - 1000));
    let order = from_peer("D", 2, stale, &[(43, "Y"), (122, &orig), (11, "A")]);
    assert_eq!(feed(&mut session, T0, &order), Disposition::Drop);
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["3", "5"]);
    assert_eq!(wires[0].num(373), Some(10));
}

#[test]
fn resend_request_with_an_end_seq_no_that_is_not_a_number_is_rejected() {
    let mut session = active_initiator(V44);
    for id in 0..8 {
        assert!(
            session
                .send_app(T0, b"D", |body| {
                    body.field_u64(11, id);
                })
                .is_ok()
        );
    }
    drain(&mut session);
    feed(
        &mut session,
        T0,
        &from_peer("2", 2, T0, &[(7, "3"), (16, "abc")]),
    );
    let wires = drain(&mut session);
    assert_eq!(kinds(&wires), ["3"], "no gap fill through the last message");
    assert_eq!(wires[0].num(373), Some(5));
}
