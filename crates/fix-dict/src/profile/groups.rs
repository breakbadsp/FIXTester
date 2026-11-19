//! Repeating groups of a profile: the checks, the loop search, and the
//! build of `fix_codec::GroupDef` values.

use super::raw::RawGroup;
use super::{GROUPS_MAX, Loader, MEMBERS_MAX, ProfileErrorKind, Target};
use crate::schema::FieldType;
use crate::venue::NamedGroup;
use fix_codec::{GroupDef, Member};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlannedMember {
    Field(u32),
    /// An index into [`Planned::groups`].
    Group(usize),
}

#[derive(Debug)]
struct PlannedGroup {
    name: String,
    /// `None` when the count of the block is wrong.
    count_tag: Option<u32>,
    /// False when the block has any error. The block is still in the plan,
    /// so the loop search sees every group.
    valid: bool,
    members: Vec<(PlannedMember, usize)>,
}

/// The groups of a profile after the checks. Names are in order.
#[derive(Debug, Default)]
pub(super) struct Planned {
    groups: Vec<PlannedGroup>,
}

/// The built groups, in the order of [`Planned::groups`].
#[derive(Debug)]
pub(super) struct Built {
    names: Vec<String>,
    defs: Vec<&'static GroupDef<'static>>,
}

impl Built {
    pub fn named(&self) -> Vec<NamedGroup> {
        let pairs = self.names.iter().zip(&self.defs);
        pairs
            .map(|(name, def)| NamedGroup {
                name: name.clone(),
                def,
            })
            .collect()
    }

    pub fn defs(&self) -> impl Iterator<Item = &'static GroupDef<'static>> + '_ {
        self.defs.iter().copied()
    }

    pub fn def(&self, name: &str) -> Option<&'static GroupDef<'static>> {
        let index = self.names.iter().position(|known| known == name)?;
        Some(self.defs[index])
    }
}

impl Planned {
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    pub fn name(&self, index: usize) -> &str {
        &self.groups[index].name
    }

    /// The nested groups of a group: `(group index, place of the member)`.
    pub fn nested(&self, index: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.groups[index]
            .members
            .iter()
            .filter_map(|(member, start)| match member {
                PlannedMember::Group(nested) => Some((*nested, *start)),
                PlannedMember::Field(_) => None,
            })
    }

    /// The count tag of the group `name`, if the group is valid.
    pub fn count_tag(&self, name: &str) -> Option<u32> {
        self.groups
            .iter()
            .find(|group| group.name == name && group.valid)
            .and_then(|group| group.count_tag)
    }

    /// Builds the groups, the nested ones first. Call it only when the
    /// checks found no error (so there is no loop, and each group has a
    /// field as its first member). The memory leaks, see `Profile::load`.
    pub fn build(&self) -> Built {
        let mut defs: Vec<Option<&'static GroupDef<'static>>> = vec![None; self.groups.len()];
        for index in 0..self.groups.len() {
            self.build_one(index, &mut defs);
        }
        let defs: Vec<&'static GroupDef<'static>> = defs.into_iter().flatten().collect();
        assert_eq!(defs.len(), self.groups.len(), "each group has a definition");
        Built {
            names: self.groups.iter().map(|group| group.name.clone()).collect(),
            defs,
        }
    }

    fn build_one(
        &self,
        index: usize,
        defs: &mut [Option<&'static GroupDef<'static>>],
    ) -> Option<&'static GroupDef<'static>> {
        assert!(index < self.groups.len(), "a nested group is in the plan");
        if defs[index].is_some() {
            return defs[index];
        }
        let group = &self.groups[index];
        let count_tag = group.count_tag?;
        let mut members: Vec<Member<'static>> = Vec::with_capacity(group.members.len());
        for (member, _) in &group.members {
            members.push(match member {
                PlannedMember::Field(tag) => Member::Field(*tag),
                PlannedMember::Group(nested) => Member::Group(self.build_one(*nested, defs)?),
            });
        }
        // The leak is bounded by GROUPS_MAX and MEMBERS_MAX. See `Profile::load`.
        let members: &'static [Member<'static>] = Box::leak(members.into_boxed_slice());
        let def: &'static GroupDef<'static> = Box::leak(Box::new(GroupDef { count_tag, members }));
        defs[index] = Some(def);
        Some(def)
    }
}

impl Loader<'_> {
    /// Checks all `[group.<name>]` blocks.
    pub(super) fn plan_groups(&mut self) -> Planned {
        let raw = self.raw;
        if raw.group.len() > GROUPS_MAX {
            self.error(0, ProfileErrorKind::TooMany("group blocks"));
            return Planned::default();
        }
        let names: Vec<&String> = raw.group.keys().collect();
        let counts: Vec<Option<u32>> = raw
            .group
            .iter()
            .map(|(name, group)| self.count_tag(name, group.get_ref()))
            .collect();
        let mut planned = Planned::default();
        for (index, (name, group)) in raw.group.iter().enumerate() {
            let (members, members_valid) =
                self.plan_members(name, group.get_ref(), group.span().start, &names, &counts);
            planned.groups.push(PlannedGroup {
                name: name.clone(),
                count_tag: counts[index],
                valid: members_valid && counts[index].is_some(),
                members,
            });
        }
        self.find_loops(&planned);
        planned
    }

    fn count_tag(&mut self, name: &str, group: &RawGroup) -> Option<u32> {
        let start = group.count.span().start;
        let tag = match self.field_tag(group.count.get_ref()) {
            Ok(tag) => tag,
            Err(kind) => {
                self.error(start, kind);
                return None;
            }
        };
        // FIX 4.2 types its count fields as Int, so a count tag that the
        // version uses in a standard group is also valid.
        let is_count = self.venue.field(tag)?.ty == FieldType::NumInGroup
            || self.venue.standard.is_group_count(tag);
        if !is_count {
            let kind = ProfileErrorKind::CountNotNumInGroup {
                group: name.to_owned(),
                tag,
            };
            self.error(start, kind);
            return None;
        }
        Some(tag)
    }

    /// Resolves the members of one group. `None` when a member is wrong.
    fn plan_members(
        &mut self,
        name: &str,
        group: &RawGroup,
        group_start: usize,
        names: &[&String],
        counts: &[Option<u32>],
    ) -> (Vec<(PlannedMember, usize)>, bool) {
        if group.members.is_empty() {
            self.error(
                group_start,
                ProfileErrorKind::GroupWithoutMembers(name.to_owned()),
            );
            return (Vec::new(), false);
        }
        if group.members.len() > MEMBERS_MAX {
            self.error(group_start, ProfileErrorKind::TooMany("group members"));
            return (Vec::new(), false);
        }
        let mut members: Vec<(PlannedMember, usize)> = Vec::new();
        let mut keys: Vec<u32> = Vec::new();
        let mut valid = true;
        for member in &group.members {
            let start = member.span().start;
            let (planned, key) = match self.target(member.get_ref(), false) {
                Ok(Target::Field(tag)) => (PlannedMember::Field(tag), Some(tag)),
                Ok(Target::Group(nested)) => {
                    let Some(index) = names.iter().position(|known| **known == nested) else {
                        valid = false;
                        continue;
                    };
                    (PlannedMember::Group(index), counts[index])
                }
                Err(kind) => {
                    self.error(start, kind);
                    valid = false;
                    continue;
                }
            };
            if let Some(key) = key {
                if keys.contains(&key) {
                    let kind = ProfileErrorKind::DuplicateInGroup {
                        group: name.to_owned(),
                        member: key.to_string(),
                    };
                    self.error(start, kind);
                    valid = false;
                }
                keys.push(key);
            }
            members.push((planned, start));
        }
        if let Some((PlannedMember::Group(_), start)) = members.first() {
            self.error(
                *start,
                ProfileErrorKind::GroupStartsWithGroup(name.to_owned()),
            );
            valid = false;
        }
        (members, valid)
    }

    /// Reports each group that is in a cycle, at the member that starts the
    /// shortest cycle through it.
    fn find_loops(&mut self, planned: &Planned) {
        for (start, path) in super::loops::find(planned) {
            self.error(start, ProfileErrorKind::GroupLoop(path));
        }
    }
}
