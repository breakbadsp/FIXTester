use super::groups::{Built, Planned};
use super::raw::{RawMessage, Ref};
use super::{Loader, ProfileErrorKind, Target};
use crate::venue::MessageOverride;
use toml::Spanned;

/// The changes of one message, before the groups are built.
#[derive(Debug, Default)]
pub(super) struct Pending {
    msg_type: String,
    required: Vec<u32>,
    forbidden: Vec<u32>,
    added: Vec<u32>,
    /// Names of the groups to add.
    group_names: Vec<String>,
}

impl Pending {
    pub fn finish(self, built: &Built) -> MessageOverride {
        let groups = self
            .group_names
            .iter()
            .filter_map(|name| built.def(name))
            .collect();
        MessageOverride {
            msg_type: self.msg_type,
            required: self.required,
            forbidden: self.forbidden,
            added: self.added,
            groups,
        }
    }
}

fn push_unique(list: &mut Vec<u32>, tag: u32) {
    if !list.contains(&tag) {
        list.push(tag);
    }
}

impl Loader<'_> {
    /// Reads the `[message.<MsgType>]` blocks.
    pub(super) fn load_messages(&mut self, planned: &Planned) -> Vec<Pending> {
        let raw = self.raw;
        let mut pending = Vec::new();
        for (msg_type, message) in &raw.message {
            if self.venue.standard.message(msg_type).is_none() {
                let kind = ProfileErrorKind::UnknownMessage(msg_type.clone());
                self.error(message.span().start, kind);
                continue;
            }
            pending.push(self.load_message(msg_type, message.get_ref(), planned));
        }
        pending
    }

    fn load_message(&mut self, msg_type: &str, raw: &RawMessage, planned: &Planned) -> Pending {
        let mut message = Pending {
            msg_type: msg_type.to_owned(),
            ..Pending::default()
        };
        for reference in &raw.required {
            if let Some(tag) = self.message_target(reference, &mut message, planned, true) {
                push_unique(&mut message.required, tag);
            }
        }
        for reference in &raw.add {
            if let Some(tag) = self.message_target(reference, &mut message, planned, true) {
                push_unique(&mut message.added, tag);
            }
        }
        let mut forbidden_at: Vec<(u32, usize)> = Vec::new();
        for reference in &raw.forbidden {
            if let Some(tag) = self.message_target(reference, &mut message, planned, false) {
                push_unique(&mut message.forbidden, tag);
                forbidden_at.push((tag, reference.span().start));
            }
        }
        self.check_conflicts(&message, &forbidden_at);
        message.required.sort_unstable();
        message.forbidden.sort_unstable();
        message.added.sort_unstable();
        message
    }

    /// Finds the tag of a reference in `required`, `add`, or `forbidden`. A
    /// group block gives its count tag, and with `adds_group` it also puts
    /// the group into the message.
    fn message_target(
        &mut self,
        reference: &Spanned<Ref>,
        message: &mut Pending,
        planned: &Planned,
        adds_group: bool,
    ) -> Option<u32> {
        match self.target(reference.get_ref(), true) {
            Ok(Target::Field(tag)) => Some(tag),
            Ok(Target::Group(name)) => {
                // A group with an error has no count tag. The error is reported.
                let tag = planned.count_tag(&name)?;
                if adds_group && !message.group_names.contains(&name) {
                    message.group_names.push(name);
                }
                Some(tag)
            }
            Err(kind) => {
                self.error(reference.span().start, kind);
                None
            }
        }
    }

    fn check_conflicts(&mut self, message: &Pending, forbidden_at: &[(u32, usize)]) {
        for (tag, start) in forbidden_at {
            let msg_type = message.msg_type.clone();
            let standard_required = self
                .venue
                .message(&message.msg_type)
                .is_some_and(|view| view.is_required(*tag));
            if standard_required || message.required.contains(tag) {
                let kind = ProfileErrorKind::RequiredAndForbidden {
                    msg_type,
                    tag: *tag,
                };
                self.error(*start, kind);
            } else if message.added.contains(tag) {
                let kind = ProfileErrorKind::AddedAndForbidden {
                    msg_type,
                    tag: *tag,
                };
                self.error(*start, kind);
            }
        }
    }
}
