use fix_codec::{GroupDef, Member, tag::msg_type};

use super::{Application, MESSAGE_MAX};
use crate::{Result, invalid};

pub const ADDITIONAL_TAGS_ERROR_PREFIX: &str = "Additional FIX tags: ";

impl Application {
    pub(crate) fn with_additional_tags(self, text: &str) -> Result<Self> {
        self.merge_additional_tags(text)
            .map_err(|error| invalid(format!("{ADDITIONAL_TAGS_ERROR_PREFIX}{error}")))
    }

    fn merge_additional_tags(mut self, text: &str) -> Result<Self> {
        if text.len() > MESSAGE_MAX {
            return Err(invalid("The field exceeds 16 KiB."));
        }
        let text = text.trim_matches(' ');
        if text.is_empty() {
            return Ok(self);
        }
        let dict = self.version.dictionary();
        let definition = dict
            .message(&self.kind)
            .ok_or_else(|| invalid("Unknown guided message type."))?;
        let body = definition.body;
        let mut parsed = Vec::new();
        for pair in text.strip_suffix(';').unwrap_or(text).split(';') {
            let (tag, value) = parse_pair(pair)?;
            if dict.is_group_count(tag)
                || body.groups.iter().any(|group| group_contains(group, tag))
            {
                return Err(invalid(format!("Tag {tag} belongs to a repeating group.")));
            }
            if !body.contains(tag)
                && dict.messages.iter().any(|message| {
                    msg_type::is_admin(message.msg_type.as_bytes()) && message.body.contains(tag)
                })
            {
                return Err(invalid(format!("Tag {tag} is a session field.")));
            }
            if protected(&self.kind, tag) || self.fields.iter().any(|(number, _)| *number == tag) {
                return Err(invalid(format!(
                    "Tag {tag} is protected or already present. Use its form field."
                )));
            }
            if parsed.iter().any(|(number, _)| *number == tag) {
                return Err(invalid(format!("Tag {tag} appears twice.")));
            }
            parsed.push((tag, value));
        }
        let first_group = self
            .fields
            .iter()
            .position(|(tag, _)| body.group(*tag).is_some())
            .unwrap_or(self.fields.len());
        self.fields.splice(first_group..first_group, parsed);
        Application::new(self.version, &self.kind, self.fields)
    }
}

fn parse_pair(pair: &str) -> Result<(u32, String)> {
    let (tag, value) = pair
        .split_once('=')
        .ok_or_else(|| invalid("Use Tag=Value pairs separated by semicolons."))?;
    let tag = tag.trim_matches(' ');
    let number = tag
        .parse::<u32>()
        .ok()
        .filter(|_| tag.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| invalid("Enter a numeric tag."))?;
    Ok((number, value.trim_matches(' ').to_owned()))
}

fn protected(kind: &str, tag: u32) -> bool {
    match kind {
        "8" | "9" => matches!(
            tag,
            6 | 11
                | 14
                | 17
                | 19
                | 20
                | 31
                | 32
                | 37
                | 38
                | 39
                | 41
                | 59
                | 60
                | 66
                | 126
                | 150
                | 151
                | 152
                | 432
                | 434
                | 442
                | 468
                | 790
        ),
        "D" | "F" | "G" => matches!(tag, 11 | 37 | 38 | 40 | 41 | 44 | 54 | 55 | 60),
        _ => false,
    }
}

fn group_contains(group: &GroupDef<'_>, tag: u32) -> bool {
    group.members.iter().any(|member| match member {
        Member::Field(number) => *number == tag,
        Member::Group(nested) => group_contains(nested, tag),
    })
}

#[cfg(test)]
mod tests;
