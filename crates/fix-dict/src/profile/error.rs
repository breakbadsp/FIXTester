use crate::schema::FieldType;
use std::fmt;

/// An error in a venue profile, with the place in the file (1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError {
    pub line: u32,
    pub column: u32,
    pub kind: ProfileErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileErrorKind {
    /// The file is not valid TOML, or a key has the wrong type or is unknown.
    Syntax(String),
    UnknownVersion(String),
    UnknownField(String),
    UnknownTag(u32),
    /// A name that is a repeating group count field, but the profile has no
    /// `[group.<name>]` block for it.
    UnknownGroup(String),
    UnknownMessage(String),
    UnknownType(String),
    /// A new field needs `name` and `type`.
    MissingKey(&'static str),
    /// The field has the tag or the name of another field, or it changes the
    /// name or the type of a standard field.
    FieldConflict {
        tag: u32,
        name: String,
    },
    DuplicateField(u32),
    /// The groups in the list include each other. The names are in order.
    GroupLoop(Vec<String>),
    DuplicateInGroup {
        group: String,
        member: String,
    },
    GroupWithoutMembers(String),
    GroupStartsWithGroup(String),
    CountNotNumInGroup {
        group: String,
        tag: u32,
    },
    RequiredAndForbidden {
        msg_type: String,
        tag: u32,
    },
    AddedAndForbidden {
        msg_type: String,
        tag: u32,
    },
    /// The default is not in the list of valid values of its field.
    EnumDefaultInvalid {
        tag: u32,
        value: String,
    },
    /// A default or a value has not the syntax of the field type.
    ValueNotOfType {
        tag: u32,
        value: String,
        ty: FieldType,
    },
    /// A fixed limit of the loader is full.
    TooMany(&'static str),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.kind)
    }
}

impl std::error::Error for ProfileError {}

impl fmt::Display for ProfileErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(text) => write!(f, "{text}"),
            Self::UnknownVersion(text) => {
                write!(
                    f,
                    "unknown version \"{text}\", use FIX.4.2, FIX.4.3, or FIX.4.4"
                )
            }
            Self::UnknownField(name) => write!(f, "unknown field name \"{name}\""),
            Self::UnknownTag(tag) => write!(f, "unknown tag {tag}: add a [[field]] for it"),
            Self::UnknownGroup(name) => {
                write!(f, "unknown group \"{name}\": add a [group.{name}] block")
            }
            Self::UnknownMessage(msg_type) => write!(f, "unknown message type \"{msg_type}\""),
            Self::UnknownType(text) => write!(f, "unknown field type \"{text}\""),
            Self::MissingKey(key) => write!(f, "a new field needs the key \"{key}\""),
            Self::FieldConflict { tag, name } => {
                write!(f, "field {tag} \"{name}\" conflicts with another field")
            }
            Self::DuplicateField(tag) => write!(f, "field {tag} is defined twice"),
            Self::GroupLoop(names) => write!(f, "group loop: {}", names.join(" -> ")),
            Self::DuplicateInGroup { group, member } => {
                write!(f, "group \"{group}\" has the member {member} twice")
            }
            Self::GroupWithoutMembers(name) => write!(f, "group \"{name}\" has no members"),
            Self::GroupStartsWithGroup(name) => write!(
                f,
                "group \"{name}\" starts with a group, but the first member is the delimiter and must be a field"
            ),
            Self::CountNotNumInGroup { group, tag } => write!(
                f,
                "the count tag {tag} of group \"{group}\" is not a NUMINGROUP field"
            ),
            Self::RequiredAndForbidden { msg_type, tag } => {
                write!(
                    f,
                    "message {msg_type}: tag {tag} is both required and forbidden"
                )
            }
            Self::AddedAndForbidden { msg_type, tag } => {
                write!(
                    f,
                    "message {msg_type}: tag {tag} is both added and forbidden"
                )
            }
            Self::EnumDefaultInvalid { tag, value } => {
                write!(
                    f,
                    "the default \"{value}\" is not a valid value of tag {tag}"
                )
            }
            Self::ValueNotOfType { tag, value, ty } => {
                write!(
                    f,
                    "\"{value}\" is not a valid {} value (tag {tag})",
                    ty.name()
                )
            }
            Self::TooMany(what) => write!(f, "too many {what}"),
        }
    }
}

/// Converts byte offsets to line and column numbers.
#[derive(Debug)]
pub(crate) struct LineIndex<'t> {
    text: &'t str,
}

impl<'t> LineIndex<'t> {
    pub fn new(text: &'t str) -> Self {
        Self { text }
    }

    /// The 1-based line and column (in characters) of a byte offset.
    pub fn locate(&self, offset: usize) -> (u32, u32) {
        assert!(
            self.text.is_char_boundary(offset.min(self.text.len())),
            "spans are at characters"
        );
        let end = offset.min(self.text.len());
        let before = self.text.get(..end).unwrap_or_default();
        let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let start = before.rfind('\n').map_or(0, |index| index + 1);
        let column = before[start..].chars().count() + 1;
        (
            u32::try_from(line).unwrap_or(u32::MAX),
            u32::try_from(column).unwrap_or(u32::MAX),
        )
    }
}
