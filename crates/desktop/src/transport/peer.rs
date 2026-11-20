use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use fix_codec::{Decoder, Message, UtcMillis, frame, frame_with_checksum, tag};
use fix_session::{Config, Event, Role, Session, SessionTime, State};

use super::{Control, POLL, Phase, Send, SendRequest};
use crate::model::{FIELDS_MAX, MESSAGE_MAX, OrderKind, OrderValues, now};
use crate::store::{Checkpoint, Direction, Packet, SavedProfile, Store, Transition};
use crate::{Error, Result, invalid};

const CHUNK: usize = 8192;
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_millis(750);
const SEQ_MAX: u64 = (i64::MAX - 1) as u64;

struct Peer<'a> {
    stream: TcpStream,
    epoch: Instant,
    session: Session,
    reset_expected: bool,
    reset_armed: bool,
    decoder: Decoder,
    input: Vec<u8>,
    output: Vec<u8>,
    store: &'a mut Store,
    saved: &'a SavedProfile,
    control: &'a Control,
}

pub(super) fn connect(
    stream: TcpStream,
    store: &mut Store,
    saved: &SavedProfile,
    control: &Control,
) -> Result<()> {
    stream.set_read_timeout(Some(POLL))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    stream.set_nodelay(true)?;
    let profile = &saved.profile;
    let checkpoint = store.checkpoint(saved.id)?;
    let mut config = Config::new(
        profile.role,
        profile.version.begin_string().as_bytes(),
        profile.sender.as_bytes(),
        profile.target.as_bytes(),
    );
    config.heartbeat_interval_ms = u64::from(profile.heartbeat) * 1000;
    config.reset_on_logon = checkpoint.reset && profile.role == Role::Initiator;
    config.logout_timeout_ms = 1500;
    config.message_len_max = MESSAGE_MAX;
    config.output_len_max = 4 * MESSAGE_MAX;
    let mut peer = Peer {
        stream,
        epoch: Instant::now(),
        session: Session::new(config, checkpoint.seq),
        reset_expected: checkpoint.reset,
        reset_armed: checkpoint.reset && profile.role == Role::Acceptor,
        decoder: Decoder::new(FIELDS_MAX),
        input: Vec::with_capacity(MESSAGE_MAX + CHUNK),
        output: Vec::new(),
        store,
        saved,
        control,
    };
    control.update(|status| status.notice.clear());
    let time = peer.time();
    peer.session.on_connected_at(time);
    peer.commit(time.utc, None, None)?;
    let result = peer.run();
    if !peer.input.is_empty() && !matches!(result, Err(Error::Database(_))) {
        let raw = std::mem::take(&mut peer.input);
        peer.commit(now(), Some((&raw, "Incomplete frame")), None)?;
    }
    result
}

impl Peer<'_> {
    fn time(&self) -> SessionTime {
        SessionTime {
            elapsed_ms: u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX),
            utc: now(),
        }
    }

    fn run(&mut self) -> Result<()> {
        let mut chunk = [0; CHUNK];
        let mut partial_since = None::<Instant>;
        while !self.session.disconnect_requested() {
            if self.control.stopping() && !self.stop()? {
                break;
            }
            if let Ok(command) = self.control.commands.try_recv() {
                self.send(command)?;
            }
            let time = self.time();
            if self
                .session
                .next_deadline_ms()
                .is_some_and(|due| due <= time.elapsed_ms)
            {
                self.session.on_timer_at(time);
                self.commit(time.utc, None, None)?;
                continue;
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    if self.control.stopping() {
                        break;
                    }
                    return Err(invalid("Peer closed the TCP connection."));
                }
                Ok(count) => {
                    self.input.extend_from_slice(&chunk[..count]);
                    if self.receive()? || self.input.is_empty() {
                        partial_since = None;
                    }
                    if !self.input.is_empty() {
                        partial_since.get_or_insert_with(Instant::now);
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::Interrupted | ErrorKind::WouldBlock | ErrorKind::TimedOut
                    ) => {}
                Err(error) => return Err(error.into()),
            }
            if partial_since.is_some_and(|start| start.elapsed() > FRAME_TIMEOUT) {
                return Err(invalid(
                    "The peer did not finish a FIX frame within five seconds.",
                ));
            }
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<bool> {
        match self.session.state() {
            State::Active => {
                let time = self.time();
                self.session.logout_at(time, b"");
                self.commit(time.utc, None, None)?;
                Ok(true)
            }
            State::LogoutSent => Ok(true),
            _ => Ok(false),
        }
    }

    fn receive(&mut self) -> Result<bool> {
        let mut bytes = std::mem::take(&mut self.input);
        let mut offset = 0;
        let result = self.receive_frames(&bytes, &mut offset);
        bytes.drain(..offset);
        self.input = bytes;
        result.map(|()| offset > 0)
    }

    fn receive_frames(&mut self, bytes: &[u8], offset: &mut usize) -> Result<()> {
        while *offset < bytes.len() {
            let rest = &bytes[*offset..];
            if self.session.disconnect_requested() {
                *offset = bytes.len();
                return self.commit(now(), Some((rest, "After disconnect")), None);
            }
            let framed = match frame_with_checksum(rest, MESSAGE_MAX) {
                Ok(Some(framed)) => framed,
                Ok(None) => return Ok(()),
                Err(error) => {
                    *offset = bytes.len();
                    self.commit(now(), Some((rest, "Invalid frame")), None)?;
                    return Err(invalid(format!("Invalid FIX frame: {error:?}")));
                }
            };
            *offset += framed.len;
            let raw = &rest[..framed.len];
            if framed.checksum.is_ok() {
                self.receive_message(raw)?;
            } else {
                self.commit(now(), Some((raw, "Invalid checksum")), None)?;
            }
        }
        Ok(())
    }

    fn receive_message(&mut self, raw: &[u8]) -> Result<()> {
        let time = self.time();
        let Ok(message) = self.decoder.decode(raw) else {
            return self.commit(time.utc, Some((raw, "Invalid fields")), None);
        };
        if let Err(error) = check_receive(self.session.state(), self.reset_expected, &message) {
            self.commit(time.utc, Some((raw, "Rejected input")), None)?;
            return Err(error);
        }
        self.session.on_message_at(time, &message);
        if self.session.state() == State::Active {
            self.reset_armed = false;
        }
        self.commit(time.utc, Some((raw, "Received")), None)
    }

    fn send(&mut self, command: SendRequest) -> Result<()> {
        let time = self.time();
        let result = self
            .prepare(command.message, time.utc)
            .and_then(|(application, values)| {
                let application = application.with_additional_tags(&command.additional)?;
                self.session
                    .send_app_at(time, application.kind.as_bytes(), |body| {
                        for (tag, value) in &application.fields {
                            body.field(*tag, value.as_bytes());
                        }
                    })
                    .map_err(|error| {
                        invalid(format!("Cannot send application message: {error:?}"))
                    })?;
                Ok(values)
            });
        let result = match result {
            Ok(values) => self.commit(time.utc, None, values.as_ref()),
            Err(error @ Error::Database(_)) => return Err(error),
            Err(error) => {
                self.complete_command(Some(error.to_string()));
                return Ok(());
            }
        };
        self.complete_command(result.as_ref().err().map(ToString::to_string));
        result
    }

    fn prepare(
        &self,
        command: Send,
        time: UtcMillis,
    ) -> Result<(crate::model::Application, Option<OrderValues>)> {
        match command {
            Send::Tags(message) if message.version == self.saved.profile.version => {
                Ok((message, None))
            }
            Send::Tags(_) => Err(invalid(
                "Build the application message for the selected FIX version.",
            )),
            Send::Execution(report) => {
                if report.version() != self.saved.profile.version {
                    return Err(invalid("Build the report for the selected FIX version."));
                }
                report.validate()?;
                let exec_id = match report.exec_id() {
                    Some(value) => value.into(),
                    None => self.store.next_order_id(time)?,
                };
                Ok((report.application(time, exec_id)?, None))
            }
            Send::GroupedOrder(order) => {
                let id = self.store.next_order_id(time)?;
                Ok((
                    order.application(self.saved.profile.version, id, time)?,
                    None,
                ))
            }
            Send::Order(order) => {
                order.validate()?;
                let id = self.store.next_order_id(time)?;
                let application = order.application(self.saved.profile.version, id, time)?;
                let values = (order.kind != OrderKind::Cancel).then_some(order.values);
                Ok((application, values))
            }
        }
    }

    fn complete_command(&self, error: Option<String>) {
        self.control.update(|status| {
            status.completed += 1;
            status.notice = error.clone().unwrap_or_else(|| "Message sent.".into());
            status.send_error = error;
        });
    }

    fn commit(
        &mut self,
        time: UtcMillis,
        received: Option<(&[u8], &str)>,
        values: Option<&OrderValues>,
    ) -> Result<()> {
        self.output.clear();
        self.session.take_output(&mut self.output);
        let seq = self.session.sequence_numbers();
        let outgoing = frames(&self.output)?;
        let had_io = received.is_some() || !outgoing.is_empty();
        if had_io {
            let pending = self.store.commit(
                self.saved.id,
                &Transition {
                    time,
                    checkpoint: Checkpoint {
                        seq,
                        reset: self.reset_armed,
                    },
                    outgoing: &outgoing,
                    received: received.map(|(raw, status)| Packet {
                        raw,
                        status,
                        direction: Direction::Received,
                    }),
                    values,
                },
            )?;
            let result = write_outgoing(
                &mut self.stream,
                self.store,
                outgoing.into_iter().zip(pending.ids().iter().copied()),
            );
            drop(pending);
            self.control.update(|status| {
                status.seq = seq;
                status.revision += 1;
            });
            result?;
        }
        let note = self
            .session
            .take_events()
            .filter_map(event_note)
            .next_back();
        self.control.update(|status| {
            status.phase = Phase::Link(self.session.state());
            status.seq = seq;
            if let Some(note) = note {
                status.notice = note;
            }
        });
        Ok(())
    }
}

fn frames(mut bytes: &[u8]) -> Result<Vec<&[u8]>> {
    let mut frames = Vec::new();
    while !bytes.is_empty() {
        let size = frame(bytes, MESSAGE_MAX)
            .ok()
            .flatten()
            .ok_or_else(|| invalid("Session generated an invalid FIX frame."))?;
        let (message, rest) = bytes.split_at(size);
        frames.push(message);
        bytes = rest;
    }
    Ok(frames)
}

fn check_receive(state: State, reset_expected: bool, message: &Message<'_>) -> Result<()> {
    let fields = message.fields();
    for number in [
        tag::MSG_SEQ_NUM,
        tag::NEW_SEQ_NO,
        tag::BEGIN_SEQ_NO,
        tag::END_SEQ_NO,
    ] {
        if fields.get_u64(number).is_some_and(|seq| seq > SEQ_MAX) {
            return Err(invalid(
                "The peer sequence number exceeds the storage limit.",
            ));
        }
    }
    if message.msg_type() == tag::msg_type::LOGON
        && matches!(state, State::LogonSent | State::AwaitingLogon)
    {
        let reset = fields.get_bool(tag::RESET_SEQ_NUM_FLAG);
        if reset != Some(reset_expected)
            || (reset_expected && fields.get_u64(tag::MSG_SEQ_NUM) != Some(1))
        {
            return Err(invalid(if reset_expected {
                "The reset Logon requires ResetSeqNumFlag=Y and MsgSeqNum=1."
            } else {
                "The peer requested a reset. Stop the session and confirm a coordinated reset."
            }));
        }
    }
    Ok(())
}

fn write_outgoing<'a>(
    stream: &mut impl Write,
    store: &Store,
    frames: impl IntoIterator<Item = (&'a [u8], i64)>,
) -> Result<()> {
    for (raw, id) in frames {
        let (count, result) = write_frame(stream, raw);
        store.finish_write(id, count, result.is_ok())?;
        result?;
    }
    Ok(())
}

fn write_frame(stream: &mut impl Write, raw: &[u8]) -> (usize, std::io::Result<()>) {
    let started = Instant::now();
    let mut count = 0;
    while count < raw.len() {
        let result = stream.write(&raw[count..]);
        match result {
            Ok(0) => return (count, Err(ErrorKind::WriteZero.into())),
            Ok(size) => count += size,
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => return (count, Err(err)),
        }
        if count < raw.len() && started.elapsed() >= WRITE_TIMEOUT {
            return (count, Err(ErrorKind::TimedOut.into()));
        }
    }
    (count, Ok(()))
}

fn event_note(event: Event) -> Option<String> {
    match event {
        Event::LoggedOn => Some("Logon complete.".into()),
        Event::GapDetected { begin, received } => Some(format!(
            "Sequence gap: expected {begin}, received {received}."
        )),
        Event::GapClosed => Some("Sequence gap closed.".into()),
        Event::DisconnectRequested(reason) => Some(format!("Session closed: {reason:?}.")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::model::Profile;
    use crate::store::Filter;
    use fix_dict::Version;
    use fix_session::SequenceNumbers;

    struct BrokenWire {
        written: Vec<u8>,
    }

    impl Write for BrokenWire {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if !self.written.is_empty() {
                return Err(ErrorKind::BrokenPipe.into());
            }
            self.written.extend_from_slice(&bytes[..6]);
            Ok(6)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn partial_write_preserves_the_reserved_sequence_and_captured_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let mut store = Store::open(&path).unwrap();
        let profile = Profile {
            name: "Partial TCP write".into(),
            version: Version::Fix44,
            role: Role::Initiator,
            sender: "TEST".into(),
            target: "PEER".into(),
            address: "127.0.0.1:9876".parse().unwrap(),
            heartbeat: 30,
        };
        let id = store.save_profile(None, &profile).unwrap();
        let raw = b"8=FIX.4.4\x019=5\x0135=D\x0110=180\x01";
        let rows = store
            .commit(
                id,
                &Transition {
                    time: now(),
                    checkpoint: Checkpoint {
                        seq: SequenceNumbers {
                            next_sender: 12,
                            next_target: 5,
                        },
                        reset: false,
                    },
                    received: None,
                    outgoing: &[raw],
                    values: None,
                },
            )
            .unwrap();
        let mut wire = BrokenWire {
            written: Vec::new(),
        };
        assert!(write_outgoing(&mut wire, &store, [(raw.as_slice(), rows.ids()[0])]).is_err());
        drop(rows);
        drop(store);

        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.checkpoint(id).unwrap().seq.next_sender, 12);
        let row = reopened
            .history(id, &Filter::default(), None)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(row.status, "Write failed");
        assert_eq!(row.raw, raw);
        assert_eq!(row.transferred, 6);
        let export = dir.path().join("partial.fix");
        assert_eq!(reopened.export(id, &Filter::default(), &export).unwrap(), 1);
        assert_eq!(std::fs::read(&export).unwrap(), wire.written);
        assert!(reopened.export(id, &Filter::default(), &export).is_err());
        assert_eq!(std::fs::read(export).unwrap(), wire.written);
    }
}
