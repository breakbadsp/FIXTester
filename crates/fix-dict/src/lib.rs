//! FIX 4.2, 4.3, and 4.4 data dictionaries, venue profiles, and message
//! validation.

#![forbid(unsafe_code)]

mod defaults;
mod generated;
mod profile;
mod reach;
mod report;
pub mod schema;
mod validate;
pub mod value;
mod venue;
mod version;

pub use defaults::{DefaultValue, missing_defaults};
pub use profile::{Profile, ProfileError, ProfileErrorKind};
pub use report::describe;
pub use validate::{ERRORS_MAX, Strictness, ValidationError, ValidationErrors, validate};
pub use venue::{FieldInfo, MessageView, NamedGroup, VenueDictionary};
pub use version::Version;
