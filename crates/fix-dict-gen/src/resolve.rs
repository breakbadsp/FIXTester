//! Expands the components of a version in place and finds the repeating
//! groups. The result has no components left.

use crate::error::GenError;
use crate::model::{Component, Field, Item, Repository};
use std::collections::HashMap;

const DEPTH_MAX: u32 = 16;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Member {
    Field(u32),
    /// An index into [`Dict::groups`].
    Group(usize),
}

/// A repeating group. The first member is the delimiter.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupShape {
    pub count_tag: u32,
    pub members: Vec<Member>,
}

/// The fields and groups of a message body, the header, or the trailer.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Section {
    /// `(tag, required)`. A group adds its count tag here.
    pub fields: Vec<(u32, bool)>,
    /// Indexes into [`Dict::groups`].
    pub groups: Vec<usize>,
}

#[derive(Debug)]
pub struct Message {
    pub msg_type: String,
    pub name: String,
    pub body: Section,
}

#[derive(Debug)]
pub struct Dict {
    pub begin_string: String,
    pub fields: Vec<Field>,
    pub messages: Vec<Message>,
    pub header: Section,
    pub trailer: Section,
    pub groups: Vec<GroupShape>,
    /// `(length tag, data tag)`.
    pub data_pairs: Vec<(u32, u32)>,
}

struct Resolver<'a> {
    repo: &'a Repository,
    groups: Vec<GroupShape>,
    index: HashMap<GroupShape, usize>,
}

impl Section {
    fn add_field(&mut self, tag: u32, required: bool) {
        match self.fields.iter_mut().find(|(known, _)| *known == tag) {
            Some((_, known_required)) => *known_required |= required,
            None => self.fields.push((tag, required)),
        }
    }
}

fn component(repo: &Repository, id: u32) -> Result<&Component, GenError> {
    repo.components
        .get(&id)
        .ok_or_else(|| GenError::data(format!("{}: no component with id {id}", repo.begin_string)))
}

impl Resolver<'_> {
    fn intern(&mut self, shape: GroupShape) -> Result<usize, GenError> {
        if !matches!(shape.members.first(), Some(Member::Field(_))) {
            return Err(GenError::data(format!(
                "{}: group {} has no field as its first member",
                self.repo.begin_string, shape.count_tag
            )));
        }
        let mut seen: Vec<&Member> = Vec::new();
        for member in &shape.members {
            if seen.contains(&member) {
                return Err(GenError::data(format!(
                    "{}: group {} has a member twice",
                    self.repo.begin_string, shape.count_tag
                )));
            }
            seen.push(member);
        }
        if let Some(index) = self.index.get(&shape) {
            return Ok(*index);
        }
        self.groups.push(shape.clone());
        self.index.insert(shape, self.groups.len() - 1);
        Ok(self.groups.len() - 1)
    }

    /// The members of one group entry. Components are expanded in place.
    fn members(&mut self, items: &[Item], depth: u32) -> Result<Vec<Member>, GenError> {
        if depth > DEPTH_MAX {
            return Err(GenError::data("components nest too deep (a loop?)"));
        }
        let mut members = Vec::new();
        for item in items {
            match item {
                Item::Field { tag, .. } => members.push(Member::Field(*tag)),
                Item::Component { id, .. } => {
                    let items = &component(self.repo, *id)?.items;
                    members.extend(self.members(items, depth + 1)?);
                }
                Item::Group {
                    count_tag, items, ..
                } => {
                    let shape = GroupShape {
                        count_tag: *count_tag,
                        members: self.members(items, depth + 1)?,
                    };
                    members.push(Member::Group(self.intern(shape)?));
                }
            }
        }
        Ok(members)
    }

    /// Adds `items` to `section`. A field is required only when each
    /// component reference above it is required.
    fn section(
        &mut self,
        items: &[Item],
        required: bool,
        skip: &[u32],
        section: &mut Section,
        depth: u32,
    ) -> Result<(), GenError> {
        if depth > DEPTH_MAX {
            return Err(GenError::data("components nest too deep (a loop?)"));
        }
        for item in items {
            match item {
                Item::Field { tag, required: r } => section.add_field(*tag, required && *r),
                Item::Component { id, .. } if skip.contains(id) => {}
                Item::Component {
                    id,
                    required: component_required,
                } => {
                    let items = &component(self.repo, *id)?.items;
                    let required = required && *component_required;
                    self.section(items, required, skip, section, depth + 1)?;
                }
                Item::Group {
                    count_tag,
                    required: group_required,
                    items,
                } => {
                    let shape = GroupShape {
                        count_tag: *count_tag,
                        members: self.members(items, depth + 1)?,
                    };
                    let index = self.intern(shape)?;
                    section.add_field(*count_tag, required && *group_required);
                    if !section
                        .groups
                        .iter()
                        .any(|g| self.groups[*g].count_tag == *count_tag)
                    {
                        section.groups.push(index);
                    }
                }
            }
        }
        Ok(())
    }
}

/// The length and data field pairs. A length field names its data field in
/// `associatedDataTag`, but FIX 4.3 leaves that out, so the field name
/// (`RawDataLength` for `RawData`) is the second source.
fn data_pairs(repo: &Repository) -> Result<Vec<(u32, u32)>, GenError> {
    let is_data = |tag: u32| repo.fields.get(&tag).is_some_and(|f| f.ty == "data");
    let mut pairs = Vec::new();
    for field in repo.fields.values().filter(|field| field.ty == "Length") {
        let stem = field
            .name
            .strip_suffix("Length")
            .or_else(|| field.name.strip_suffix("Len"));
        let by_name = stem.and_then(|stem| {
            repo.fields
                .values()
                .find(|other| other.ty == "data" && other.name == stem)
                .map(|other| other.tag)
        });
        let by_attribute = field.associated.filter(|tag| is_data(*tag));
        if let Some(data) = by_attribute.or(by_name) {
            pairs.push((field.tag, data));
        }
    }
    for field in repo.fields.values().filter(|field| field.ty == "data") {
        if !pairs.iter().any(|(_, data)| *data == field.tag) {
            return Err(GenError::data(format!(
                "{}: data field {} has no length field",
                repo.begin_string, field.name
            )));
        }
    }
    Ok(pairs)
}

pub fn resolve(repo: &Repository) -> Result<Dict, GenError> {
    let header_id = repo.component_id("StandardHeader")?;
    let trailer_id = repo.component_id("StandardTrailer")?;
    let mut resolver = Resolver {
        repo,
        groups: Vec::new(),
        index: HashMap::new(),
    };
    let skip = [header_id, trailer_id];
    let mut header = Section::default();
    resolver.section(
        &component(repo, header_id)?.items,
        true,
        &[],
        &mut header,
        0,
    )?;
    let mut trailer = Section::default();
    resolver.section(
        &component(repo, trailer_id)?.items,
        true,
        &[],
        &mut trailer,
        0,
    )?;
    let mut messages = Vec::new();
    for source in &repo.messages {
        let mut body = Section::default();
        resolver.section(&source.items, true, &skip, &mut body, 0)?;
        messages.push(Message {
            msg_type: source.msg_type.clone(),
            name: source.name.clone(),
            body,
        });
    }
    messages.sort_by(|a, b| a.msg_type.cmp(&b.msg_type));
    if messages
        .windows(2)
        .any(|pair| pair[0].msg_type == pair[1].msg_type)
    {
        return Err(GenError::data(format!(
            "{}: a message type appears twice",
            repo.begin_string
        )));
    }
    Ok(Dict {
        begin_string: repo.begin_string.clone(),
        fields: repo.fields.values().cloned().collect(),
        messages,
        header,
        trailer,
        groups: resolver.groups,
        data_pairs: data_pairs(repo)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse_versions;

    /// A repository with one version: a block (`Instrument`) inside a
    /// required message reference, an optional block with a required field, a
    /// repeating block with a nested group, and a data field pair.
    const FIXTURE: &str = r#"<fixRepository copyright="test"><fix version="FIX.4.4">
      <fields>
        <field id="8" name="BeginString" type="String"/>
        <field id="10" name="CheckSum" type="String"/>
        <field id="35" name="MsgType" type="String"/>
        <field id="55" name="Symbol" type="String"/>
        <field id="48" name="SecurityID" type="String"/>
        <field id="11" name="ClOrdID" type="String"/>
        <field id="21" name="HandlInst" type="char">
          <enum value="1" symbolicName="Automated"/><enum value="2" symbolicName="Manual"/>
        </field>
        <field id="95" name="RawDataLength" type="Length"/>
        <field id="96" name="RawData" type="data"/>
        <field id="453" name="NoPartyIDs" type="NumInGroup"/>
        <field id="448" name="PartyID" type="String"/>
        <field id="802" name="NoPartySubIDs" type="NumInGroup"/>
        <field id="523" name="PartySubID" type="String"/>
        <field id="555" name="NoLegs" type="NumInGroup"/>
        <field id="600" name="LegSymbol" type="String"/>
      </fields>
      <components>
        <component id="1" name="StandardHeader" type="Block">
          <fieldRef id="8" required="1"/><fieldRef id="35" required="1"/>
        </component>
        <component id="2" name="StandardTrailer" type="Block">
          <fieldRef id="10" required="1"/>
        </component>
        <component id="3" name="Instrument" type="Block">
          <fieldRef id="55" required="1"/><fieldRef id="48" required="0"/>
        </component>
        <component id="4" name="Extra" type="Block">
          <fieldRef id="21" required="1"/>
        </component>
        <component id="5" name="PartySub" type="ImplicitBlockRepeating">
          <repeatingGroup id="802" required="0"><fieldRef id="523" required="0"/></repeatingGroup>
        </component>
        <component id="6" name="Parties" type="BlockRepeating">
          <repeatingGroup id="453" required="0">
            <fieldRef id="448" required="0"/><componentRef id="5" required="0"/>
          </repeatingGroup>
        </component>
        <component id="7" name="LegBlock" type="Block">
          <fieldRef id="600" required="1"/>
        </component>
      </components>
      <messages>
        <message id="1" name="Order" msgType="D">
          <componentRef id="1" required="1"/>
          <fieldRef id="11" required="1"/>
          <componentRef id="3" required="1"/>
          <componentRef id="4" required="0"/>
          <componentRef id="6" required="0"/>
          <repeatingGroup id="555" required="1">
            <componentRef id="7" required="1"/>
          </repeatingGroup>
          <fieldRef id="95" required="0"/><fieldRef id="96" required="0"/>
          <componentRef id="2" required="1"/>
        </message>
        <message id="2" name="Other" msgType="AB">
          <componentRef id="1" required="1"/>
          <componentRef id="6" required="0"/>
          <componentRef id="2" required="1"/>
        </message>
      </messages>
    </fix></fixRepository>"#;

    fn dict() -> Dict {
        let parsed = parse_versions(FIXTURE, &["FIX.4.4"]).unwrap();
        resolve(&Repository::from_node(&parsed.versions[0]).unwrap()).unwrap()
    }

    #[test]
    fn a_field_is_required_only_when_every_reference_above_it_is_required() {
        let dict = dict();
        let order = &dict
            .messages
            .iter()
            .find(|m| m.msg_type == "D")
            .unwrap()
            .body;
        let required = |tag: u32| {
            order
                .fields
                .iter()
                .find(|(t, _)| *t == tag)
                .map(|(_, r)| *r)
        };
        assert_eq!(required(11), Some(true));
        assert_eq!(required(55), Some(true), "in a required block");
        assert_eq!(required(48), Some(false), "optional in a required block");
        assert_eq!(required(21), Some(false), "required in an optional block");
        assert_eq!(
            required(555),
            Some(true),
            "a required group needs its count"
        );
        assert_eq!(required(453), Some(false));
    }

    #[test]
    fn the_header_and_the_trailer_are_not_part_of_the_body() {
        let dict = dict();
        let order = &dict
            .messages
            .iter()
            .find(|m| m.msg_type == "D")
            .unwrap()
            .body;
        assert!(order.fields.iter().all(|(tag, _)| *tag != 8 && *tag != 10));
        assert_eq!(dict.header.fields, vec![(8, true), (35, true)]);
        assert_eq!(dict.trailer.fields, vec![(10, true)]);
    }

    #[test]
    fn a_group_in_a_component_keeps_its_nested_group_and_is_shared() {
        let dict = dict();
        let parties = dict.groups.iter().find(|g| g.count_tag == 453).unwrap();
        let sub = dict.groups.iter().position(|g| g.count_tag == 802).unwrap();
        assert_eq!(
            parties.members,
            vec![Member::Field(448), Member::Group(sub)]
        );
        let uses: Vec<_> = dict
            .messages
            .iter()
            .filter(|m| {
                m.body
                    .groups
                    .iter()
                    .any(|g| dict.groups[*g].count_tag == 453)
            })
            .collect();
        assert_eq!(uses.len(), 2, "both messages use the one interned group");
        assert_eq!(dict.groups.iter().filter(|g| g.count_tag == 453).count(), 1);
    }

    #[test]
    fn a_component_in_a_group_is_expanded_in_place() {
        let dict = dict();
        let legs = dict.groups.iter().find(|g| g.count_tag == 555).unwrap();
        assert_eq!(legs.members, vec![Member::Field(600)]);
    }

    #[test]
    fn the_data_pair_comes_from_the_field_names() {
        assert_eq!(dict().data_pairs, vec![(95, 96)]);
    }

    #[test]
    fn a_group_that_starts_with_a_group_is_an_error() {
        let text = FIXTURE.replace(
            r#"<fieldRef id="448" required="0"/><componentRef id="5" required="0"/>"#,
            r#"<componentRef id="5" required="0"/><fieldRef id="448" required="0"/>"#,
        );
        let parsed = parse_versions(&text, &["FIX.4.4"]).unwrap();
        let result = resolve(&Repository::from_node(&parsed.versions[0]).unwrap());
        assert!(matches!(result, Err(GenError::Data(_))));
    }
}
