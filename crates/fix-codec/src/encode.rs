use crate::frame::{SOH, checksum};
use crate::tag;

/// The message is longer than the encoder limit. Nothing was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLarge;

/// Writes FIX messages of at most `len_max` bytes. It allocates its buffers
/// once, at that size, and never grows them.
#[derive(Debug)]
pub struct Encoder {
    body: Vec<u8>,
    out: Vec<u8>,
    len_max: usize,
    overflow: bool,
}

impl Encoder {
    pub fn new(len_max: usize) -> Self {
        assert!(len_max >= 64, "a FIX message needs at least 64 bytes");
        Self {
            body: Vec::with_capacity(len_max),
            out: Vec::with_capacity(len_max),
            len_max,
            overflow: false,
        }
    }

    pub fn len_max(&self) -> usize {
        self.len_max
    }

    /// Starts a message of `msg_type`. Add the other fields in order, then
    /// call [`Body::finish`].
    pub fn start(&mut self, msg_type: &[u8]) -> Body<'_> {
        self.body.clear();
        self.overflow = false;
        let mut body = Body { encoder: self };
        body.field(tag::MSG_TYPE, msg_type);
        body
    }
}

/// A message in progress.
#[derive(Debug)]
pub struct Body<'e> {
    encoder: &'e mut Encoder,
}

impl<'e> Body<'e> {
    /// Adds a field. The value must not be empty or contain SOH.
    pub fn field(&mut self, tag: u32, value: &[u8]) -> &mut Self {
        assert!(!value.is_empty(), "tag {tag}: a FIX value is never empty");
        assert!(
            !value.contains(&SOH),
            "tag {tag}: use data_field for values with SOH"
        );
        self.write(tag, value)
    }

    pub fn field_u64(&mut self, tag: u32, value: u64) -> &mut Self {
        let mut digits = [0u8; 20];
        let text = format_u64(value, &mut digits);
        self.write(tag, text)
    }

    pub fn field_bool(&mut self, tag: u32, value: bool) -> &mut Self {
        self.write(tag, if value { b"Y" } else { b"N" })
    }

    /// Adds a data field and its length field. The data can contain SOH.
    pub fn data_field(&mut self, length_tag: u32, data: &[u8]) -> &mut Self {
        let data_tag = tag::data_tag_for(length_tag);
        assert!(data_tag.is_some(), "tag {length_tag} is not a length field");
        assert!(
            !data.is_empty(),
            "tag {length_tag}: a FIX value is never empty"
        );
        self.field_u64(length_tag, data.len() as u64);
        self.write(data_tag.unwrap_or_default(), data)
    }

    /// Adds `BeginString`, `BodyLength`, and `CheckSum`, and returns the
    /// complete message, or [`TooLarge`] when it is longer than the limit.
    pub fn finish(self, begin_string: &[u8]) -> Result<&'e [u8], TooLarge> {
        let encoder = self.encoder;
        // "8=" SOH "9=" up to 20 digits SOH, and the 7-byte trailer.
        let total_max = begin_string.len() + encoder.body.len() + 4 + 23 + 7;
        if encoder.overflow || total_max > encoder.len_max {
            return Err(TooLarge);
        }
        let out = &mut encoder.out;
        out.clear();
        write_field(out, tag::BEGIN_STRING, begin_string);
        let mut digits = [0u8; 20];
        let body_len = format_u64(encoder.body.len() as u64, &mut digits);
        write_field(out, tag::BODY_LENGTH, body_len);
        out.extend_from_slice(&encoder.body);
        let sum = checksum(out);
        let sum_text = [b'0' + sum / 100, b'0' + sum / 10 % 10, b'0' + sum % 10];
        write_field(out, tag::CHECK_SUM, &sum_text);
        debug_assert!(out.len() <= encoder.len_max);
        Ok(out)
    }

    /// Writes a field when it fits in the limit. When it does not fit, the
    /// message is marked too large and `finish` returns an error.
    fn write(&mut self, tag: u32, value: &[u8]) -> &mut Self {
        let encoder = &mut *self.encoder;
        // Up to 10 tag digits, '=', and SOH.
        let field_max = value.len() + 12;
        if encoder.overflow || encoder.body.len() + field_max > encoder.len_max {
            encoder.overflow = true;
            return self;
        }
        write_field(&mut encoder.body, tag, value);
        self
    }
}

fn write_field(out: &mut Vec<u8>, tag: u32, value: &[u8]) {
    let mut digits = [0u8; 20];
    out.extend_from_slice(format_u64(u64::from(tag), &mut digits));
    out.push(b'=');
    out.extend_from_slice(value);
    out.push(SOH);
}

fn format_u64(mut value: u64, digits: &mut [u8; 20]) -> &[u8] {
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = b'0' + u8::try_from(value % 10).unwrap_or_default();
        value /= 10;
        if value == 0 {
            return &digits[start..];
        }
    }
}
