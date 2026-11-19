//! Default values for the fields that a user does not enter (R29).

use crate::venue::VenueDictionary;

/// A default value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultValue<'a> {
    /// A fixed text.
    Text(&'a str),
    /// The current UTC time. The caller fills it: this crate has no clock.
    Time,
}

/// `HandlInst`, `TimeInForce`, and `TransactTime`.
const HANDL_INST: u32 = 21;
const TIME_IN_FORCE: u32 = 59;
const TRANSACT_TIME: u32 = 60;

/// Standard defaults. They apply to a message that requires the field.
const STANDARD: [(u32, DefaultValue<'static>); 3] = [
    (HANDL_INST, DefaultValue::Text("1")),
    // Day. The standard tables do not require TimeInForce, so this default
    // applies when a venue profile requires it.
    (TIME_IN_FORCE, DefaultValue::Text("0")),
    (TRANSACT_TIME, DefaultValue::Time),
];

/// The default for each field that the message type needs or allows and
/// that is not in `present_tags`.
///
/// - A default from the venue profile applies to each field that the body
///   of the message allows, also when the standard does not require it.
/// - A standard default (`HandlInst` is 1, `TimeInForce` is 0 (Day),
///   `TransactTime` is the time) applies only when the venue dictionary requires the field in the body.
///   A profile default for the same tag replaces it.
///
/// The header and trailer fields are not here: the session layer sets them.
/// The result lists the profile defaults first, in tag order. An unknown
/// message type gives nothing.
pub fn missing_defaults<'a>(
    venue: &'a VenueDictionary,
    msg_type: &str,
    present_tags: &'a [u32],
) -> impl Iterator<Item = (u32, DefaultValue<'a>)> + 'a {
    let view = venue.message(msg_type);
    let profile = venue
        .profile_defaults()
        .map(|(tag, text)| (tag, DefaultValue::Text(text)))
        .filter(move |(tag, _)| view.is_some_and(|view| view.body_allows(*tag)));
    let standard = STANDARD
        .into_iter()
        .filter(move |(tag, _)| venue.profile_default(*tag).is_none())
        .filter(move |(tag, _)| view.is_some_and(|view| view.is_body_required(*tag)));
    profile
        .chain(standard)
        .filter(move |(tag, _)| !present_tags.contains(tag))
}
