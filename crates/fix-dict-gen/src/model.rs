//! The repository data of one FIX version, as the XML gives it.

use crate::error::GenError;
use crate::xml::Node;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone)]
pub struct Field {
    pub tag: u32,
    pub name: String,
    /// The repository type name, for example `Qty` or `UTCTimestamp`.
    pub ty: String,
    /// `(value, symbolic name)` in file order.
    pub values: Vec<(String, String)>,
    /// The `associatedDataTag` attribute.
    pub associated: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Field {
        tag: u32,
        required: bool,
    },
    Component {
        id: u32,
        required: bool,
    },
    Group {
        count_tag: u32,
        required: bool,
        items: Vec<Item>,
    },
}

#[derive(Debug)]
pub struct Component {
    pub name: String,
    pub items: Vec<Item>,
}

#[derive(Debug)]
pub struct MessageSource {
    pub msg_type: String,
    pub name: String,
    pub items: Vec<Item>,
}

#[derive(Debug)]
pub struct Repository {
    pub begin_string: String,
    pub fields: BTreeMap<u32, Field>,
    pub components: HashMap<u32, Component>,
    pub messages: Vec<MessageSource>,
}

fn number(node: &Node, key: &str) -> Result<u32, GenError> {
    let text = node.require(key)?;
    text.parse().map_err(|_| {
        GenError::data(format!(
            "attribute {key}=\"{text}\" of <{}> is not a number",
            node.name
        ))
    })
}

fn is_required(node: &Node) -> bool {
    node.attr("required") == Some("1")
}

fn parse_items(parent: &Node) -> Result<Vec<Item>, GenError> {
    let mut items = Vec::new();
    for child in &parent.children {
        match child.name.as_str() {
            "fieldRef" => items.push(Item::Field {
                tag: number(child, "id")?,
                required: is_required(child),
            }),
            "componentRef" => items.push(Item::Component {
                id: number(child, "id")?,
                required: is_required(child),
            }),
            "repeatingGroup" => items.push(Item::Group {
                count_tag: number(child, "id")?,
                required: is_required(child),
                items: parse_items(child)?,
            }),
            _ => {}
        }
    }
    Ok(items)
}

fn parse_field(node: &Node) -> Result<Field, GenError> {
    let tag = number(node, "id")?;
    let mut values = Vec::new();
    for value in node.children_named("enum") {
        values.push((
            value.require("value")?.to_owned(),
            value.require("symbolicName")?.to_owned(),
        ));
    }
    let associated = match node.attr("associatedDataTag") {
        Some(text) => Some(text.parse().map_err(|_| {
            GenError::data(format!("field {tag} has a bad associatedDataTag {text}"))
        })?),
        None => None,
    };
    Ok(Field {
        tag,
        name: node.require("name")?.to_owned(),
        ty: node.require("type")?.to_owned(),
        values,
        associated,
    })
}

impl Repository {
    /// Reads one `fix` element.
    pub fn from_node(fix: &Node) -> Result<Self, GenError> {
        let begin_string = fix.require("version")?.to_owned();
        let mut fields = BTreeMap::new();
        for node in fix.child("fields").iter().flat_map(|n| &n.children) {
            let field = parse_field(node)?;
            if fields.insert(field.tag, field).is_some() {
                return Err(GenError::data(format!(
                    "{begin_string}: a field tag appears twice"
                )));
            }
        }
        let mut components = HashMap::new();
        for node in fix.child("components").iter().flat_map(|n| &n.children) {
            let id = number(node, "id")?;
            let component = Component {
                name: node.require("name")?.to_owned(),
                items: parse_items(node)?,
            };
            components.insert(id, component);
        }
        let mut messages = Vec::new();
        for node in fix.child("messages").iter().flat_map(|n| &n.children) {
            messages.push(MessageSource {
                msg_type: node.require("msgType")?.to_owned(),
                name: node.require("name")?.to_owned(),
                items: parse_items(node)?,
            });
        }
        Ok(Self {
            begin_string,
            fields,
            components,
            messages,
        })
    }

    pub fn component_id(&self, name: &str) -> Result<u32, GenError> {
        self.components
            .iter()
            .find(|(_, component)| component.name == name)
            .map(|(id, _)| *id)
            .ok_or_else(|| GenError::data(format!("{}: no component {name}", self.begin_string)))
    }
}
