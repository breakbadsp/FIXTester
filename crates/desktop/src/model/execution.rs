mod action;
mod context;

pub use action::Action;
pub use context::{Context, Source};

use fix_codec::UtcMillis;
use fix_dict::Version;

use super::{Application, retained};
use crate::{Result, invalid};

#[derive(Debug, Clone)]
pub struct ReportField {
    pub tag: u32,
    pub label: &'static str,
    pub value: String,
    pub required: bool,
    pub detail: bool,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub action: Action,
    version: Version,
    fields: Vec<ReportField>,
    groups: Vec<(u32, String)>,
    multileg: bool,
    reference_type: String,
}

impl Report {
    pub fn draft(context: &Context, child: Option<usize>, action: Action) -> Result<Self> {
        if !context.source.actions().contains(&action) {
            return Err(invalid(
                "This action does not apply to the selected message.",
            ));
        }
        let order = context.order(child)?;
        let mut report = Self {
            action,
            version: context.version,
            fields: Vec::new(),
            groups: Vec::new(),
            multileg: order.value(442) == "3" || (order.multileg && order.value(442) != "2"),
            reference_type: if context.source == Source::Trade {
                order.value(150).to_owned()
            } else {
                String::new()
            },
        };
        let order_id = if context.source.assigns_order_id() {
            context.order_id(child)
        } else {
            order.value(37).to_owned()
        };
        report.field(37, order_id, true, false);
        report.field(11, order.value(11), true, false);
        if context.source.request_chain() {
            report.field(41, order.value(41), true, false);
        }
        report.field(39, action.status(context.version), true, false);
        if action.cancel_reject().is_some() {
            report.field(102, "", false, false);
        } else {
            report.execution_fields(context, order);
        }
        report.field(58, "", false, true);
        Ok(report)
    }

    fn execution_fields(&mut self, context: &Context, order: &context::OrderContext) {
        use Action::{
            Acknowledge, Calculated, Cancel, DoneForDay, Expire, FullFill, PendingNew,
            PendingReplace, Reject, Restate, Status, Stopped,
        };
        let action = self.action;
        let initial = matches!(action, Acknowledge | PendingNew | Reject);
        let new_terms = action != PendingReplace;
        self.field(54, order.value(54), true, false);
        self.field(55, order.value(55), self.version == Version::Fix42, false);
        self.field(
            38,
            if new_terms { order.value(38) } else { "" },
            false,
            false,
        );
        let leaves = match action {
            Acknowledge | PendingNew => order.value(38),
            FullFill | Reject | Cancel | Expire | DoneForDay | Calculated => "0",
            _ => "",
        };
        let initial_value = if initial { "0" } else { "" };
        self.field(14, initial_value, true, false);
        self.field(151, leaves, true, false);
        self.field(6, initial_value, true, false);
        if action.has_trade_values() || action == Stopped {
            self.field(32, "", action.has_trade_values(), false);
            self.field(31, "", action.has_trade_values(), false);
        }
        if action.references_trade() {
            let reference = if context.source == Source::Trade {
                order.value(17)
            } else {
                ""
            };
            self.field(19, reference, true, false);
        }
        if action == Reject {
            self.field(103, "", false, false);
        }
        if matches!(action, Restate | Cancel) {
            self.field(378, "", action == Restate, false);
        }
        let list_id = if context.source == Source::List {
            &context.list_id
        } else {
            order.value(66)
        };
        if !list_id.is_empty() {
            self.field(66, list_id, true, true);
        }
        if action == Status && !order.value(790).is_empty() {
            self.field(790, order.value(790), true, true);
        }
        self.order_details(order, new_terms);
    }

    fn order_details(&mut self, order: &context::OrderContext, new_terms: bool) {
        for tag in [
            22, 48, 65, 167, 200, 201, 202, 205, 206, 207, 223, 231, 460, 461, 541, 762,
        ] {
            let value = order.value(tag);
            if !value.is_empty() || matches!(tag, 22 | 48) {
                self.field(tag, value, false, true);
            }
        }
        for tag in [1, 15, 40, 44, 59, 99, 152, 468] {
            let value = if new_terms || tag == 1 {
                order.value(tag)
            } else {
                ""
            };
            if !value.is_empty() || matches!(tag, 44 | 152 | 468) {
                self.field(tag, value, false, true);
            }
        }
        if !order.value(442).is_empty() {
            self.field(442, order.value(442), false, true);
        } else if self.multileg {
            self.field(442, "3", true, true);
        }
        if new_terms {
            self.groups.clone_from(&order.groups);
        }
    }

    fn field(&mut self, tag: u32, value: impl Into<String>, required: bool, detail: bool) {
        let dict = self.version.dictionary();
        if let Some(definition) = dict.field(tag)
            && dict
                .message(self.action.msg_type())
                .is_some_and(|msg| msg.body.contains(tag))
        {
            self.fields.push(ReportField {
                tag,
                label: definition.name,
                value: value.into(),
                required,
                detail,
            });
        }
    }

    pub fn fields_mut(&mut self) -> &mut [ReportField] {
        &mut self.fields
    }

    pub fn carried_groups(&self) -> &[(u32, String)] {
        &self.groups
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn reference_fill_type(&mut self) -> Option<&mut String> {
        (self.version == Version::Fix42 && self.action.references_trade())
            .then_some(&mut self.reference_type)
    }

    fn value(&self, tag: u32) -> &str {
        self.fields
            .iter()
            .find(|field| field.tag == tag)
            .map_or("", |field| &field.value)
    }

    pub fn validate(&self) -> Result<()> {
        for field in &self.fields {
            if field.required && field.value.is_empty() {
                return Err(invalid(format!("Enter {} ({}).", field.label, field.tag)));
            }
            if !field.value.is_empty()
                && retained(self.version.dictionary(), field.tag, field.value.as_bytes()).is_none()
            {
                return Err(invalid(format!("Check {} ({}).", field.label, field.tag)));
            }
        }
        if self.action.cancel_reject().is_none() {
            self.validate_execution()?;
        }
        Ok(())
    }

    fn validate_execution(&self) -> Result<()> {
        if self.value(48).is_empty() != self.value(22).is_empty() {
            return Err(invalid("Enter SecurityID and IDSource together."));
        }
        if self.value(55).is_empty() && self.value(48).is_empty() && !self.multileg {
            return Err(invalid("Enter a symbol or a security identifier."));
        }
        let quantity_required = !self.multileg
            && !(matches!(
                self.action,
                Action::Acknowledge | Action::PendingNew | Action::Reject
            ) && (!self.value(152).is_empty() || !self.value(468).is_empty()));
        if quantity_required && self.value(38).is_empty() {
            return Err(invalid("Enter OrderQty (38)."));
        }
        for tag in [14, 32, 38, 151, 152, 468] {
            if self.value(tag).starts_with('-') {
                return Err(invalid(format!("Tag {tag} needs a nonnegative quantity.")));
            }
        }
        if self.action.has_trade_values() && !nonzero(self.value(32)) {
            return Err(invalid("Enter a positive last quantity (32)."));
        }
        if self.action == Action::FullFill && nonzero(self.value(151)) {
            return Err(invalid("A full fill requires zero LeavesQty."));
        }
        if self.action == Action::PartialFill && !nonzero(self.value(151)) {
            return Err(invalid("A partial fill requires positive LeavesQty."));
        }
        if self.version == Version::Fix42
            && self.action.references_trade()
            && !matches!(self.reference_type.as_str(), "1" | "2")
        {
            return Err(invalid("Select the type of the referenced FIX 4.2 fill."));
        }
        Ok(())
    }

    pub(crate) fn application(&self, time: UtcMillis, exec_id: String) -> Result<Application> {
        self.validate()?;
        let action = self.action;
        let mut fields = self
            .fields
            .iter()
            .filter(|field| !field.value.is_empty())
            .map(|field| (field.tag, field.value.clone()))
            .collect::<Vec<_>>();
        if let Some(response_to) = action.cancel_reject() {
            fields.push((434, response_to.into()));
        } else {
            fields.push((17, exec_id));
            fields.push((
                150,
                action.exec_type(self.version, self.value(39), &self.reference_type),
            ));
            if self.version == Version::Fix42 {
                fields.push((20, action.transaction().into()));
                if action != Action::Status {
                    for tag in [31, 32] {
                        if self.value(tag).is_empty() {
                            fields.push((tag, "0".into()));
                        }
                    }
                }
            }
            fields.extend_from_slice(&self.groups);
        }
        let mut timestamp = [0; 21];
        let timestamp = fix_codec::write_utc_timestamp(time, &mut timestamp);
        fields.push((60, String::from_utf8_lossy(timestamp).into_owned()));
        Application::new(self.version, action.msg_type(), fields)
    }

    pub(crate) fn exec_id(&self) -> Option<&'static str> {
        match self.action {
            Action::Status => Some("0"),
            action if action.cancel_reject().is_some() => Some(""),
            _ => None,
        }
    }
}

fn nonzero(value: &str) -> bool {
    value.bytes().any(|byte| (b'1'..=b'9').contains(&byte))
}

#[cfg(test)]
mod tests;
