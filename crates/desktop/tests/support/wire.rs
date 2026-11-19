use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use fix_codec::{Decoder, Encoder, frame, write_utc_timestamp};
use fixtester_desktop::model::{Profile, now};

pub struct Wire {
    pub stream: TcpStream,
    input: Vec<u8>,
    pub profile: Profile,
}

impl Wire {
    pub fn new(stream: TcpStream, profile: &Profile) -> Self {
        stream
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        Self {
            stream,
            input: Vec::new(),
            profile: profile.clone(),
        }
    }

    pub fn receive(&mut self, expected: &[u8], seq: u64) -> Vec<u8> {
        while frame(&self.input, 16_384).unwrap().is_none() {
            let mut buffer = [0; 4096];
            let count = self.stream.read(&mut buffer).unwrap();
            assert_ne!(count, 0);
            self.input.extend_from_slice(&buffer[..count]);
        }
        let size = frame(&self.input, 16_384).unwrap().unwrap();
        let raw = self.input.drain(..size).collect::<Vec<_>>();
        let mut decoder = Decoder::new(256);
        let message = decoder.decode(&raw).unwrap();
        assert_eq!(message.msg_type(), expected);
        assert_eq!(message.fields().get_u64(34), Some(seq));
        assert_eq!(
            message.fields().get(49),
            Some(self.profile.sender.as_bytes())
        );
        assert_eq!(
            message.fields().get(56),
            Some(self.profile.target.as_bytes())
        );
        assert_eq!(
            message.begin_string(),
            self.profile.version.begin_string().as_bytes()
        );
        raw
    }

    pub fn send(&mut self, kind: &[u8], seq: u64, fields: &[(u32, &[u8])]) -> Vec<u8> {
        let raw = self.message(kind, seq, fields);
        // Split inside the body-length field.
        self.stream.write_all(&raw[..10]).unwrap();
        self.stream.write_all(&raw[10..]).unwrap();
        raw
    }

    pub fn message(&self, kind: &[u8], seq: u64, fields: &[(u32, &[u8])]) -> Vec<u8> {
        let mut encoder = Encoder::new(16_384);
        let mut clock = [0; 21];
        let mut body = encoder.start(kind);
        body.field(49, self.profile.target.as_bytes())
            .field(56, self.profile.sender.as_bytes())
            .field_u64(34, seq)
            .field(52, write_utc_timestamp(now(), &mut clock));
        for &(tag, value) in fields {
            body.field(tag, value);
        }
        body.finish(self.profile.version.begin_string().as_bytes())
            .unwrap()
            .to_vec()
    }

    pub fn logon(&mut self, seq: u64, reset: bool) {
        let interval = self.profile.heartbeat.to_string();
        let mut fields = vec![(98, &b"0"[..]), (108, interval.as_bytes())];
        if reset {
            fields.push((141, b"Y"));
        }
        self.send(b"A", seq, &fields);
    }
}
