//! A readable text of a venue dictionary: the output of `fixsim-profile`.

use crate::venue::{MessageView, VenueDictionary};
use fix_codec::{GroupDef, Member};
use std::fmt::Write as _;

fn tag_name(venue: &VenueDictionary, tag: u32) -> String {
    venue
        .field(tag)
        .map_or_else(|| tag.to_string(), |info| format!("{}({tag})", info.name))
}

fn tag_list(venue: &VenueDictionary, tags: &[u32]) -> String {
    let names: Vec<String> = tags.iter().map(|tag| tag_name(venue, *tag)).collect();
    names.join(", ")
}

/// The groups that the report has already written in full. A group is a
/// shared static, so its address tells whether it is the same group. A
/// group that two groups both nest would otherwise be written once for each
/// path, and the paths can grow as 2^n.
type Shown<'g> = Vec<&'g GroupDef<'g>>;

/// Writes a group and its nested groups, one member on each line. A group
/// that the report has shown before is named with the ID of its first use.
fn write_group<'g>(
    out: &mut String,
    venue: &VenueDictionary,
    group: &'g GroupDef<'g>,
    depth: usize,
    shown: &mut Shown<'g>,
) {
    let indent = "  ".repeat(depth);
    let name = tag_name(venue, group.count_tag);
    // Two definitions can have the same count tag, so each one has an ID.
    if let Some(position) = shown.iter().position(|known| std::ptr::eq(*known, group)) {
        let _ = writeln!(out, "{indent}group {name}  (see [{}])", position + 1);
        return;
    }
    shown.push(group);
    assert!(shown.len() <= SHOWN_MAX, "the report shows each group once");
    let _ = writeln!(out, "{indent}group {name} [{}]", shown.len());
    for (index, member) in group.members.iter().enumerate() {
        match member {
            Member::Field(tag) => {
                let mark = if index == 0 { "  (delimiter)" } else { "" };
                let _ = writeln!(out, "{indent}  {}{mark}", tag_name(venue, *tag));
            }
            Member::Group(nested) => write_group(out, venue, nested, depth + 1, shown),
        }
    }
}

/// The most groups in one report: the standard groups of a version and the
/// groups of a profile. The output has at most this many group trees.
const SHOWN_MAX: usize = 512;

fn write_message(
    out: &mut String,
    venue: &VenueDictionary,
    view: &MessageView<'_>,
    shown: &mut Shown<'static>,
) {
    let _ = writeln!(out, "message {} ({})", view.msg_type(), view.name());
    for (label, tags) in [
        ("required", view.profile_required()),
        ("forbidden", view.profile_forbidden()),
        ("added", view.profile_added()),
    ] {
        if !tags.is_empty() {
            let _ = writeln!(out, "  {label}: {}", tag_list(venue, tags));
        }
    }
    for group in view.groups() {
        write_group(out, venue, group, 1, shown);
    }
}

/// Describes what the profile changes: the version, the defaults, the new
/// fields, and for each message that the profile changes, its changes and
/// the tree of its repeating groups. A group that is used more than once is
/// written in full at its first use and named at the others.
pub fn describe(venue: &VenueDictionary) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "version {}", venue.version().begin_string());
    for (tag, value) in venue.profile_defaults() {
        let _ = writeln!(out, "default {} = \"{value}\"", tag_name(venue, tag));
    }
    for field in venue.custom_fields() {
        let values: Vec<&str> = field.values().collect();
        let _ = writeln!(
            out,
            "field {}({}) {} values [{}]",
            field.name,
            field.tag,
            field.ty.name(),
            values.join(", ")
        );
    }
    let mut shown: Shown<'static> = Vec::new();
    for msg_type in venue.changed_messages() {
        if let Some(view) = venue.message(msg_type) {
            write_message(&mut out, venue, &view, &mut shown);
        }
    }
    out
}
