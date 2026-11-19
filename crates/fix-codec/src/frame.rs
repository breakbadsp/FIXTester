/// The field separator.
pub const SOH: u8 = 0x01;

/// The trailer is `10=nnn<SOH>`: seven bytes.
const TRAILER_LEN: usize = 7;
/// A BeginString longer than this is not FIX.
const BEGIN_STRING_LEN_MAX: usize = 16;
/// BodyLength has at most this many digits.
const BODY_LENGTH_DIGITS_MAX: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// The stream does not start with `8=FIX`.
    BadBeginString,
    /// The second field is not a valid `9=<length>`.
    BadBodyLength,
    /// The bytes at the end of the body are not `10=nnn<SOH>`.
    BadTrailer,
    ChecksumMismatch {
        declared: u8,
        computed: u8,
    },
    /// The message is longer than the limit of the caller.
    TooLong,
}

/// Finds one complete message at the start of `buf`.
///
/// Returns `Ok(Some(len))` with the length of the message, or `Ok(None)` when
/// `buf` holds only part of a message. After an error, call [`skip_garbled`].
pub fn frame(buf: &[u8], len_max: usize) -> Result<Option<usize>, FrameError> {
    let Some(begin_end) = field_end(buf, 0, BEGIN_STRING_LEN_MAX + 3)? else {
        return Ok(None);
    };
    if !buf.starts_with(b"8=FIX") {
        return Err(FrameError::BadBeginString);
    }
    let length_start = begin_end + 1;
    let Some(length_end) = field_end(buf, length_start, BODY_LENGTH_DIGITS_MAX + 3)
        .map_err(|_| FrameError::BadBodyLength)?
    else {
        return Ok(None);
    };
    let length_field = &buf[length_start..length_end];
    let body_len = length_field
        .strip_prefix(b"9=")
        .and_then(crate::parse_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(FrameError::BadBodyLength)?;
    let body_start = length_end + 1;
    let trailer_start = body_start + body_len;
    let total = trailer_start + TRAILER_LEN;
    if total > len_max {
        return Err(FrameError::TooLong);
    }
    if buf.len() < total {
        return Ok(None);
    }
    let trailer = &buf[trailer_start..total];
    if !trailer.starts_with(b"10=") || trailer[TRAILER_LEN - 1] != SOH {
        return Err(FrameError::BadTrailer);
    }
    let declared = crate::parse_u64(&trailer[3..6])
        .and_then(|value| u8::try_from(value).ok())
        .ok_or(FrameError::BadTrailer)?;
    let computed = checksum(&buf[..trailer_start]);
    if declared != computed {
        return Err(FrameError::ChecksumMismatch { declared, computed });
    }
    debug_assert!(total <= buf.len());
    Ok(Some(total))
}

/// Returns the number of bytes to drop after a [`FrameError`]: up to the next
/// `8=FIX` that follows an SOH, or all of `buf` when there is none.
pub fn skip_garbled(buf: &[u8]) -> usize {
    let pattern = b"\x018=FIX";
    buf.windows(pattern.len())
        .skip(1)
        .position(|window| window == pattern)
        .map_or(buf.len(), |position| position + 2)
}

/// The FIX checksum: the sum of the bytes, modulo 256.
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))
}

/// Returns the index of the SOH that ends the field at `start`, or `None` when
/// the field is not complete. A field longer than `len_max` is an error.
fn field_end(buf: &[u8], start: usize, len_max: usize) -> Result<Option<usize>, FrameError> {
    let rest = buf.get(start..).unwrap_or_default();
    match rest.iter().take(len_max).position(|byte| *byte == SOH) {
        Some(offset) => Ok(Some(start + offset)),
        None if rest.len() >= len_max => Err(FrameError::BadBeginString),
        None => Ok(None),
    }
}
