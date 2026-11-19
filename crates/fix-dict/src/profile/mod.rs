//! Venue profiles: flat TOML that changes the standard dictionary. See
//! `docs/requirements.md`, section 8.

mod error;
mod fields;
mod groups;
mod loops;
mod messages;
mod raw;

pub use error::{ProfileError, ProfileErrorKind};

use crate::reach::ReachMap;
use crate::venue::VenueDictionary;
use crate::version::Version;
use error::LineIndex;
use raw::{RawProfile, Ref};

/// The most fields, group blocks, members of a group, and values of a field
/// that a profile can have. A larger profile is an error, not a slowdown.
pub const FIELDS_MAX: usize = 256;
pub const GROUPS_MAX: usize = 64;
pub const MEMBERS_MAX: usize = 64;
pub const VALUES_MAX: usize = 1024;

/// What a name or a tag in a profile stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Field(u32),
    /// The name of a `[group.<name>]` block.
    Group(String),
}

/// Loads venue profiles.
#[derive(Debug, Clone, Copy)]
pub struct Profile;

impl Profile {
    /// Reads a profile and merges it over the standard dictionary of its
    /// version. It reports every error that it finds, in file order.
    ///
    /// # Leaked memory
    ///
    /// `fix_codec::GroupDef` borrows nested groups, so the groups of a
    /// profile cannot live in the `VenueDictionary` without a self-reference.
    /// A successful load leaks them (`Box::leak`) to get `'static` values.
    /// The size is bounded: at most [`GROUPS_MAX`] groups with at most
    /// [`MEMBERS_MAX`] members each, so below 64 KiB for each load. A failed
    /// load leaks nothing. Load profiles at start or on a rare user edit,
    /// not for each message.
    pub fn load(text: &str) -> Result<VenueDictionary, Vec<ProfileError>> {
        let lines = LineIndex::new(text);
        let raw: RawProfile = match toml::from_str(text) {
            Ok(raw) => raw,
            Err(error) => {
                let offset = error.span().map_or(0, |span| span.start);
                let (line, column) = lines.locate(offset);
                return Err(vec![ProfileError {
                    line,
                    column,
                    kind: ProfileErrorKind::Syntax(error.message().to_owned()),
                }]);
            }
        };
        let Some(version) = Version::from_begin_string(raw.version.get_ref()) else {
            let (line, column) = lines.locate(raw.version.span().start);
            let kind = ProfileErrorKind::UnknownVersion(raw.version.get_ref().clone());
            return Err(vec![ProfileError { line, column, kind }]);
        };
        let mut loader = Loader {
            lines,
            raw: &raw,
            venue: VenueDictionary::standard(version),
            errors: Vec::new(),
        };
        loader.load_fields();
        loader.load_defaults();
        let planned = loader.plan_groups();
        let pending = loader.load_messages(&planned);
        loader.finish(&planned, pending)
    }
}

struct Loader<'t> {
    lines: LineIndex<'t>,
    raw: &'t RawProfile,
    venue: VenueDictionary,
    errors: Vec<ProfileError>,
}

impl Loader<'_> {
    fn error(&mut self, offset: usize, kind: ProfileErrorKind) {
        let (line, column) = self.lines.locate(offset);
        self.errors.push(ProfileError { line, column, kind });
    }

    /// Finds what `reference` stands for. A name that is a group block wins
    /// over a field name. With `count_is_field`, the name of a repeating
    /// group count field without a block stands for the count field (to
    /// require or forbid it); else it is an unknown group.
    fn target(&self, reference: &Ref, count_is_field: bool) -> Result<Target, ProfileErrorKind> {
        match reference {
            Ref::Tag(tag) => self
                .venue
                .field(*tag)
                .map(|info| Target::Field(info.tag))
                .ok_or(ProfileErrorKind::UnknownTag(*tag)),
            Ref::Name(name) if self.raw.group.contains_key(name) => Ok(Target::Group(name.clone())),
            Ref::Name(name) => match self.venue.field_by_name(name) {
                Some(info)
                    if info.ty == crate::schema::FieldType::NumInGroup && !count_is_field =>
                {
                    Err(ProfileErrorKind::UnknownGroup(name.clone()))
                }
                Some(info) => Ok(Target::Field(info.tag)),
                None => Err(ProfileErrorKind::UnknownField(name.clone())),
            },
        }
    }

    /// Finds a field by tag or by name. A group block of the same name is
    /// not a field, so it is not considered.
    fn field_tag(&self, reference: &Ref) -> Result<u32, ProfileErrorKind> {
        match reference {
            Ref::Tag(tag) => self
                .venue
                .field(*tag)
                .map(|info| info.tag)
                .ok_or(ProfileErrorKind::UnknownTag(*tag)),
            Ref::Name(name) => self
                .venue
                .field_by_name(name)
                .map(|info| info.tag)
                .ok_or_else(|| ProfileErrorKind::UnknownField(name.clone())),
        }
    }

    fn finish(
        mut self,
        planned: &groups::Planned,
        pending: Vec<messages::Pending>,
    ) -> Result<VenueDictionary, Vec<ProfileError>> {
        if !self.errors.is_empty() {
            self.errors.sort_by_key(|error| (error.line, error.column));
            return Err(self.errors);
        }
        let built = planned.build();
        self.venue.messages = pending
            .into_iter()
            .map(|message| message.finish(&built))
            .collect();
        self.venue
            .messages
            .sort_by(|a, b| a.msg_type.cmp(&b.msg_type));
        self.venue.reach = ReachMap::build(built.defs());
        self.venue.groups = built.named();
        Ok(self.venue)
    }
}
