use crate::decode::{Field, Fields};

/// The definition of a repeating group. The first member is the delimiter:
/// each entry starts with it.
#[derive(Debug, Clone, Copy)]
pub struct GroupDef<'d> {
    /// The `NumInGroup` tag that gives the number of entries.
    pub count_tag: u32,
    pub members: &'d [Member<'d>],
}

#[derive(Debug, Clone, Copy)]
pub enum Member<'d> {
    Field(u32),
    Group(&'d GroupDef<'d>),
}

impl GroupDef<'_> {
    fn delimiter(&self) -> Result<u32, GroupError> {
        match self.members.first() {
            Some(Member::Field(tag)) => Ok(*tag),
            _ => Err(GroupError::BadDefinition {
                count_tag: self.count_tag,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupError {
    /// The definition has no members, or its first member is a group.
    BadDefinition { count_tag: u32 },
    /// The `NumInGroup` value is not a number.
    BadCount { count_tag: u32 },
    /// The number of entries differs from the `NumInGroup` value.
    CountMismatch {
        count_tag: u32,
        declared: u64,
        found: u64,
    },
}

/// A repeating group in a message.
#[derive(Debug, Clone, Copy)]
pub struct Group<'a, 'd> {
    entries: Fields<'a>,
    def: &'d GroupDef<'d>,
    count: u64,
}

impl<'a, 'd> Group<'a, 'd> {
    pub fn len(&self) -> u64 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The entries in order. Each entry holds its fields and its nested groups.
    pub fn entries(&self) -> GroupEntries<'a, 'd> {
        GroupEntries {
            rest: self.entries,
            def: self.def,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GroupEntries<'a, 'd> {
    rest: Fields<'a>,
    def: &'d GroupDef<'d>,
}

impl<'a> Iterator for GroupEntries<'a, '_> {
    type Item = Fields<'a>;

    fn next(&mut self) -> Option<Fields<'a>> {
        if self.rest.fields.is_empty() {
            return None;
        }
        // Group::new checked every entry, so entry_len cannot fail here.
        let len = entry_len(self.rest.fields, self.rest.bytes, self.def).ok()?;
        let (entry, rest) = self.rest.fields.split_at(len);
        self.rest.fields = rest;
        Some(Fields {
            bytes: self.rest.bytes,
            fields: entry,
        })
    }
}

impl<'a> Fields<'a> {
    /// Reads the repeating group `def`. Returns `Ok(None)` when the message
    /// does not have its `NumInGroup` field.
    pub fn group<'d>(&self, def: &'d GroupDef<'d>) -> Result<Option<Group<'a, 'd>>, GroupError> {
        let Some(count_index) = self
            .fields
            .iter()
            .position(|field| field.tag == def.count_tag)
        else {
            return Ok(None);
        };
        let start = count_index + 1;
        let (count, len) = read_entries(&self.fields[count_index..], self.bytes, def)?;
        let entries = Fields {
            bytes: self.bytes,
            fields: &self.fields[start..start + len],
        };
        Ok(Some(Group {
            entries,
            def,
            count,
        }))
    }
}

/// Reads the `NumInGroup` field at `fields[0]` and the entries after it.
/// Returns the count and the number of fields in all entries.
fn read_entries(
    fields: &[Field],
    bytes: &[u8],
    def: &GroupDef,
) -> Result<(u64, usize), GroupError> {
    let count_tag = def.count_tag;
    let delimiter = def.delimiter()?;
    let declared = Fields {
        bytes,
        fields: &fields[..1],
    }
    .get_u64(count_tag)
    .ok_or(GroupError::BadCount { count_tag })?;
    let mut len = 0;
    let mut found = 0;
    while found < declared {
        let rest = &fields[1 + len..];
        if rest.first().map(|field| field.tag) != Some(delimiter) {
            return Err(GroupError::CountMismatch {
                count_tag,
                declared,
                found,
            });
        }
        len += entry_len(rest, bytes, def)?;
        found += 1;
    }
    if fields.get(1 + len).map(|field| field.tag) == Some(delimiter) {
        let extra = found + 1;
        return Err(GroupError::CountMismatch {
            count_tag,
            declared,
            found: extra,
        });
    }
    Ok((declared, len))
}

/// Returns the number of fields in the entry at the start of `fields`.
/// The entry ends before the next delimiter or the first tag that is not a
/// member.
fn entry_len(fields: &[Field], bytes: &[u8], def: &GroupDef) -> Result<usize, GroupError> {
    let delimiter = def.delimiter()?;
    debug_assert_eq!(fields.first().map(|field| field.tag), Some(delimiter));
    let mut index = 1;
    while let Some(field) = fields.get(index) {
        if field.tag == delimiter {
            break;
        }
        let member = def.members.iter().find(|member| match member {
            Member::Field(tag) => *tag == field.tag,
            Member::Group(nested) => nested.count_tag == field.tag,
        });
        match member {
            Some(Member::Field(_)) => index += 1,
            Some(Member::Group(nested)) => {
                let (_, nested_len) = read_entries(&fields[index..], bytes, nested)?;
                index += 1 + nested_len;
            }
            None => break,
        }
    }
    Ok(index)
}
