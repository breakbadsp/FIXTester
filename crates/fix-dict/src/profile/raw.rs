//! The TOML structure of a venue profile. `Spanned` values keep their place
//! in the file for error messages.

use serde::Deserialize;
use std::collections::BTreeMap;
use toml::Spanned;

/// A tag or a name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Ref {
    Tag(u32),
    Name(String),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawProfile {
    pub version: Spanned<String>,
    #[serde(default)]
    pub defaults: BTreeMap<String, Spanned<String>>,
    #[serde(default)]
    pub field: Vec<Spanned<RawField>>,
    #[serde(default)]
    pub group: BTreeMap<String, Spanned<RawGroup>>,
    #[serde(default)]
    pub message: BTreeMap<String, Spanned<RawMessage>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawField {
    pub tag: Spanned<u32>,
    pub name: Option<Spanned<String>>,
    #[serde(rename = "type")]
    pub ty: Option<Spanned<String>>,
    #[serde(default)]
    pub values: Vec<Spanned<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawGroup {
    pub count: Spanned<Ref>,
    #[serde(default)]
    pub members: Vec<Spanned<Ref>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMessage {
    #[serde(default)]
    pub required: Vec<Spanned<Ref>>,
    #[serde(default)]
    pub forbidden: Vec<Spanned<Ref>>,
    #[serde(default)]
    pub add: Vec<Spanned<Ref>>,
}
