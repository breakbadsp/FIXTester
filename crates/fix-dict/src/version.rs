use crate::generated::{fix42, fix43, fix44};
use crate::schema::Dictionary;

/// A FIX version that FIXSim supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Version {
    Fix42,
    Fix43,
    Fix44,
}

impl Version {
    pub const ALL: [Version; 3] = [Version::Fix42, Version::Fix43, Version::Fix44];

    /// The position in [`Version::ALL`].
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Fix42 => 0,
            Self::Fix43 => 1,
            Self::Fix44 => 2,
        }
    }

    /// The `BeginString` value: `FIX.4.2`, `FIX.4.3`, or `FIX.4.4`.
    pub fn begin_string(self) -> &'static str {
        match self {
            Self::Fix42 => "FIX.4.2",
            Self::Fix43 => "FIX.4.3",
            Self::Fix44 => "FIX.4.4",
        }
    }

    pub fn from_begin_string(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|version| version.begin_string() == text)
    }

    /// The standard tables of the version.
    pub fn dictionary(self) -> &'static Dictionary {
        match self {
            Self::Fix42 => &fix42::DICTIONARY,
            Self::Fix43 => &fix43::DICTIONARY,
            Self::Fix44 => &fix44::DICTIONARY,
        }
    }
}
