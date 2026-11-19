//! Message validation against a venue dictionary.

use crate::schema::FieldType;
use crate::value::matches_type;
use crate::venue::{MessageView, VenueDictionary};
use fix_codec::tag::{BEGIN_STRING, BODY_LENGTH, CHECK_SUM};
use fix_codec::{Fields, GroupDef, GroupError, Member, Message};
use std::fmt;

/// The most errors that a [`ValidationErrors`] buffer keeps. [`validate`]
/// counts the others.
pub const ERRORS_MAX: usize = 64;

/// How strict the validation is. A session has one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strictness {
    /// Reject a tag that the message type does not list (in the standard or
    /// in the profile), also when the dictionary knows the tag.
    pub reject_unlisted_fields: bool,
    /// Reject a tag that the dictionary does not know at all.
    pub reject_unknown_tags: bool,
}

impl Strictness {
    /// Accepts unknown and unlisted tags. It still checks required fields,
    /// profile `forbidden` fields, values, types, and groups.
    pub const LENIENT: Self = Self {
        reject_unlisted_fields: false,
        reject_unknown_tags: false,
    };
    pub const STRICT: Self = Self {
        reject_unlisted_fields: true,
        reject_unknown_tags: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    /// The message type is not in the dictionary.
    UnknownMessageType,
    MissingRequired {
        tag: u32,
    },
    /// The dictionary knows the tag, but the message type does not list it.
    FieldNotAllowed {
        tag: u32,
    },
    /// The venue profile forbids the tag in this message type.
    ForbiddenField {
        tag: u32,
    },
    UnknownTag {
        tag: u32,
    },
    /// The field has a list of valid values and the value is not in it.
    ValueNotInEnum {
        tag: u32,
    },
    ValueNotOfType {
        tag: u32,
        ty: FieldType,
    },
    /// The `NumInGroup` value and the entries do not agree, or the group is
    /// malformed.
    Group(GroupError),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownMessageType => write!(f, "unknown message type"),
            Self::MissingRequired { tag } => write!(f, "required tag {tag} is missing"),
            Self::FieldNotAllowed { tag } => write!(f, "tag {tag} is not allowed in this message"),
            Self::ForbiddenField { tag } => {
                write!(f, "tag {tag} is forbidden by the venue profile")
            }
            Self::UnknownTag { tag } => write!(f, "unknown tag {tag}"),
            Self::ValueNotInEnum { tag } => {
                write!(f, "the value of tag {tag} is not a valid value")
            }
            Self::ValueNotOfType { tag, ty } => {
                write!(f, "the value of tag {tag} is not a valid {}", ty.name())
            }
            Self::Group(error) => write!(f, "repeating group error: {error:?}"),
        }
    }
}

impl std::error::Error for ValidationError {}

/// A fixed-capacity buffer for the errors of [`validate`]. The caller makes
/// it once and uses it for each message, so a rejected message does not
/// allocate.
#[derive(Debug, Clone)]
pub struct ValidationErrors {
    items: [ValidationError; ERRORS_MAX],
    len: usize,
    extra: u32,
}

impl Default for ValidationErrors {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationErrors {
    pub fn new() -> Self {
        Self {
            items: [ValidationError::UnknownMessageType; ERRORS_MAX],
            len: 0,
            extra: 0,
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.extra = 0;
    }

    /// The first [`ERRORS_MAX`] errors, in the order of discovery.
    pub fn as_slice(&self) -> &[ValidationError] {
        &self.items[..self.len]
    }

    /// The number of errors that did not fit in the buffer.
    pub fn extra(&self) -> u32 {
        self.extra
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn push(&mut self, error: ValidationError) {
        assert!(self.len <= ERRORS_MAX, "the length stays in the buffer");
        if self.len == ERRORS_MAX {
            self.extra = self.extra.saturating_add(1);
        } else {
            self.items[self.len] = error;
            self.len += 1;
        }
    }
}

/// Where a field is: at the top level of a message, or in a group entry.
#[derive(Clone, Copy)]
enum Scope<'a> {
    Message(MessageView<'a>),
    Entry(&'static GroupDef<'static>),
}

impl Scope<'_> {
    fn group(&self, count_tag: u32) -> Option<&'static GroupDef<'static>> {
        match self {
            Self::Message(view) => view.group(count_tag),
            Self::Entry(def) => def.members.iter().find_map(|member| match member {
                Member::Group(nested) if nested.count_tag == count_tag => Some(*nested),
                _ => None,
            }),
        }
    }
}

struct Checker<'a> {
    venue: &'a VenueDictionary,
    strictness: Strictness,
    errors: &'a mut ValidationErrors,
}

impl Checker<'_> {
    fn push(&mut self, error: ValidationError) {
        self.errors.push(error);
    }

    /// Checks one field. Returns false when the value has the wrong type.
    fn check_field(&mut self, scope: Scope<'_>, tag: u32, value: &[u8]) -> bool {
        let Some(info) = self.venue.field(tag) else {
            if self.strictness.reject_unknown_tags {
                self.push(ValidationError::UnknownTag { tag });
            }
            return true;
        };
        if let Scope::Message(view) = scope {
            if view.is_forbidden(tag) {
                self.push(ValidationError::ForbiddenField { tag });
                return true;
            }
            if self.strictness.reject_unlisted_fields && !view.allows(tag) {
                self.push(ValidationError::FieldNotAllowed { tag });
            }
        }
        if !matches_type(info.ty, value) {
            self.push(ValidationError::ValueNotOfType { tag, ty: info.ty });
            return false;
        }
        // Only a field with a list of values is text. Data fields are bytes.
        let in_list = !info.has_values()
            || std::str::from_utf8(value).is_ok_and(|text| info.accepts_value(text));
        if !in_list {
            self.push(ValidationError::ValueNotInEnum { tag });
        }
        true
    }

    /// Checks the fields of a message or of a group entry. A group inside
    /// them is checked entry by entry. The scan has a cursor, so a group is
    /// read where it is, also when its count tag appears more than once.
    fn walk(&mut self, scope: Scope<'_>, fields: Fields<'_>) {
        let mut index = 0;
        while let Some((tag, value)) = field_at(fields, index) {
            index += 1;
            let type_ok = self.check_field(scope, tag, value);
            let Some(def) = scope.group(tag) else {
                continue;
            };
            match fields.skip(index - 1).group(def) {
                Ok(Some(group)) => {
                    for entry in group.entries() {
                        self.walk(Scope::Entry(def), entry);
                        index += entry.len();
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    // A count that is not a number already has a type error.
                    if type_ok || !matches!(error, GroupError::BadCount { .. }) {
                        self.push(ValidationError::Group(error));
                    }
                    // The entries are not reliable. Do not check their fields
                    // as fields of the message.
                    while let Some((next, _)) = field_at(fields, index) {
                        if !self.venue.group_has_tag(def, next) {
                            break;
                        }
                        index += 1;
                    }
                }
            }
        }
    }
}

/// The field at position `index`.
fn field_at(fields: Fields<'_>, index: usize) -> Option<(u32, &[u8])> {
    fields.skip(index).iter().next()
}

/// Returns true when `tag` is a top-level field of `fields`. A field inside
/// a repeating group does not count.
fn has_top_level(view: &MessageView<'_>, fields: Fields<'_>, tag: u32) -> bool {
    let mut index = 0;
    while let Some((next, _)) = field_at(fields, index) {
        if next == tag {
            return true;
        }
        index += 1;
        let group = view
            .group(next)
            .and_then(|def| fields.skip(index - 1).group(def).ok().flatten());
        if let Some(group) = group {
            index += group.entries().map(|entry| entry.len()).sum::<usize>();
        }
    }
    false
}

/// Checks a decoded message against the venue dictionary.
///
/// It clears `errors` and fills it with the errors that it finds. It
/// allocates nothing. `BeginString`, `BodyLength`, and `CheckSum` are not
/// checked: the decoder has checked that they are present. A required tag
/// counts as present only at the top level of the message, not inside a
/// group. The members of a group entry are not checked for required fields.
pub fn validate<'e>(
    venue: &VenueDictionary,
    message: &Message<'_>,
    strictness: Strictness,
    errors: &'e mut ValidationErrors,
) -> Result<(), &'e ValidationErrors> {
    errors.clear();
    let view = std::str::from_utf8(message.msg_type())
        .ok()
        .and_then(|msg_type| venue.message(msg_type));
    let Some(view) = view else {
        errors.push(ValidationError::UnknownMessageType);
        return Err(errors);
    };
    let fields = message.fields();
    let mut checker = Checker {
        venue,
        strictness,
        errors,
    };
    for tag in view.required_tags() {
        let structural = matches!(tag, BEGIN_STRING | BODY_LENGTH | CHECK_SUM);
        if !structural && !has_top_level(&view, fields, tag) {
            checker.push(ValidationError::MissingRequired { tag });
        }
    }
    checker.walk(Scope::Message(view), fields);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
