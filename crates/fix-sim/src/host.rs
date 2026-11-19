//! One side of the simulation: a session and the host code around it. The
//! host keeps a receive buffer, frames and decodes the bytes, and records what
//! the session sent, so that the world can check what the peer delivered.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use fix_codec::{Decoder, Encoder, UtcMillis, frame, tag, write_utc_timestamp};
use fix_session::{Config, Disposition, Event, SendError, SequenceNumbers, Session, State};

use crate::pipe::Pipe;
use crate::rng::Rng;

const FRAME_MAX: usize = 4096;
const DECODER_FIELDS: usize = 64;
/// ClOrdID: the tag of the order id in the application messages.
const ORDER_TAG: u32 = 11;
const NEW_ORDER: &[u8] = b"D";

/// A message that the session wrote, except a gap fill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub kind: Vec<u8>,
    pub order: Option<u64>,
    pub at: UtcMillis,
    /// The bytes of an order, for [`Host::duplicate_order`]. Empty for others.
    pub raw: Vec<u8>,
}

/// A message that the session gave to the application.
#[derive(Debug)]
pub struct Delivered {
    pub seq: u64,
    pub kind: Vec<u8>,
    pub order: Option<u64>,
}

/// Counts of what happened during one run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub logons: u64,
    pub gaps_detected: u64,
    pub gaps_closed: u64,
    pub resend_requests: u64,
    pub sequence_resets: u64,
    pub rejects_sent: u64,
    pub disconnects_requested: u64,
    pub orders_sent: u64,
    pub delivered: u64,
    pub messages_dropped: u64,
}

#[derive(Debug)]
pub struct Host {
    pub session: Session,
    /// The local clock is the simulation clock plus this offset.
    pub skew_ms: i64,
    pub connected: bool,
    /// The next outgoing message is lost before it reaches the wire.
    pub drop_armed: bool,
    /// A lost message can be a ResendRequest or a gap fill. See
    /// [`crate::Options`].
    pub drop_recovery: bool,
    pub sent: BTreeMap<u64, Sent>,
    pub last_delivered: u64,
    pub delivered_orders: BTreeSet<u64>,
    pub stats: Stats,
    recv: Vec<u8>,
    frame_copy: Vec<u8>,
    out: Vec<u8>,
    decoder: Decoder,
    encoder: Encoder,
    floor: SequenceNumbers,
}

impl Host {
    pub fn new(config: Config, skew_ms: i64, drop_recovery: bool) -> Self {
        Self {
            session: Session::new(config, SequenceNumbers::default()),
            skew_ms,
            connected: false,
            drop_armed: false,
            drop_recovery,
            sent: BTreeMap::new(),
            last_delivered: 0,
            delivered_orders: BTreeSet::new(),
            stats: Stats::default(),
            recv: Vec::new(),
            frame_copy: Vec::new(),
            out: Vec::new(),
            decoder: Decoder::new(DECODER_FIELDS),
            encoder: Encoder::new(FRAME_MAX),
            floor: SequenceNumbers::default(),
        }
    }

    pub fn local(&self, now: UtcMillis) -> UtcMillis {
        UtcMillis(now.0.saturating_add_signed(self.skew_ms))
    }

    /// The simulation time of the next session timer, or `None`.
    pub fn deadline(&self) -> Option<UtcMillis> {
        if !self.connected {
            return None;
        }
        let local = self.session.next_deadline()?;
        Some(UtcMillis(local.0.saturating_add_signed(-self.skew_ms)))
    }

    pub fn connect(&mut self, now: UtcMillis) {
        self.connected = true;
        self.recv.clear();
        self.session.on_connected(self.local(now));
    }

    pub fn disconnect(&mut self, now: UtcMillis) {
        self.connected = false;
        self.recv.clear();
        self.session.on_disconnected(self.local(now));
    }

    pub fn timer(&mut self, now: UtcMillis) {
        self.session.on_timer(self.local(now));
    }

    pub fn logout(&mut self, now: UtcMillis) {
        self.session.logout(self.local(now), b"sim logout");
    }

    /// Sends one order. Returns the MsgSeqNum, or `None` when the session is
    /// not active. Checks that the session refuses exactly when not active.
    pub fn send_order(&mut self, now: UtcMillis, id: u64) -> Result<Option<u64>, String> {
        let local = self.local(now);
        let active = self.session.state() == State::Active;
        let result = self.session.send_app(local, NEW_ORDER, |body| {
            body.field_u64(ORDER_TAG, id).field(55, b"SIM");
        });
        match (result, active) {
            (Ok(seq), true) => {
                self.stats.orders_sent += 1;
                Ok(Some(seq))
            }
            (Err(SendError::NotActive), false) => Ok(None),
            (Ok(_), false) => Err("send_app accepted a message when not active".into()),
            (Err(SendError::NotActive), true) => {
                Err("send_app refused a message when active".into())
            }
            (Err(error), _) => Err(format!("send_app failed: {error:?}")),
        }
    }

    /// Frames and decodes the received bytes and runs the session on each
    /// message. Returns the messages that the session delivered.
    pub fn receive(&mut self, now: UtcMillis, bytes: &[u8]) -> Result<Vec<Delivered>, String> {
        self.recv.extend_from_slice(bytes);
        let local = self.local(now);
        let mut delivered = Vec::new();
        while !self.session.disconnect_requested() {
            let len = match frame(&self.recv, FRAME_MAX) {
                Ok(Some(len)) => len,
                Ok(None) => break,
                Err(error) => return Err(format!("frame error {error:?}")),
            };
            self.frame_copy.clear();
            self.frame_copy.extend_from_slice(&self.recv[..len]);
            self.recv.drain(..len);
            let message = self
                .decoder
                .decode(&self.frame_copy)
                .map_err(|error| format!("decode error {error:?}"))?;
            let fields = message.fields();
            let seq = fields.get_u64(tag::MSG_SEQ_NUM).unwrap_or(0);
            let (kind, order) = (message.msg_type().to_vec(), fields.get_u64(ORDER_TAG));
            if self.session.on_message(local, &message) == Disposition::Deliver {
                self.stats.delivered += 1;
                delivered.push(Delivered { seq, kind, order });
            }
            self.count_events();
            if self.session.events_lost() != 0 {
                return Err("the session lost events".into());
            }
        }
        Ok(delivered)
    }

    /// Takes the events and the output of the session. Records the messages,
    /// loses one when `drop_armed`, and writes the rest to `pipe`.
    pub fn flush(&mut self, now: UtcMillis, pipe: &mut Pipe, rng: &mut Rng) -> Result<(), String> {
        self.count_events();
        self.out.clear();
        self.session.take_output(&mut self.out);
        if self.out.is_empty() {
            return Ok(());
        }
        let spans = self.split_output()?;
        let recovery = self.record_sent(&spans, now)?;
        let lost = self.pick_lost(&recovery, rng);
        let mut wire = Vec::with_capacity(self.out.len());
        for (index, span) in spans.iter().enumerate() {
            if Some(index) != lost {
                wire.extend_from_slice(&self.out[span.clone()]);
            }
        }
        pipe.write(&wire, now, rng);
        Ok(())
    }

    /// When a loss is armed, chooses the message to lose. A message of the
    /// recovery protocol is lost only when `drop_recovery` is set.
    fn pick_lost(&mut self, recovery: &[bool], rng: &mut Rng) -> Option<usize> {
        if !self.drop_armed || !self.connected {
            return None;
        }
        let candidates: Vec<usize> = (0..recovery.len())
            .filter(|index| self.drop_recovery || !recovery[*index])
            .collect();
        if candidates.is_empty() {
            return None;
        }
        self.drop_armed = false;
        self.stats.messages_dropped += 1;
        candidates.get(rng.index(candidates.len())).copied()
    }

    /// Writes an old order again, with PossDupFlag=Y and OrigSendingTime, as a
    /// peer that resends messages by mistake. Returns false when there is no
    /// order to repeat. The session does not know about the copy, so it is no
    /// new message: the peer must not deliver the same order twice.
    pub fn duplicate_order(
        &mut self,
        now: UtcMillis,
        pipe: &mut Pipe,
        rng: &mut Rng,
    ) -> Result<bool, String> {
        let orders: Vec<&Sent> = self
            .sent
            .values()
            .filter(|sent| sent.order.is_some())
            .collect();
        if orders.is_empty() || !self.connected {
            return Ok(false);
        }
        let pick = rng.index(orders.len());
        let raw = orders[pick].raw.clone();
        let local = self.local(now);
        let message = self
            .decoder
            .decode(&raw)
            .map_err(|error| format!("stored order does not decode: {error:?}"))?;
        let mut stamp = [0u8; 21];
        let time = write_utc_timestamp(local, &mut stamp);
        let mut body = self.encoder.start(message.msg_type());
        for (field_tag, value) in message.fields().iter() {
            match field_tag {
                tag::MSG_TYPE => {}
                tag::SENDING_TIME => {
                    body.field(tag::SENDING_TIME, time)
                        .field(tag::ORIG_SENDING_TIME, value);
                }
                _ => {
                    body.field(field_tag, value);
                }
            }
        }
        body.field_bool(tag::POSS_DUP_FLAG, true);
        let bytes = body
            .finish(message.begin_string())
            .map_err(|_| "the replayed order is too large".to_string())?;
        pipe.write(bytes, now, rng);
        Ok(true)
    }

    /// Checks that neither sequence number went back.
    pub fn check_numbers(&mut self) -> Result<(), String> {
        let now = self.session.sequence_numbers();
        if now.next_sender < self.floor.next_sender || now.next_target < self.floor.next_target {
            return Err(format!(
                "sequence numbers went back from {:?} to {now:?}",
                self.floor
            ));
        }
        self.floor = now;
        Ok(())
    }

    fn count_events(&mut self) {
        for event in self.session.take_events() {
            let stats = &mut self.stats;
            match event {
                Event::LoggedOn => stats.logons += 1,
                Event::GapDetected { .. } => stats.gaps_detected += 1,
                Event::GapClosed => stats.gaps_closed += 1,
                Event::ResendRequested { .. } => stats.resend_requests += 1,
                Event::SequenceReset { .. } => stats.sequence_resets += 1,
                Event::RejectSent { .. } => stats.rejects_sent += 1,
                Event::DisconnectRequested(_) => stats.disconnects_requested += 1,
            }
        }
    }

    fn split_output(&self) -> Result<Vec<Range<usize>>, String> {
        let mut spans = Vec::new();
        let mut start = 0;
        while start < self.out.len() {
            match frame(&self.out[start..], FRAME_MAX) {
                Ok(Some(len)) => spans.push(start..start + len),
                other => return Err(format!("the session wrote a bad frame: {other:?}")),
            }
            start += spans.last().map_or(0, Range::len);
        }
        Ok(spans)
    }

    /// Records the messages. Returns, for each, whether it belongs to the
    /// recovery protocol: a ResendRequest or a gap fill.
    fn record_sent(&mut self, spans: &[Range<usize>], now: UtcMillis) -> Result<Vec<bool>, String> {
        let mut recovery = Vec::with_capacity(spans.len());
        for span in spans {
            let message = self
                .decoder
                .decode(&self.out[span.clone()])
                .map_err(|error| format!("the session wrote an undecodable message: {error:?}"))?;
            let fields = message.fields();
            let kind = message.msg_type().to_vec();
            let gap_fill = kind == b"4" && fields.get_bool(tag::GAP_FILL_FLAG) == Some(true);
            recovery.push(gap_fill || kind == b"2");
            if gap_fill {
                continue;
            }
            let Some(seq) = fields.get_u64(tag::MSG_SEQ_NUM) else {
                return Err("the session wrote a message without MsgSeqNum".into());
            };
            let order = fields.get_u64(ORDER_TAG);
            let raw = if order.is_some() {
                self.out[span.clone()].to_vec()
            } else {
                Vec::new()
            };
            let sent = Sent {
                kind,
                order,
                at: now,
                raw,
            };
            if let Some(first) = self.sent.insert(seq, sent.clone()) {
                return Err(format!(
                    "MsgSeqNum {seq} was sent twice: {first:?} and {sent:?}"
                ));
            }
        }
        Ok(recovery)
    }
}
