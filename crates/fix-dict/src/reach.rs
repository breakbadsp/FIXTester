//! For each group, the sorted tags that can be in its entries, with the
//! tags of its nested groups. The validation uses it to skip the fields of a
//! group that it cannot read. A tree walk would follow each path through
//! shared nested groups, and the paths can grow as 2^n.

use fix_codec::{GroupDef, Member};
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::version::Version;

/// The tags of each group, by the address of the (shared, static) group.
#[derive(Debug, Clone, Default)]
pub(crate) struct ReachMap {
    tags: BTreeMap<usize, Vec<u32>>,
}

fn key(group: &GroupDef<'_>) -> usize {
    std::ptr::from_ref(group).addr()
}

impl ReachMap {
    /// Computes the tags of `roots` and of the groups nested in them. The
    /// work is linear in the number of distinct groups and their members.
    pub fn build<'g>(roots: impl Iterator<Item = &'g GroupDef<'g>>) -> Self {
        let mut map = Self::default();
        for root in roots {
            map.visit(root);
        }
        map
    }

    fn visit(&mut self, group: &GroupDef<'_>) {
        if self.tags.contains_key(&key(group)) {
            return;
        }
        let mut tags = vec![group.count_tag];
        for member in group.members {
            match member {
                Member::Field(tag) => tags.push(*tag),
                Member::Group(nested) => {
                    self.visit(nested);
                    tags.extend(self.tags.get(&key(nested)).into_iter().flatten());
                }
            }
        }
        tags.sort_unstable();
        tags.dedup();
        self.tags.insert(key(group), tags);
    }

    /// `None` when the map does not have the group.
    pub fn contains(&self, group: &GroupDef<'_>, tag: u32) -> Option<bool> {
        let tags = self.tags.get(&key(group))?;
        Some(tags.binary_search(&tag).is_ok())
    }

    /// The map of the standard groups of `version`. It is built at the first
    /// use and then shared.
    pub fn standard(version: Version) -> &'static Self {
        static MAPS: [OnceLock<ReachMap>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
        MAPS[version.index()].get_or_init(|| {
            let dict = version.dictionary();
            let sections = dict
                .messages
                .iter()
                .map(|message| &message.body)
                .chain([&dict.header, &dict.trailer]);
            Self::build(sections.flat_map(|section| section.groups.iter().copied()))
        })
    }
}
