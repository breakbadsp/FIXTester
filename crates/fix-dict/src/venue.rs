//! A standard dictionary with a venue profile merged over it.

use crate::reach::ReachMap;
use crate::schema::{self, Dictionary, EnumValue, FieldDef, FieldType, MessageDef};
use crate::version::Version;
use fix_codec::GroupDef;

/// A field that a profile defines. The tag is not in the standard tables.
#[derive(Debug, Clone)]
pub(crate) struct CustomField {
    pub tag: u32,
    pub name: String,
    pub ty: FieldType,
    pub values: Vec<String>,
}

/// What a profile changes in one message type.
#[derive(Debug, Clone, Default)]
pub(crate) struct MessageOverride {
    pub msg_type: String,
    /// Fields that the venue requires. They are also allowed.
    pub required: Vec<u32>,
    pub forbidden: Vec<u32>,
    /// Optional fields that the venue adds.
    pub added: Vec<u32>,
    /// Groups that the profile defines. They replace a standard group with
    /// the same count tag.
    pub groups: Vec<&'static GroupDef<'static>>,
}

/// A group that a profile defines, by its name in the file.
#[derive(Debug, Clone)]
pub struct NamedGroup {
    pub name: String,
    pub def: &'static GroupDef<'static>,
}

/// The standard dictionary of one version, with the changes of one venue
/// profile. [`VenueDictionary::standard`] has no changes.
#[derive(Debug, Clone)]
pub struct VenueDictionary {
    pub(crate) version: Version,
    pub(crate) standard: &'static Dictionary,
    pub(crate) fields: Vec<CustomField>,
    /// Values that the profile adds to standard fields: `(tag, values)`.
    pub(crate) extra_values: Vec<(u32, Vec<String>)>,
    pub(crate) defaults: Vec<(u32, String)>,
    pub(crate) messages: Vec<MessageOverride>,
    pub(crate) groups: Vec<NamedGroup>,
    /// The tags of the profile groups. The standard groups are in
    /// [`ReachMap::standard`].
    pub(crate) reach: ReachMap,
}

/// A field of a venue dictionary: a standard field with the values that the
/// profile adds, or a field that the profile defines.
#[derive(Debug, Clone, Copy)]
pub struct FieldInfo<'a> {
    pub tag: u32,
    pub name: &'a str,
    pub ty: FieldType,
    standard_values: &'static [EnumValue],
    extra_values: &'a [String],
}

impl FieldInfo<'_> {
    /// Returns true when the field has a list of valid values.
    pub fn has_values(&self) -> bool {
        !self.standard_values.is_empty() || !self.extra_values.is_empty()
    }

    /// Returns true when the field has no list of values, or `value` is in it.
    pub fn accepts_value(&self, value: &str) -> bool {
        schema::values_accept(self.ty, self.standard_values, self.extra_values, value)
    }

    /// The valid values: the standard ones, then the ones from the profile.
    pub fn values(&self) -> impl Iterator<Item = &str> {
        let standard = self.standard_values.iter().map(|known| known.value);
        standard.chain(self.extra_values.iter().map(String::as_str))
    }
}

impl VenueDictionary {
    /// The standard dictionary of `version`, with no venue changes.
    pub fn standard(version: Version) -> Self {
        Self {
            version,
            standard: version.dictionary(),
            fields: Vec::new(),
            extra_values: Vec::new(),
            defaults: Vec::new(),
            messages: Vec::new(),
            groups: Vec::new(),
            reach: ReachMap::default(),
        }
    }

    /// Returns true when `tag` can be in an entry of `group`, also in a
    /// nested group. `group` comes from this dictionary.
    pub(crate) fn group_has_tag(&self, group: &GroupDef<'_>, tag: u32) -> bool {
        let found = self
            .reach
            .contains(group, tag)
            .or_else(|| ReachMap::standard(self.version).contains(group, tag));
        found.unwrap_or(false)
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn standard_dictionary(&self) -> &'static Dictionary {
        self.standard
    }

    pub fn field(&self, tag: u32) -> Option<FieldInfo<'_>> {
        if let Some(def) = self.standard.field(tag) {
            return Some(self.standard_info(def));
        }
        let custom = self.fields.iter().find(|field| field.tag == tag)?;
        Some(custom_info(custom))
    }

    pub fn field_by_name(&self, name: &str) -> Option<FieldInfo<'_>> {
        if let Some(def) = self.standard.field_by_name(name) {
            return Some(self.standard_info(def));
        }
        let custom = self.fields.iter().find(|field| field.name == name)?;
        Some(custom_info(custom))
    }

    fn standard_info(&self, def: &'static FieldDef) -> FieldInfo<'_> {
        let extra = self
            .extra_values
            .iter()
            .find(|(tag, _)| *tag == def.tag)
            .map_or(&[][..], |(_, values)| values.as_slice());
        FieldInfo {
            tag: def.tag,
            name: def.name,
            ty: def.ty,
            standard_values: def.values,
            extra_values: extra,
        }
    }

    /// The fields that the profile defines, in tag order.
    pub fn custom_fields(&self) -> impl Iterator<Item = FieldInfo<'_>> {
        self.fields.iter().map(custom_info)
    }

    /// The groups that the profile defines, in name order.
    pub fn profile_groups(&self) -> &[NamedGroup] {
        &self.groups
    }

    /// The default value that the profile gives to `tag`.
    pub fn profile_default(&self, tag: u32) -> Option<&str> {
        self.defaults
            .iter()
            .find(|(known, _)| *known == tag)
            .map(|(_, value)| value.as_str())
    }

    /// The tags and values of the profile defaults, in tag order.
    pub fn profile_defaults(&self) -> impl Iterator<Item = (u32, &str)> {
        self.defaults
            .iter()
            .map(|(tag, value)| (*tag, value.as_str()))
    }

    pub fn message(&self, msg_type: &str) -> Option<MessageView<'_>> {
        let def = self.standard.message(msg_type)?;
        let changes = self
            .messages
            .iter()
            .find(|changes| changes.msg_type == msg_type);
        Some(MessageView {
            standard: self.standard,
            def,
            changes,
        })
    }

    /// The message types that the profile changes, in order.
    pub fn changed_messages(&self) -> impl Iterator<Item = &str> {
        self.messages
            .iter()
            .map(|changes| changes.msg_type.as_str())
    }

    /// The group with `count_tag` in `msg_type`: the profile group when the
    /// profile has one, else the standard group.
    pub fn group(&self, msg_type: &str, count_tag: u32) -> Option<&'static GroupDef<'static>> {
        self.message(msg_type)?.group(count_tag)
    }
}

fn custom_info(custom: &CustomField) -> FieldInfo<'_> {
    FieldInfo {
        tag: custom.tag,
        name: &custom.name,
        ty: custom.ty,
        standard_values: &[],
        extra_values: &custom.values,
    }
}

/// One message type of a venue dictionary.
#[derive(Debug, Clone, Copy)]
pub struct MessageView<'a> {
    standard: &'static Dictionary,
    def: &'static MessageDef,
    changes: Option<&'a MessageOverride>,
}

impl MessageView<'_> {
    pub fn msg_type(&self) -> &'static str {
        self.def.msg_type
    }

    pub fn name(&self) -> &'static str {
        self.def.name
    }

    /// The tags that the profile requires in this message.
    pub fn profile_required(&self) -> &[u32] {
        self.changes.map_or(&[], |changes| &changes.required)
    }

    /// The tags that the profile forbids in this message.
    pub fn profile_forbidden(&self) -> &[u32] {
        self.changes.map_or(&[], |changes| &changes.forbidden)
    }

    /// The optional tags that the profile adds to this message.
    pub fn profile_added(&self) -> &[u32] {
        self.changes.map_or(&[], |changes| &changes.added)
    }

    fn changes_contain(&self, pick: impl Fn(&MessageOverride) -> &[u32], tag: u32) -> bool {
        self.changes
            .is_some_and(|changes| pick(changes).contains(&tag))
    }

    /// Returns true when the profile forbids `tag` in this message.
    pub fn is_forbidden(&self, tag: u32) -> bool {
        self.changes_contain(|changes| &changes.forbidden, tag)
    }

    /// Returns true when a message needs `tag`: the standard header, body,
    /// or trailer requires it, or the profile does.
    pub fn is_required(&self, tag: u32) -> bool {
        self.is_body_required(tag)
            || self.standard.header.field(tag) == Some(true)
            || self.standard.trailer.field(tag) == Some(true)
    }

    /// Like [`MessageView::is_required`], for the body only.
    pub fn is_body_required(&self, tag: u32) -> bool {
        !self.is_forbidden(tag)
            && (self.def.body.field(tag) == Some(true)
                || self.changes_contain(|changes| &changes.required, tag))
    }

    /// Returns true when the body can have `tag` at the top level.
    pub fn body_allows(&self, tag: u32) -> bool {
        !self.is_forbidden(tag)
            && (self.def.body.contains(tag)
                || self.changes_contain(|changes| &changes.required, tag)
                || self.changes_contain(|changes| &changes.added, tag))
    }

    /// Returns true when the header, the body, or the trailer can have `tag`
    /// at the top level.
    pub fn allows(&self, tag: u32) -> bool {
        self.body_allows(tag)
            || self.standard.header.contains(tag)
            || self.standard.trailer.contains(tag)
    }

    /// The tags that the message needs, in the order header, body, trailer,
    /// profile. The profile can add a tag that the standard also has.
    pub fn required_tags(&self) -> impl Iterator<Item = u32> + '_ {
        let standard = [
            &self.standard.header,
            &self.def.body,
            &self.standard.trailer,
        ]
        .into_iter()
        .flat_map(|section| section.required.iter().copied());
        let profile = self
            .changes
            .into_iter()
            .flat_map(|changes| changes.required.iter().copied());
        standard
            .chain(profile)
            .filter(|tag| !self.is_forbidden(*tag))
    }

    /// The group with `count_tag` at the top level of the body or the header.
    pub fn group(&self, count_tag: u32) -> Option<&'static GroupDef<'static>> {
        let profile = self.changes.and_then(|changes| {
            changes
                .groups
                .iter()
                .copied()
                .find(|group| group.count_tag == count_tag)
        });
        if self.is_forbidden(count_tag) {
            return None;
        }
        profile
            .or_else(|| self.def.body.group(count_tag))
            .or_else(|| self.standard.header.group(count_tag))
    }

    /// The groups at the top level of the body: the profile groups, then the
    /// standard groups that the profile does not replace or forbid.
    pub fn groups(&self) -> impl Iterator<Item = &'static GroupDef<'static>> + '_ {
        let profile: &[&'static GroupDef<'static>] = self
            .changes
            .map_or(&[], |changes| changes.groups.as_slice());
        let standard = self.def.body.groups.iter().copied().filter(move |group| {
            !profile.iter().any(|g| g.count_tag == group.count_tag)
                && !self.is_forbidden(group.count_tag)
        });
        profile.iter().copied().chain(standard)
    }
}
