//! FIX tag=value codec for FIX 4.2, 4.3, and 4.4.
//!
//! The codec does no I/O and reads no clock. [`frame`] finds one complete
//! message in a byte stream, [`Decoder`] splits it into fields without a
//! copy, [`Fields::group`] reads repeating groups from a [`GroupDef`], and
//! [`Encoder`] writes a message with its `BodyLength` and `CheckSum`.

mod decode;
mod encode;
mod frame;
mod group;
pub mod tag;
mod time;

pub use decode::{DecodeError, Decoder, Field, Fields, Message, parse_bool, parse_u64};
pub use encode::{Body, Encoder, TooLarge};
pub use frame::{FrameError, Framed, SOH, frame, frame_with_checksum, skip_garbled};
pub use group::{Group, GroupDef, GroupEntries, GroupError, Member};
pub use time::{UtcMillis, parse_utc_timestamp, write_utc_timestamp};
