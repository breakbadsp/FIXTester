//! A small element tree of the FIX Repository XML. It keeps only the `fix`
//! elements of the versions that the generator needs.

use crate::error::GenError;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

#[derive(Debug, Default)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// The attribute `key`, or a data error that names the element.
    pub fn require(&self, key: &str) -> Result<&str, GenError> {
        self.attr(key).ok_or_else(|| {
            GenError::data(format!("element <{}> has no attribute {key}", self.name))
        })
    }

    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |child| child.name == name)
    }
}

fn node_of(start: &BytesStart<'_>) -> Result<Node, GenError> {
    let name = start.name().as_ref().to_owned();
    let mut attrs = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| GenError::Xml(error.to_string()))?;
        let key = attribute.key.as_ref().to_owned();
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|error| GenError::Xml(error.to_string()))?;
        attrs.push((key, value.into_owned()));
    }
    Ok(Node {
        name,
        attrs,
        children: Vec::new(),
    })
}

fn wanted(start: &BytesStart<'_>, versions: &[&str]) -> Result<bool, GenError> {
    let node = node_of(start)?;
    Ok(node
        .attr("version")
        .is_some_and(|version| versions.contains(&version)))
}

#[derive(Debug)]
pub struct Parsed {
    /// The `copyright` attribute of the root element.
    pub copyright: String,
    /// The `fix` elements whose `version` is wanted, in file order.
    pub versions: Vec<Node>,
}

pub fn parse_versions(text: &str, versions: &[&str]) -> Result<Parsed, GenError> {
    let mut reader = Reader::from_str(text);
    let mut buffer = Vec::new();
    let mut stack: Vec<Node> = Vec::new();
    let mut found = Vec::new();
    let mut copyright = String::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| GenError::Xml(error.to_string()))?;
        match event {
            Event::Start(start) => {
                if start.name().as_ref() == "fix" && !wanted(&start, versions)? {
                    let end = start.to_end().into_owned();
                    let mut skip = Vec::new();
                    reader
                        .read_to_end_into(end.name(), &mut skip)
                        .map_err(|error| GenError::Xml(error.to_string()))?;
                } else {
                    let node = node_of(&start)?;
                    if stack.is_empty() {
                        node.attr("copyright")
                            .unwrap_or_default()
                            .clone_into(&mut copyright);
                    }
                    stack.push(node);
                }
            }
            Event::Empty(start) => {
                let node = node_of(&start)?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                }
            }
            Event::End(_) => {
                let node = stack
                    .pop()
                    .ok_or_else(|| GenError::Xml("an end tag has no start tag".to_owned()))?;
                if node.name == "fix" {
                    found.push(node);
                } else if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(Parsed {
        copyright,
        versions: found,
    })
}
