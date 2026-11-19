use std::io::Write as _;

use fix_codec::tag::{self, msg_type};
use fix_codec::{
    Body, Encoder, Fields, Message, UtcMillis, parse_utc_timestamp, write_utc_timestamp,
};

use crate::config::{Config, Role};

/// The largest number of events that the host can leave in the queue. The
/// host takes the events after each call, so the queue stays short.
const EVENTS_MAX: usize = 64;
/// The largest HeartBtInt that a peer Logon can set, in seconds.
const HEART_BT_INT_MAX_S: u64 = 3600;

// SessionRejectReason(373) values.
const REJECT_REQUIRED_TAG_MISSING: u64 = 1;
const REJECT_VALUE_INCORRECT: u64 = 5;
const REJECT_COMP_ID_PROBLEM: u64 = 9;
const REJECT_SENDING_TIME_ACCURACY: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No connection.
    Disconnected,
    /// Initiator: Logon sent, waiting for the Logon reply.
    LogonSent,
    /// Acceptor: connected, waiting for the Logon of the initiator.
    AwaitingLogon,
    /// Logged on. Application messages can flow.
    Active,
    /// Logout sent, waiting for the Logout reply.
    LogoutSent,
    /// The session asked the host to close the connection.
    Disconnecting,
}

/// What the host does with a received message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Give it to the application: an application message, or a Reject.
    Deliver,
    /// The session handled it, or it must not be used.
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectReason {
    /// A Logout exchange finished.
    LoggedOut,
    LogonTimeout,
    LogoutTimeout,
    /// No message after a TestRequest.
    HeartbeatTimeout,
    /// The first message was not a Logon, or the Logon was not valid.
    BadLogon,
    /// MsgSeqNum was lower than expected with no PossDupFlag.
    SequenceTooLow,
    /// BeginString, CompID, MsgSeqNum, or SendingTime was not valid.
    BadHeader,
    /// The host did not take the output, and it reached its limit.
    OutputFull,
    /// A session message did not fit in `Config::message_len_max`.
    MessageTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    LoggedOn,
    /// The peer sent a message with a higher MsgSeqNum than expected. The
    /// session sent a ResendRequest from `begin`.
    GapDetected {
        begin: u64,
        received: u64,
    },
    /// All messages up to the gap have arrived or were gap-filled.
    GapClosed,
    /// The peer asked for a resend. The session sent a gap fill to `new_seq`.
    ResendRequested {
        begin: u64,
        end: u64,
        new_seq: u64,
    },
    /// A SequenceReset changed the next expected MsgSeqNum.
    SequenceReset {
        new_seq: u64,
    },
    /// The session sent a Reject for the message `ref_seq`.
    RejectSent {
        ref_seq: u64,
        reason: u64,
    },
    DisconnectRequested(DisconnectReason),
}

/// A problem in the standard header of a received message.
#[derive(Debug, Clone, Copy)]
enum HeaderProblem {
    CompId,
    MissingTag(&'static [u8]),
    BadValue(&'static [u8]),
    SendingTimeAccuracy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// Application messages can be sent only when the session is active.
    NotActive,
    /// The host has not taken enough output: one more message could pass
    /// `Config::output_len_max`.
    OutputFull,
    /// The message is longer than `Config::message_len_max`. Nothing was sent.
    TooLarge,
}

/// The next MsgSeqNum to send and the next one expected from the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceNumbers {
    pub next_sender: u64,
    pub next_target: u64,
}

impl Default for SequenceNumbers {
    fn default() -> Self {
        Self {
            next_sender: 1,
            next_target: 1,
        }
    }
}

/// Time for one session event. The host supplies both clocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionTime {
    /// Milliseconds from a fixed monotonic origin.
    pub elapsed_ms: u64,
    /// UTC for FIX timestamps and SendingTime checks.
    pub utc: UtcMillis,
}

impl SessionTime {
    fn legacy(utc: UtcMillis) -> Self {
        Self {
            elapsed_ms: utc.0,
            utc,
        }
    }
}

#[derive(Debug)]
pub struct Session {
    config: Config,
    state: State,
    seq: SequenceNumbers,
    /// Heartbeat interval in use: from the config, or from the peer Logon.
    heartbeat_ms: u64,
    /// When the state started: for the Logon and Logout timeouts.
    state_since: u64,
    last_sent: u64,
    last_received: u64,
    /// The TestReqID and time of a TestRequest that has no reply yet.
    test_request: Option<(u64, u64)>,
    /// The highest MsgSeqNum seen while a ResendRequest is open.
    gap_until: Option<u64>,
    /// When the last ResendRequest was sent. A gap that stays open for one
    /// heartbeat interval gets a new ResendRequest, in case one was lost.
    resend_sent: u64,
    /// The initiator sent ResetSeqNumFlag=Y and waits for the reply.
    reset_requested: bool,
    encoder: Encoder,
    output: Vec<u8>,
    events: Vec<Event>,
    events_lost: u64,
    text: Vec<u8>,
}

impl Session {
    pub fn new(config: Config, seq: SequenceNumbers) -> Self {
        assert!(
            seq.next_sender >= 1 && seq.next_target >= 1,
            "MsgSeqNum starts at 1"
        );
        assert!(
            config.heartbeat_interval_ms >= 1000,
            "HeartBtInt is in whole seconds"
        );
        let encoder = Encoder::new(config.message_len_max);
        let output = Vec::with_capacity(config.output_len_max);
        Self {
            heartbeat_ms: config.heartbeat_interval_ms,
            config,
            state: State::Disconnected,
            seq,
            state_since: 0,
            last_sent: 0,
            last_received: 0,
            test_request: None,
            gap_until: None,
            resend_sent: 0,
            reset_requested: false,
            encoder,
            output,
            events: Vec::with_capacity(EVENTS_MAX),
            events_lost: 0,
            text: Vec::with_capacity(128),
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn sequence_numbers(&self) -> SequenceNumbers {
        self.seq
    }

    /// The number of events dropped because the queue was full. The host
    /// takes the events after each call, so this stays 0.
    pub fn events_lost(&self) -> u64 {
        self.events_lost
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn disconnect_requested(&self) -> bool {
        self.state == State::Disconnecting
    }

    /// Appends the bytes to send to `sink` and clears the output.
    pub fn take_output(&mut self, sink: &mut Vec<u8>) {
        sink.extend_from_slice(&self.output);
        self.output.clear();
    }

    /// Takes the events since the last call, in order.
    pub fn take_events(&mut self) -> std::vec::Drain<'_, Event> {
        self.events.drain(..)
    }

    /// The connection is open. The initiator sends Logon.
    pub fn on_connected(&mut self, now: UtcMillis) {
        self.on_connected_at(SessionTime::legacy(now));
    }

    pub fn on_connected_at(&mut self, now: SessionTime) {
        assert_eq!(
            self.state,
            State::Disconnected,
            "on_connected without on_disconnected"
        );
        self.last_received = now.elapsed_ms;
        self.test_request = None;
        self.gap_until = None;
        match self.config.role {
            Role::Initiator => {
                self.reset_requested = self.config.reset_on_logon;
                if self.reset_requested {
                    self.seq = SequenceNumbers::default();
                }
                if self.send_logon(now, self.reset_requested) {
                    self.enter(State::LogonSent, now);
                }
            }
            Role::Acceptor => self.enter(State::AwaitingLogon, now),
        }
    }

    /// The connection is closed.
    pub fn on_disconnected(&mut self, now: UtcMillis) {
        self.on_disconnected_at(SessionTime::legacy(now));
    }

    pub fn on_disconnected_at(&mut self, now: SessionTime) {
        self.output.clear();
        self.test_request = None;
        self.gap_until = None;
        self.enter(State::Disconnected, now);
    }

    /// Starts a Logout. The session waits for the reply, then asks the host to
    /// disconnect.
    pub fn logout(&mut self, now: UtcMillis, text: &[u8]) {
        self.logout_at(SessionTime::legacy(now), text);
    }

    pub fn logout_at(&mut self, now: SessionTime, text: &[u8]) {
        if self.state != State::Active {
            return;
        }
        if self.send_logout(now, text) {
            self.enter(State::LogoutSent, now);
        }
    }

    /// Sends an application message. `fill` adds the body fields after the
    /// header. Returns the MsgSeqNum of the message.
    pub fn send_app(
        &mut self,
        now: UtcMillis,
        msg_type: &[u8],
        fill: impl FnOnce(&mut Body),
    ) -> Result<u64, SendError> {
        self.send_app_at(SessionTime::legacy(now), msg_type, fill)
    }

    pub fn send_app_at(
        &mut self,
        now: SessionTime,
        msg_type: &[u8],
        fill: impl FnOnce(&mut Body),
    ) -> Result<u64, SendError> {
        assert!(
            !msg_type::is_admin(msg_type),
            "session messages are sent by the session"
        );
        if self.state != State::Active {
            return Err(SendError::NotActive);
        }
        let seq = self.seq.next_sender;
        self.write_message(now, msg_type, seq, fill)?;
        self.seq.next_sender += 1;
        Ok(seq)
    }

    /// The time when [`Session::on_timer`] must run next, or `None` when no
    /// timer is due.
    pub fn next_deadline(&self) -> Option<UtcMillis> {
        self.next_deadline_ms().map(UtcMillis)
    }

    /// Returns the deadline in milliseconds from the host's monotonic origin.
    pub fn next_deadline_ms(&self) -> Option<u64> {
        let after = u64::saturating_add;
        match self.state {
            State::Disconnected | State::Disconnecting => None,
            State::LogonSent | State::AwaitingLogon => {
                Some(after(self.state_since, self.config.logon_timeout_ms))
            }
            State::LogoutSent => Some(after(self.state_since, self.config.logout_timeout_ms)),
            State::Active => {
                let heartbeat = after(self.last_sent, self.heartbeat_ms);
                let silence = match self.test_request {
                    Some((_, sent)) => after(sent, self.heartbeat_ms),
                    None => after(
                        self.last_received,
                        self.heartbeat_ms + self.heartbeat_ms / 5,
                    ),
                };
                let resend = self
                    .gap_until
                    .map_or(heartbeat, |_| after(self.resend_sent, self.heartbeat_ms));
                Some(heartbeat.min(silence).min(resend))
            }
        }
    }

    /// Runs the timers: heartbeats, TestRequests, and the timeouts.
    pub fn on_timer(&mut self, now: UtcMillis) {
        self.on_timer_at(SessionTime::legacy(now));
    }

    pub fn on_timer_at(&mut self, now: SessionTime) {
        let since = now.elapsed_ms.saturating_sub(self.state_since);
        match self.state {
            State::Disconnected | State::Disconnecting => {}
            State::LogonSent | State::AwaitingLogon => {
                if since >= self.config.logon_timeout_ms {
                    self.request_disconnect(DisconnectReason::LogonTimeout, now);
                }
            }
            State::LogoutSent => {
                if since >= self.config.logout_timeout_ms {
                    self.request_disconnect(DisconnectReason::LogoutTimeout, now);
                }
            }
            State::Active => self.check_heartbeats(now),
        }
    }

    fn check_heartbeats(&mut self, now: SessionTime) {
        let silent_ms = now.elapsed_ms.saturating_sub(self.last_received);
        match self.test_request {
            Some((_, sent)) if now.elapsed_ms.saturating_sub(sent) >= self.heartbeat_ms => {
                self.request_disconnect(DisconnectReason::HeartbeatTimeout, now);
                return;
            }
            None if silent_ms >= self.heartbeat_ms + self.heartbeat_ms / 5 => {
                let id = now.elapsed_ms;
                self.send(now, msg_type::TEST_REQUEST, |body| {
                    body.field_u64(tag::TEST_REQ_ID, id);
                });
                self.test_request = Some((id, now.elapsed_ms));
            }
            _ => {}
        }
        if self.gap_until.is_some()
            && now.elapsed_ms.saturating_sub(self.resend_sent) >= self.heartbeat_ms
        {
            self.send_resend_request(now);
        }
        if now.elapsed_ms.saturating_sub(self.last_sent) >= self.heartbeat_ms {
            self.send(now, msg_type::HEARTBEAT, |_| {});
        }
    }

    /// Processes one received message.
    pub fn on_message(&mut self, now: UtcMillis, message: &Message) -> Disposition {
        self.on_message_at(SessionTime::legacy(now), message)
    }

    pub fn on_message_at(&mut self, now: SessionTime, message: &Message) -> Disposition {
        if matches!(self.state, State::Disconnected | State::Disconnecting) {
            return Disposition::Drop;
        }
        self.last_received = now.elapsed_ms;
        let fields = message.fields();
        let kind = message.msg_type();
        let Some(seq) = fields.get_u64(tag::MSG_SEQ_NUM).filter(|seq| *seq >= 1) else {
            self.fatal_logout(
                now,
                b"MsgSeqNum is missing or not valid",
                DisconnectReason::BadHeader,
            );
            return Disposition::Drop;
        };
        if message.begin_string() != self.config.begin_string.as_slice() {
            self.fatal_logout(
                now,
                b"BeginString is not correct",
                DisconnectReason::BadHeader,
            );
            return Disposition::Drop;
        }
        if matches!(self.state, State::LogonSent | State::AwaitingLogon) {
            return self.on_message_before_logon(now, kind, &fields, seq);
        }
        if !self.header_is_valid(now, kind, &fields, seq) {
            return Disposition::Drop;
        }
        if kind == msg_type::SEQUENCE_RESET && fields.get_bool(tag::GAP_FILL_FLAG) != Some(true) {
            self.on_sequence_reset(now, kind, &fields, seq);
            return Disposition::Drop;
        }
        if kind == msg_type::RESEND_REQUEST && seq >= self.seq.next_target {
            // A ResendRequest is answered even when its MsgSeqNum is too high.
            self.on_resend_request(now, &fields, seq);
        }
        if !self.sequence_is_next(now, &fields, seq) {
            return Disposition::Drop;
        }
        self.on_next_message(now, kind, &fields, seq)
    }

    fn on_message_before_logon(
        &mut self,
        now: SessionTime,
        kind: &[u8],
        fields: &Fields,
        seq: u64,
    ) -> Disposition {
        if kind == msg_type::LOGOUT && self.state == State::LogonSent {
            self.request_disconnect(DisconnectReason::BadLogon, now);
            return Disposition::Drop;
        }
        if kind != msg_type::LOGON || !self.comp_ids_are_valid(fields) {
            self.request_disconnect(DisconnectReason::BadLogon, now);
            return Disposition::Drop;
        }
        if let Some(text) = self.logon_problem(now.utc, fields) {
            self.fatal_logout(now, text, DisconnectReason::BadLogon);
            return Disposition::Drop;
        }
        let heartbeat_s = fields.get_u64(tag::HEART_BT_INT).unwrap_or(1);
        let reset = fields.get_bool(tag::RESET_SEQ_NUM_FLAG) == Some(true);
        if reset {
            self.seq.next_target = 1;
            if self.config.role == Role::Acceptor {
                self.seq.next_sender = 1;
            }
        }
        if seq < self.seq.next_target {
            self.fatal_logout(
                now,
                b"MsgSeqNum too low in Logon",
                DisconnectReason::SequenceTooLow,
            );
            return Disposition::Drop;
        }
        if self.config.role == Role::Acceptor {
            self.heartbeat_ms = heartbeat_s * 1000;
            if !self.send_logon(now, reset) {
                return Disposition::Drop;
            }
        }
        self.reset_requested = false;
        self.enter(State::Active, now);
        self.push_event(Event::LoggedOn);
        if seq > self.seq.next_target {
            self.open_gap(now, seq);
        } else {
            self.seq.next_target += 1;
        }
        Disposition::Drop
    }

    /// Checks the standard header. On a problem it sends Reject (and Logout
    /// for a CompID or SendingTime problem), and returns false.
    fn header_is_valid(
        &mut self,
        now: SessionTime,
        kind: &[u8],
        fields: &Fields,
        seq: u64,
    ) -> bool {
        let Err(problem) = self.check_header(now.utc, fields) else {
            return true;
        };
        // A rejected message still uses its MsgSeqNum: the peer must not
        // send it again (FIX session test cases 2d, 2e, 2f).
        if seq == self.seq.next_target {
            self.advance_target(seq + 1);
        }
        match problem {
            HeaderProblem::CompId => {
                self.reject(now, seq, kind, REJECT_COMP_ID_PROBLEM, b"CompID problem");
                self.fatal_logout(now, b"CompID problem", DisconnectReason::BadHeader);
            }
            HeaderProblem::SendingTimeAccuracy => {
                let text = b"SendingTime accuracy problem";
                self.reject(now, seq, kind, REJECT_SENDING_TIME_ACCURACY, text);
                self.fatal_logout(now, text, DisconnectReason::BadHeader);
            }
            HeaderProblem::MissingTag(text) | HeaderProblem::BadValue(text) => {
                let reason = if matches!(problem, HeaderProblem::MissingTag(_)) {
                    REJECT_REQUIRED_TAG_MISSING
                } else {
                    REJECT_VALUE_INCORRECT
                };
                self.reject(now, seq, kind, reason, text);
            }
        }
        false
    }

    /// Checks CompIDs, the Boolean header flags, SendingTime, and, for a
    /// possible duplicate, OrigSendingTime.
    fn check_header(&self, now: UtcMillis, fields: &Fields) -> Result<(), HeaderProblem> {
        if !self.comp_ids_are_valid(fields) {
            return Err(HeaderProblem::CompId);
        }
        let flags = [
            tag::POSS_DUP_FLAG,
            tag::POSS_RESEND,
            tag::GAP_FILL_FLAG,
            tag::RESET_SEQ_NUM_FLAG,
        ];
        if flags.iter().any(|flag| fields.get_bool(*flag).is_none()) {
            return Err(HeaderProblem::BadValue(b"A Boolean field is not Y or N"));
        }
        let sending_time = fields
            .get(tag::SENDING_TIME)
            .and_then(parse_utc_timestamp)
            .ok_or(HeaderProblem::MissingTag(
                b"SendingTime is missing or not valid",
            ))?;
        if fields.get_bool(tag::POSS_DUP_FLAG) == Some(true) {
            let orig = fields
                .get(tag::ORIG_SENDING_TIME)
                .and_then(parse_utc_timestamp)
                .ok_or(HeaderProblem::MissingTag(
                    b"OrigSendingTime is missing or not valid",
                ))?;
            if orig > sending_time {
                return Err(HeaderProblem::SendingTimeAccuracy);
            }
        }
        if sending_time.0.abs_diff(now.0) > self.config.sending_time_tolerance_ms {
            return Err(HeaderProblem::SendingTimeAccuracy);
        }
        Ok(())
    }

    /// Returns the reason why a Logon (with valid CompIDs) is not valid.
    fn logon_problem(&self, now: UtcMillis, fields: &Fields) -> Option<&'static [u8]> {
        if self.check_header(now, fields).is_err() {
            return Some(b"Logon header is not valid");
        }
        if fields.get_u64(tag::ENCRYPT_METHOD) != Some(0) {
            return Some(b"EncryptMethod must be 0");
        }
        let heartbeat_valid = fields
            .get_u64(tag::HEART_BT_INT)
            .is_some_and(|value| (1..=HEART_BT_INT_MAX_S).contains(&value));
        if !heartbeat_valid {
            return Some(b"HeartBtInt is missing or out of range");
        }
        None
    }

    fn comp_ids_are_valid(&self, fields: &Fields) -> bool {
        fields.get(tag::SENDER_COMP_ID) == Some(self.config.target_comp_id.as_slice())
            && fields.get(tag::TARGET_COMP_ID) == Some(self.config.sender_comp_id.as_slice())
    }

    /// Returns true when `seq` is the expected MsgSeqNum. Handles a gap, a
    /// possible duplicate, and a MsgSeqNum that is too low.
    fn sequence_is_next(&mut self, now: SessionTime, fields: &Fields, seq: u64) -> bool {
        let expected = self.seq.next_target;
        if seq > expected {
            if self.gap_until.is_none() {
                self.open_gap(now, seq);
            }
            self.gap_until = self.gap_until.max(Some(seq));
            return false;
        }
        if seq < expected {
            if fields.get_bool(tag::POSS_DUP_FLAG) != Some(true) {
                self.text.clear();
                let _ = write!(
                    self.text,
                    "MsgSeqNum too low, expecting {expected} but received {seq}"
                );
                let text = std::mem::take(&mut self.text);
                self.fatal_logout(now, &text, DisconnectReason::SequenceTooLow);
                self.text = text;
            }
            return false;
        }
        true
    }

    fn on_next_message(
        &mut self,
        now: SessionTime,
        kind: &[u8],
        fields: &Fields,
        seq: u64,
    ) -> Disposition {
        if kind == msg_type::SEQUENCE_RESET {
            // GapFillFlag=Y: the reset mode returned earlier.
            self.on_sequence_reset(now, kind, fields, seq);
            return Disposition::Drop;
        }
        self.advance_target(seq + 1);
        match kind {
            msg_type::HEARTBEAT => {
                let id = fields.get_u64(tag::TEST_REQ_ID);
                if id.is_some() && id == self.test_request.map(|(sent, _)| sent) {
                    self.test_request = None;
                }
                Disposition::Drop
            }
            msg_type::TEST_REQUEST => {
                self.on_test_request(now, fields, seq);
                Disposition::Drop
            }
            msg_type::LOGOUT => {
                if self.state != State::LogoutSent {
                    self.send_logout(now, b"");
                }
                self.request_disconnect(DisconnectReason::LoggedOut, now);
                Disposition::Drop
            }
            msg_type::LOGON => {
                self.reject(
                    now,
                    seq,
                    kind,
                    REJECT_VALUE_INCORRECT,
                    b"Logon when logged on",
                );
                Disposition::Drop
            }
            msg_type::RESEND_REQUEST => Disposition::Drop,
            _ => Disposition::Deliver,
        }
    }

    fn on_test_request(&mut self, now: SessionTime, fields: &Fields, seq: u64) {
        let Some(id) = fields.get(tag::TEST_REQ_ID) else {
            self.reject(
                now,
                seq,
                msg_type::TEST_REQUEST,
                REJECT_REQUIRED_TAG_MISSING,
                b"TestReqID is missing",
            );
            return;
        };
        let sent = self.send(now, msg_type::HEARTBEAT, |body| {
            body.field(tag::TEST_REQ_ID, id);
        });
        debug_assert!(sent || self.state == State::Disconnecting);
    }

    /// Handles a SequenceReset. In reset mode MsgSeqNum is not checked. In
    /// gap fill mode the caller checked that `seq` is the expected one.
    fn on_sequence_reset(&mut self, now: SessionTime, kind: &[u8], fields: &Fields, seq: u64) {
        let gap_fill = fields.get_bool(tag::GAP_FILL_FLAG) == Some(true);
        let Some(new_seq) = fields.get_u64(tag::NEW_SEQ_NO) else {
            self.reject(
                now,
                seq,
                kind,
                REJECT_REQUIRED_TAG_MISSING,
                b"NewSeqNo is missing",
            );
            if gap_fill {
                self.advance_target(seq + 1);
            }
            return;
        };
        let lowest = if gap_fill {
            seq + 1
        } else {
            self.seq.next_target
        };
        if new_seq < lowest {
            self.reject(
                now,
                seq,
                kind,
                REJECT_VALUE_INCORRECT,
                b"NewSeqNo is too low",
            );
            if gap_fill {
                self.advance_target(seq + 1);
            }
            return;
        }
        self.advance_target(new_seq);
        self.push_event(Event::SequenceReset { new_seq });
    }

    /// Answers a ResendRequest with one gap fill for the requested range (R17).
    fn on_resend_request(&mut self, now: SessionTime, fields: &Fields, seq: u64) {
        let Some(begin) = fields.get_u64(tag::BEGIN_SEQ_NO) else {
            self.reject(
                now,
                seq,
                msg_type::RESEND_REQUEST,
                REJECT_REQUIRED_TAG_MISSING,
                b"BeginSeqNo is missing",
            );
            return;
        };
        // An absent EndSeqNo means "all": the same as 0. A present value
        // that is not a number is an error, not "all".
        let end = match fields.get(tag::END_SEQ_NO) {
            None => Some(0),
            Some(text) => fix_codec::parse_u64(text),
        };
        let end = end.unwrap_or(u64::MAX);
        if begin == 0 || end == u64::MAX || (end != 0 && end < begin) {
            let text = b"BeginSeqNo or EndSeqNo is not valid";
            self.reject(
                now,
                seq,
                msg_type::RESEND_REQUEST,
                REJECT_VALUE_INCORRECT,
                text,
            );
            return;
        }
        let next_sender = self.seq.next_sender;
        if begin >= next_sender {
            return;
        }
        // EndSeqNo=0 means "to the last message sent". A finite range stops
        // at EndSeqNo, so the peer does not skip messages after it.
        let new_seq = if end == 0 {
            next_sender
        } else {
            (end + 1).min(next_sender)
        };
        let mut time = [0u8; 21];
        let sending_time = write_utc_timestamp(now.utc, &mut time);
        let written = self.write_message(now, msg_type::SEQUENCE_RESET, begin, |body| {
            body.field_bool(tag::POSS_DUP_FLAG, true)
                .field(tag::ORIG_SENDING_TIME, sending_time)
                .field_bool(tag::GAP_FILL_FLAG, true)
                .field_u64(tag::NEW_SEQ_NO, new_seq);
        });
        if let Err(error) = written {
            self.disconnect_after(error, now);
            return;
        }
        self.push_event(Event::ResendRequested {
            begin,
            end,
            new_seq,
        });
    }

    fn open_gap(&mut self, now: SessionTime, received: u64) {
        let begin = self.seq.next_target;
        self.gap_until = Some(received);
        self.send_resend_request(now);
        self.push_event(Event::GapDetected { begin, received });
    }

    /// Asks for all messages from the next expected MsgSeqNum (EndSeqNo=0).
    fn send_resend_request(&mut self, now: SessionTime) {
        let begin = self.seq.next_target;
        self.send(now, msg_type::RESEND_REQUEST, |body| {
            body.field_u64(tag::BEGIN_SEQ_NO, begin)
                .field_u64(tag::END_SEQ_NO, 0);
        });
        self.resend_sent = now.elapsed_ms;
    }

    fn advance_target(&mut self, next: u64) {
        debug_assert!(
            next >= self.seq.next_target,
            "the expected MsgSeqNum never goes back"
        );
        self.seq.next_target = next;
        if self.gap_until.is_some_and(|until| next > until) {
            self.gap_until = None;
            self.push_event(Event::GapClosed);
        }
    }

    fn reject(
        &mut self,
        now: SessionTime,
        ref_seq: u64,
        ref_msg_type: &[u8],
        reason: u64,
        text: &[u8],
    ) {
        self.send(now, msg_type::REJECT, |body| {
            body.field_u64(tag::REF_SEQ_NUM, ref_seq)
                .field(tag::REF_MSG_TYPE, ref_msg_type)
                .field_u64(tag::SESSION_REJECT_REASON, reason)
                .field(tag::TEXT, text);
        });
        self.push_event(Event::RejectSent { ref_seq, reason });
    }

    /// Sends Logout with `text` and asks the host to disconnect.
    fn fatal_logout(&mut self, now: SessionTime, text: &[u8], reason: DisconnectReason) {
        if self.send_logout(now, text) {
            self.request_disconnect(reason, now);
        }
    }

    fn send_logout(&mut self, now: SessionTime, text: &[u8]) -> bool {
        self.send(now, msg_type::LOGOUT, |body| {
            if !text.is_empty() {
                body.field(tag::TEXT, text);
            }
        })
    }

    fn send_logon(&mut self, now: SessionTime, reset: bool) -> bool {
        let heartbeat_s = self.heartbeat_ms / 1000;
        let credentials = if self.config.has_logon_credentials() {
            (self.config.username.clone(), self.config.password.clone())
        } else {
            (None, None)
        };
        self.send(now, msg_type::LOGON, |body| {
            body.field_u64(tag::ENCRYPT_METHOD, 0)
                .field_u64(tag::HEART_BT_INT, heartbeat_s);
            if reset {
                body.field_bool(tag::RESET_SEQ_NUM_FLAG, true);
            }
            if let Some(username) = &credentials.0 {
                body.field(tag::USERNAME, username);
            }
            if let Some(password) = &credentials.1 {
                body.field(tag::PASSWORD, password);
            }
        })
    }

    fn request_disconnect(&mut self, reason: DisconnectReason, now: SessionTime) {
        self.enter(State::Disconnecting, now);
        self.push_event(Event::DisconnectRequested(reason));
    }

    fn enter(&mut self, state: State, now: SessionTime) {
        self.state = state;
        self.state_since = now.elapsed_ms;
    }

    /// Queues an event. When the host has not taken the events and the queue
    /// is full, the event is counted in [`Session::events_lost`] and dropped.
    fn push_event(&mut self, event: Event) {
        if self.events.len() == EVENTS_MAX {
            self.events_lost += 1;
            return;
        }
        self.events.push(event);
    }

    fn output_has_room(&self) -> bool {
        self.output.len() + self.config.message_len_max <= self.config.output_len_max
    }

    /// Sends a session message with the next MsgSeqNum. When the host does
    /// not take the output, the session disconnects instead of growing it.
    /// Returns false when the message was not written; then the session has
    /// asked the host to disconnect.
    fn send(&mut self, now: SessionTime, kind: &[u8], fill: impl FnOnce(&mut Body)) -> bool {
        let seq = self.seq.next_sender;
        match self.write_message(now, kind, seq, fill) {
            Ok(()) => {
                self.seq.next_sender += 1;
                true
            }
            Err(error) => {
                self.disconnect_after(error, now);
                false
            }
        }
    }

    fn disconnect_after(&mut self, error: SendError, now: SessionTime) {
        let reason = match error {
            SendError::TooLarge => DisconnectReason::MessageTooLarge,
            SendError::OutputFull | SendError::NotActive => DisconnectReason::OutputFull,
        };
        if self.state != State::Disconnecting {
            self.request_disconnect(reason, now);
        }
    }

    /// Writes a message with MsgSeqNum `seq` to the output. The caller moves
    /// the sender sequence. On an error nothing is written.
    fn write_message(
        &mut self,
        now: SessionTime,
        kind: &[u8],
        seq: u64,
        fill: impl FnOnce(&mut Body),
    ) -> Result<(), SendError> {
        if !self.output_has_room() {
            return Err(SendError::OutputFull);
        }
        let mut time = [0u8; 21];
        let sending_time = write_utc_timestamp(now.utc, &mut time);
        let mut body = self.encoder.start(kind);
        body.field(tag::SENDER_COMP_ID, &self.config.sender_comp_id)
            .field(tag::TARGET_COMP_ID, &self.config.target_comp_id)
            .field_u64(tag::MSG_SEQ_NUM, seq)
            .field(tag::SENDING_TIME, sending_time);
        fill(&mut body);
        let bytes = body
            .finish(&self.config.begin_string)
            .map_err(|_| SendError::TooLarge)?;
        self.output.extend_from_slice(bytes);
        self.last_sent = now.elapsed_ms;
        Ok(())
    }
}
