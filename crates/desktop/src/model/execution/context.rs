use fix_codec::{Decoder, Fields, Group, GroupDef, Member};
use fix_dict::{Version, schema::Section};

use super::super::{FIELDS_MAX, retained};
use super::action::{self, Action};
use crate::store::{Direction, Entry};
use crate::{Result, invalid};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    New,
    List,
    Cancel,
    Replace,
    Status,
    Trade,
}

impl Source {
    pub fn identify(version: Version, entry: &Entry) -> Option<Self> {
        version.dictionary().message(&entry.kind)?;
        let status = if entry.status.starts_with(super::automation::SKIPPED) {
            "Received"
        } else {
            entry.status.as_str()
        };
        match (entry.direction, status, entry.kind.as_str()) {
            (Direction::Received, "Received", "D" | "AB") => Some(Self::New),
            (Direction::Received, "Received", "E") => Some(Self::List),
            (Direction::Received, "Received", "F") => Some(Self::Cancel),
            (Direction::Received, "Received", "G" | "AC") => Some(Self::Replace),
            (Direction::Received, "Received", "H") => Some(Self::Status),
            (Direction::Sent, "Sent", "8") => {
                let mut decoder = Decoder::new(FIELDS_MAX);
                let message = decoder.decode(&entry.raw).ok()?;
                let fields = message.fields();
                let trade = if version == Version::Fix42 {
                    matches!(fields.get(150), Some(b"1" | b"2"))
                        && matches!(fields.get(20), Some(b"0" | b"2"))
                } else {
                    matches!(fields.get(150), Some(b"F" | b"G"))
                };
                (trade && fields.get(17).is_some()).then_some(Self::Trade)
            }
            _ => None,
        }
    }

    pub fn actions(self) -> &'static [Action] {
        match self {
            Self::New | Self::List => action::ORDER,
            Self::Cancel => action::CANCEL,
            Self::Replace => action::REPLACE,
            Self::Status => &[Action::Status],
            Self::Trade => action::TRADE,
        }
    }

    pub(super) fn assigns_order_id(self) -> bool {
        matches!(self, Self::New | Self::List)
    }

    pub(super) fn request_chain(self) -> bool {
        matches!(self, Self::Cancel | Self::Replace)
    }
}

#[derive(Debug)]
pub struct Context {
    pub(super) version: Version,
    pub(super) source: Source,
    row: i64,
    orders: Vec<OrderContext>,
    pub(super) list_id: String,
}

#[derive(Debug, Default, Clone)]
pub(super) struct OrderContext {
    pub(super) fields: Vec<(u32, String)>,
    pub(super) groups: Vec<(u32, String)>,
    pub(super) multileg: bool,
}

impl OrderContext {
    pub(super) fn value(&self, tag: u32) -> &str {
        self.fields
            .iter()
            .find(|(number, _)| *number == tag)
            .map_or("", |(_, value)| value.as_str())
    }
}

impl Context {
    pub fn read(version: Version, entry: &Entry) -> Result<Self> {
        let source = Source::identify(version, entry)
            .ok_or_else(|| invalid("Select a received order or a sent trade."))?;
        let dict = version.dictionary();
        let definition = dict
            .message(&entry.kind)
            .ok_or_else(|| invalid("Unknown message type."))?;
        let mut decoder = Decoder::new(FIELDS_MAX);
        let message = decoder
            .decode(&entry.raw)
            .map_err(|_| invalid("Cannot decode the selected message."))?;
        if message.begin_string() != dict.begin_string.as_bytes()
            || message.msg_type() != entry.kind.as_bytes()
        {
            return Err(invalid(
                "The selected message does not match this FIX session.",
            ));
        }
        let fields = message.fields();
        let report = dict
            .message("8")
            .ok_or_else(|| invalid("Missing report definition."))?;
        let mut context = Self {
            version,
            source,
            row: entry.id,
            orders: Vec::new(),
            list_id: String::new(),
        };
        if source == Source::List {
            context.read_list(fields, &definition.body, &report.body)?;
        } else {
            context
                .orders
                .push(read_order(version, fields, &definition.body, &report.body)?);
        }
        Ok(context)
    }

    fn read_list(&mut self, fields: Fields<'_>, source: &Section, target: &Section) -> Result<()> {
        let def = source
            .group(73)
            .ok_or_else(|| invalid("Missing NoOrders definition."))?;
        let children =
            read_group(fields, def)?.ok_or_else(|| invalid("The list has no orders."))?;
        let list = read_order(self.version, fields, source, target)?;
        list.value(66).clone_into(&mut self.list_id);
        if self.list_id.is_empty() || children.is_empty() {
            return Err(invalid(
                "The list needs a ListID and at least one child order.",
            ));
        }
        let nested = def
            .members
            .iter()
            .filter_map(|member| match member {
                Member::Group(group) => Some(*group),
                Member::Field(_) => None,
            })
            .collect::<Vec<_>>();
        for child in children.entries() {
            let order = read_fields(self.version, child, &nested, target)?;
            let id = order.value(11);
            if id.is_empty() || self.orders.iter().any(|other| other.value(11) == id) {
                return Err(invalid("Each list child needs a different ClOrdID."));
            }
            self.orders.push(order);
        }
        Ok(())
    }

    pub fn source(&self) -> Source {
        self.source
    }

    pub fn children(&self) -> impl Iterator<Item = (usize, String)> + '_ {
        self.orders.iter().enumerate().map(|(index, order)| {
            let (id, symbol) = (order.value(11), order.value(55));
            let label = if symbol.is_empty() {
                id.to_owned()
            } else {
                format!("{id} · {symbol}")
            };
            (index, label)
        })
    }

    pub(super) fn order(&self, child: Option<usize>) -> Result<&OrderContext> {
        let index = match (self.source, child) {
            (Source::List, Some(index)) => index,
            (Source::List, None) => return Err(invalid("Select one order in the list.")),
            (_, None) => 0,
            (_, Some(_)) => return Err(invalid("The selected message is not an order list.")),
        };
        self.orders
            .get(index)
            .ok_or_else(|| invalid("Unknown list child."))
    }

    pub(super) fn order_id(&self, child: Option<usize>) -> String {
        if let Some(index) = child {
            format!("FT-R{}-{}", self.row, index + 1)
        } else {
            format!("FT-R{}", self.row)
        }
    }
}

pub(super) fn read_order(
    version: Version,
    fields: Fields<'_>,
    source: &Section,
    target: &Section,
) -> Result<OrderContext> {
    read_fields(version, fields, source.groups, target)
}

fn read_fields(
    version: Version,
    mut fields: Fields<'_>,
    definitions: &[&GroupDef<'_>],
    target: &Section,
) -> Result<OrderContext> {
    let dict = version.dictionary();
    let mut order = OrderContext::default();
    while let Some((tag, raw)) = fields.iter().next() {
        let group = definitions.iter().find(|group| group.count_tag == tag);
        if let Some(def) = group {
            let group = read_group(fields, def)?.ok_or_else(|| invalid("Missing group count."))?;
            if matches!(tag, 454 | 555)
                && !group.is_empty()
                && let Some(destination) = target.group(tag)
            {
                order
                    .groups
                    .extend(project_group(version, group, def, destination)?);
                order.multileg |= tag == 555;
            }
            fields = fields.skip(1 + group.entries().map(|entry| entry.len()).sum::<usize>());
            continue;
        }
        if !order.fields.iter().any(|(number, _)| *number == tag)
            && let Some(field) = retained(dict, tag, raw)
        {
            order.fields.push(field);
        }
        fields = fields.skip(1);
    }
    Ok(order)
}

fn read_group<'a, 'd>(fields: Fields<'a>, def: &'d GroupDef<'d>) -> Result<Option<Group<'a, 'd>>> {
    fields
        .group(def)
        .map_err(|_| invalid(format!("Cannot read repeating group {}.", def.count_tag)))
}

fn project_group(
    version: Version,
    group: Group<'_, '_>,
    source: &GroupDef<'_>,
    target: &GroupDef<'_>,
) -> Result<Vec<(u32, String)>> {
    let dict = version.dictionary();
    let mut projected = vec![(target.count_tag, group.len().to_string())];
    for entry in group.entries() {
        let start = projected.len();
        for member in target.members {
            match member {
                // A prior execution price is not a price for this report.
                Member::Field(tag) if *tag != 637
                    && !dict.data_pairs.iter().any(|(length, data)| tag == length || tag == data) => {
                    if source.members.iter().any(|member| matches!(member, Member::Field(number) if number == tag))
                        && let Some(raw) = entry.get(*tag)
                    {
                        projected.push(retained(dict, *tag, raw)
                            .ok_or_else(|| invalid(format!("Cannot carry group field {tag}.")))?);
                    }
                }
                Member::Group(target) if matches!(target.count_tag, 604) => {
                    if let Some(Member::Group(source)) = source.members.iter().find(|member| {
                        matches!(member, Member::Group(group) if group.count_tag == target.count_tag)
                    }) && let Some(group) = read_group(entry, source)? {
                        projected.extend(project_group(version, group, source, target)?);
                    }
                }
                _ => {}
            }
        }
        if projected.get(start).map(|(tag, _)| *tag) != fix_dict::schema::delimiter(target) {
            return Err(invalid(format!(
                "Group {} needs its first field.",
                target.count_tag
            )));
        }
    }
    Ok(projected)
}
