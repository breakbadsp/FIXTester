//! The types of the standard tables. The generated files in
//! `generated/` fill them. All tables are `'static`.

use fix_codec::{GroupDef, Member};

/// The FIX data type of a field, as the FIX Repository names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldType {
    String,
    Int,
    Float,
    Qty,
    Price,
    PriceOffset,
    Amt,
    Percentage,
    Char,
    Boolean,
    Length,
    SeqNum,
    NumInGroup,
    DayOfMonth,
    Currency,
    Exchange,
    Country,
    MultipleValueString,
    UtcTimestamp,
    UtcTimeOnly,
    UtcDate,
    LocalMktDate,
    MonthYear,
    Data,
}

impl FieldType {
    /// The name in venue profiles: the FIX Repository name in upper case.
    pub fn name(self) -> &'static str {
        match self {
            Self::String => "STRING",
            Self::Int => "INT",
            Self::Float => "FLOAT",
            Self::Qty => "QTY",
            Self::Price => "PRICE",
            Self::PriceOffset => "PRICEOFFSET",
            Self::Amt => "AMT",
            Self::Percentage => "PERCENTAGE",
            Self::Char => "CHAR",
            Self::Boolean => "BOOLEAN",
            Self::Length => "LENGTH",
            Self::SeqNum => "SEQNUM",
            Self::NumInGroup => "NUMINGROUP",
            Self::DayOfMonth => "DAYOFMONTH",
            Self::Currency => "CURRENCY",
            Self::Exchange => "EXCHANGE",
            Self::Country => "COUNTRY",
            Self::MultipleValueString => "MULTIPLEVALUESTRING",
            Self::UtcTimestamp => "UTCTIMESTAMP",
            Self::UtcTimeOnly => "UTCTIMEONLY",
            Self::UtcDate => "UTCDATE",
            Self::LocalMktDate => "LOCALMKTDATE",
            Self::MonthYear => "MONTHYEAR",
            Self::Data => "DATA",
        }
    }

    /// Finds a type by its profile name, in any letter case. The old
    /// names `UTCDATEONLY` and `MULTIPLESTRINGVALUE` also work.
    pub fn from_name(text: &str) -> Option<Self> {
        const ALL: [FieldType; 24] = [
            FieldType::String,
            FieldType::Int,
            FieldType::Float,
            FieldType::Qty,
            FieldType::Price,
            FieldType::PriceOffset,
            FieldType::Amt,
            FieldType::Percentage,
            FieldType::Char,
            FieldType::Boolean,
            FieldType::Length,
            FieldType::SeqNum,
            FieldType::NumInGroup,
            FieldType::DayOfMonth,
            FieldType::Currency,
            FieldType::Exchange,
            FieldType::Country,
            FieldType::MultipleValueString,
            FieldType::UtcTimestamp,
            FieldType::UtcTimeOnly,
            FieldType::UtcDate,
            FieldType::LocalMktDate,
            FieldType::MonthYear,
            FieldType::Data,
        ];
        let upper = text.to_ascii_uppercase();
        match upper.as_str() {
            "UTCDATEONLY" => return Some(Self::UtcDate),
            "MULTIPLESTRINGVALUE" => return Some(Self::MultipleValueString),
            _ => {}
        }
        ALL.into_iter().find(|ty| ty.name() == upper)
    }
}

/// A valid value of a field and its name, for example `("1", "Buy")`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumValue {
    pub value: &'static str,
    pub name: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct FieldDef {
    pub tag: u32,
    pub name: &'static str,
    pub ty: FieldType,
    /// The valid values. Empty when the field has no list of values.
    pub values: &'static [EnumValue],
}

/// A field of a message section, and whether it is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldRef {
    pub tag: u32,
    pub required: bool,
}

/// The fields and the repeating groups of a message body, the header, or
/// the trailer. A group adds its count tag to the fields. Both tag lists
/// are sorted.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub required: &'static [u32],
    pub optional: &'static [u32],
    pub groups: &'static [&'static GroupDef<'static>],
}

#[derive(Debug, Clone, Copy)]
pub struct MessageDef {
    pub msg_type: &'static str,
    pub name: &'static str,
    /// The body: the fields between the standard header and the trailer.
    pub body: Section,
}

/// The standard tables of one FIX version.
#[derive(Debug, Clone, Copy)]
pub struct Dictionary {
    pub begin_string: &'static str,
    /// Sorted by tag.
    pub fields: &'static [FieldDef],
    /// Sorted by message type.
    pub messages: &'static [MessageDef],
    pub header: Section,
    pub trailer: Section,
    /// `(length tag, data tag)`: the data field can contain SOH.
    pub data_pairs: &'static [(u32, u32)],
}

impl Dictionary {
    pub fn field(&self, tag: u32) -> Option<&'static FieldDef> {
        let fields: &'static [FieldDef] = self.fields;
        fields
            .binary_search_by_key(&tag, |field| field.tag)
            .ok()
            .map(|index| &fields[index])
    }

    pub fn field_by_name(&self, name: &str) -> Option<&'static FieldDef> {
        let fields: &'static [FieldDef] = self.fields;
        fields.iter().find(|field| field.name == name)
    }

    pub fn message(&self, msg_type: &str) -> Option<&'static MessageDef> {
        let messages: &'static [MessageDef] = self.messages;
        messages
            .binary_search_by_key(&msg_type, |message| message.msg_type)
            .ok()
            .map(|index| &messages[index])
    }

    /// The group with `count_tag` in the body of `msg_type`, or else in the
    /// standard header (for `NoHops`).
    pub fn group(&self, msg_type: &str, count_tag: u32) -> Option<&'static GroupDef<'static>> {
        let message = self.message(msg_type)?;
        message
            .body
            .group(count_tag)
            .or_else(|| self.header.group(count_tag))
    }
}

impl Section {
    pub fn group(&self, count_tag: u32) -> Option<&'static GroupDef<'static>> {
        let groups: &'static [&'static GroupDef<'static>] = self.groups;
        groups
            .iter()
            .copied()
            .find(|group| group.count_tag == count_tag)
    }

    /// Returns whether the section has `tag`: `Some(true)` when it is
    /// required.
    pub fn field(&self, tag: u32) -> Option<bool> {
        if self.required.binary_search(&tag).is_ok() {
            Some(true)
        } else if self.optional.binary_search(&tag).is_ok() {
            Some(false)
        } else {
            None
        }
    }

    pub fn contains(&self, tag: u32) -> bool {
        self.field(tag).is_some()
    }

    /// The fields: the required ones first, then the optional ones.
    pub fn fields(&self) -> impl Iterator<Item = FieldRef> {
        let required = self.required.iter().map(|tag| FieldRef {
            tag: *tag,
            required: true,
        });
        let optional = self.optional.iter().map(|tag| FieldRef {
            tag: *tag,
            required: false,
        });
        required.chain(optional)
    }
}

/// Constructors for the generated tables. They are `const fn`, so the
/// tables are short and stay `'static`.
pub mod build {
    use super::{EnumValue, FieldDef, FieldType, GroupDef, MessageDef, Section};
    use fix_codec::Member;

    pub const fn value(value: &'static str, name: &'static str) -> EnumValue {
        EnumValue { value, name }
    }

    pub const fn field(
        tag: u32,
        name: &'static str,
        ty: FieldType,
        values: &'static [EnumValue],
    ) -> FieldDef {
        FieldDef {
            tag,
            name,
            ty,
            values,
        }
    }

    pub const fn fld(tag: u32) -> Member<'static> {
        Member::Field(tag)
    }

    pub const fn grp(group: &'static GroupDef<'static>) -> Member<'static> {
        Member::Group(group)
    }

    pub const fn section(
        required: &'static [u32],
        optional: &'static [u32],
        groups: &'static [&'static GroupDef<'static>],
    ) -> Section {
        Section {
            required,
            optional,
            groups,
        }
    }

    pub const fn message(msg_type: &'static str, name: &'static str, body: Section) -> MessageDef {
        MessageDef {
            msg_type,
            name,
            body,
        }
    }
}

/// Returns the tag of the group delimiter, or `None` when the first member
/// is not a field.
pub fn delimiter(group: &GroupDef<'_>) -> Option<u32> {
    match group.members.first() {
        Some(Member::Field(tag)) => Some(*tag),
        _ => None,
    }
}

impl FieldDef {
    /// Returns true when the field has no list of values, or `value` is in it.
    /// For `MultipleValueString`, each word must be in the list.
    pub fn accepts_value(&self, value: &str) -> bool {
        values_accept(self.ty, self.values, &[], value)
    }
}

impl Dictionary {
    /// Returns true when a standard group of this version has `count_tag`
    /// as its count tag, in the header or in any message, also nested.
    pub fn is_group_count(&self, count_tag: u32) -> bool {
        fn in_group(group: &GroupDef<'_>, count_tag: u32) -> bool {
            group.count_tag == count_tag
                || group.members.iter().any(|member| match member {
                    Member::Group(nested) => in_group(nested, count_tag),
                    Member::Field(_) => false,
                })
        }
        let sections = self
            .messages
            .iter()
            .map(|message| &message.body)
            .chain(std::iter::once(&self.header));
        sections
            .flat_map(|section| section.groups.iter())
            .any(|group| in_group(group, count_tag))
    }

    /// Returns true when `tag` is unknown, has no list of values, or accepts
    /// `value`. Use [`Dictionary::field`] to tell unknown tags apart.
    pub fn is_valid_value(&self, tag: u32, value: &str) -> bool {
        self.field(tag)
            .is_none_or(|field| field.accepts_value(value))
    }
}

/// The value check that standard fields and venue fields share. `extra` are
/// the values that a venue profile adds.
pub(crate) fn values_accept(
    ty: FieldType,
    standard: &[EnumValue],
    extra: &[String],
    value: &str,
) -> bool {
    if standard.is_empty() && extra.is_empty() {
        return true;
    }
    let known = |word: &str| {
        standard.iter().any(|known| known.value == word) || extra.iter().any(|known| known == word)
    };
    if ty == FieldType::MultipleValueString {
        value.split(' ').all(known)
    } else {
        known(value)
    }
}
