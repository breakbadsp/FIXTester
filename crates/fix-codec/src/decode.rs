use crate::frame::SOH;
use crate::tag;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// A tag is empty, is not decimal, or is zero.
    BadTag { position: u32 },
    /// A field has no `=`.
    MissingEquals { position: u32 },
    /// A field has no value.
    EmptyValue { tag: u32 },
    /// A data field does not match the length that its length field gives.
    BadDataLength { tag: u32 },
    /// The first three fields are not 8, 9, and 35, or the last is not 10.
    BadStructure,
    /// The message has more fields than the decoder limit.
    TooManyFields,
}

/// One field: its tag and the position of its value in the message bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub tag: u32,
    value_start: u32,
    value_end: u32,
}

/// Splits framed messages into fields. It keeps one field table and reuses
/// it, so decoding does not allocate after the first message.
#[derive(Debug)]
pub struct Decoder {
    fields: Vec<Field>,
    fields_max: usize,
}

impl Decoder {
    pub fn new(fields_max: usize) -> Self {
        assert!(fields_max >= 4, "a message has at least four fields");
        Self {
            fields: Vec::with_capacity(fields_max),
            fields_max,
        }
    }

    /// Decodes one message that [`crate::frame`] returned.
    pub fn decode<'a>(&'a mut self, bytes: &'a [u8]) -> Result<Message<'a>, DecodeError> {
        assert!(
            u32::try_from(bytes.len()).is_ok(),
            "the frame limit is below 4 GiB"
        );
        self.fields.clear();
        let mut position = 0usize;
        let mut data_len: Option<(u32, usize)> = None;
        while position < bytes.len() {
            if self.fields.len() == self.fields_max {
                return Err(DecodeError::TooManyFields);
            }
            let field = read_field(bytes, position, data_len.take())?;
            if let Some(data_tag) = tag::data_tag_for(field.tag) {
                let length = parse_u64(value_of(bytes, &field))
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or(DecodeError::BadDataLength { tag: field.tag })?;
                data_len = Some((data_tag, length));
            }
            position = field.value_end as usize + 1;
            self.fields.push(field);
        }
        if data_len.is_some() {
            return Err(DecodeError::BadStructure);
        }
        let message = Message {
            bytes,
            fields: &self.fields,
        };
        if !message.has_valid_structure() {
            return Err(DecodeError::BadStructure);
        }
        Ok(message)
    }
}

fn read_field(
    bytes: &[u8],
    start: usize,
    data_len: Option<(u32, usize)>,
) -> Result<Field, DecodeError> {
    let position = u32::try_from(start).unwrap_or(u32::MAX);
    let rest = &bytes[start..];
    let equals = rest
        .iter()
        .take_while(|byte| **byte != SOH)
        .position(|byte| *byte == b'=')
        .ok_or(DecodeError::MissingEquals { position })?;
    let tag = parse_u64(&rest[..equals])
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or(DecodeError::BadTag { position })?;
    let value_start = start + equals + 1;
    let value_end = match data_len {
        Some((data_tag, length)) if data_tag == tag => {
            let end = value_start + length;
            if bytes.get(end) != Some(&SOH) {
                return Err(DecodeError::BadDataLength { tag });
            }
            end
        }
        Some(_) => return Err(DecodeError::BadDataLength { tag }),
        None => bytes[value_start..]
            .iter()
            .position(|byte| *byte == SOH)
            .map_or(bytes.len(), |offset| value_start + offset),
    };
    if value_end == value_start {
        return Err(DecodeError::EmptyValue { tag });
    }
    if value_end == bytes.len() {
        return Err(DecodeError::BadStructure);
    }
    let to_u32 = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    Ok(Field {
        tag,
        value_start: to_u32(value_start),
        value_end: to_u32(value_end),
    })
}

fn value_of<'a>(bytes: &'a [u8], field: &Field) -> &'a [u8] {
    &bytes[field.value_start as usize..field.value_end as usize]
}

/// A decoded message. It borrows the message bytes and the decoder.
#[derive(Debug, Clone, Copy)]
pub struct Message<'a> {
    bytes: &'a [u8],
    fields: &'a [Field],
}

impl<'a> Message<'a> {
    fn has_valid_structure(&self) -> bool {
        let n = self.fields.len();
        n >= 4
            && self.fields[0].tag == tag::BEGIN_STRING
            && self.fields[1].tag == tag::BODY_LENGTH
            && self.fields[2].tag == tag::MSG_TYPE
            && self.fields[n - 1].tag == tag::CHECK_SUM
    }

    /// The message bytes.
    pub fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }

    pub fn begin_string(&self) -> &'a [u8] {
        value_of(self.bytes, &self.fields[0])
    }

    pub fn msg_type(&self) -> &'a [u8] {
        value_of(self.bytes, &self.fields[2])
    }

    /// All fields after `BodyLength` and before `CheckSum`: the header (from
    /// `MsgType`), the body, and the trailer fields other than `CheckSum`.
    pub fn fields(&self) -> Fields<'a> {
        let n = self.fields.len();
        Fields {
            bytes: self.bytes,
            fields: &self.fields[2..n - 1],
        }
    }
}

/// A sequence of fields: a whole message, or one entry of a repeating group.
#[derive(Debug, Clone, Copy)]
pub struct Fields<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) fields: &'a [Field],
}

impl<'a> Fields<'a> {
    /// The value of the first field with `tag`. A tag inside a repeating
    /// group is also found; use [`Fields::group`] for group fields.
    pub fn get(&self, tag: u32) -> Option<&'a [u8]> {
        self.fields
            .iter()
            .find(|field| field.tag == tag)
            .map(|field| value_of(self.bytes, field))
    }

    pub fn get_u64(&self, tag: u32) -> Option<u64> {
        self.get(tag).and_then(parse_u64)
    }

    /// `Some(true)` for `Y`, `Some(false)` for `N` or when the field is absent,
    /// and `None` for any other value.
    pub fn get_bool(&self, tag: u32) -> Option<bool> {
        self.get(tag).map_or(Some(false), parse_bool)
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// The fields from position `count` on. With `count` at or above
    /// [`Fields::len`] it is empty. Use it to read a group from a cursor:
    /// [`Fields::group`] finds the first `NumInGroup` field of the slice.
    #[must_use]
    pub fn skip(&self, count: usize) -> Fields<'a> {
        Fields {
            bytes: self.bytes,
            fields: &self.fields[count.min(self.fields.len())..],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The fields in order, as `(tag, value)`.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &'a [u8])> + 'a {
        let bytes = self.bytes;
        self.fields
            .iter()
            .map(move |field| (field.tag, value_of(bytes, field)))
    }
}

/// Parses a non-negative decimal integer with at most 19 digits.
pub fn parse_u64(text: &[u8]) -> Option<u64> {
    if text.is_empty() || text.len() > 19 {
        return None;
    }
    text.iter().try_fold(0u64, |value, byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + u64::from(byte - b'0'))
    })
}

/// Parses a FIX Boolean: `Y` or `N`.
pub fn parse_bool(text: &[u8]) -> Option<bool> {
    match text {
        b"Y" => Some(true),
        b"N" => Some(false),
        _ => None,
    }
}
