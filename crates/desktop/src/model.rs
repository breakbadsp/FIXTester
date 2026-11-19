use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use fix_codec::{Fields, Message, UtcMillis, tag, write_utc_timestamp};
use fix_dict::{Version, schema::Dictionary, value::matches_type};
use fix_session::Role;

use crate::{Result, invalid};

pub const MESSAGE_MAX: usize = 16 * 1024;
pub const FIELDS_MAX: usize = 256;
pub const SESSIONS_MAX: usize = 8;
pub const TEXT_MAX: usize = 1024;

#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub version: Version,
    pub sender: String,
    pub target: String,
    pub role: Role,
    pub address: SocketAddr,
    pub heartbeat: u16,
}

impl Profile {
    pub fn validate(&self) -> Result<()> {
        if !wire_text(&self.name, 80)
            || !wire_text(&self.sender, 64)
            || !wire_text(&self.target, 64)
            || self.sender == self.target
        {
            return Err(invalid(
                "Enter a name and two different CompIDs in printable ASCII.",
            ));
        }
        if self.address.port() == 0 || !(1..=3600).contains(&self.heartbeat) {
            return Err(invalid(
                "Enter a nonzero port and a heartbeat from 1 to 3600 seconds.",
            ));
        }
        Ok(())
    }
}

pub fn now() -> UtcMillis {
    UtcMillis(
        u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or_default(),
    )
}

fn wire_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && value.bytes().all(|b| (32..=126).contains(&b))
}

#[derive(Debug)]
pub struct Application {
    pub(crate) version: Version,
    pub(crate) kind: String,
    pub(crate) fields: Vec<(u32, String)>,
}

impl Application {
    pub fn tags(version: Version, kind: &str, text: &str) -> Result<Self> {
        if text.len() > MESSAGE_MAX {
            return Err(invalid("The tag editor exceeds 16 KiB."));
        }
        let fields = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let (tag, value) = line
                    .split_once('=')
                    .ok_or_else(|| invalid("Use one Tag=Value pair per line."))?;
                let tag = tag
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| invalid("Enter a numeric tag."))?;
                Ok((tag, value.to_owned()))
            })
            .collect::<Result<Vec<_>>>()?;
        Self::new(version, kind, fields)
    }

    fn new(version: Version, kind: &str, fields: Vec<(u32, String)>) -> Result<Self> {
        let dict = version.dictionary();
        if kind.is_empty()
            || kind.len() > 16
            || !kind.bytes().all(|b| b.is_ascii_alphanumeric())
            || tag::msg_type::is_admin(kind.as_bytes())
        {
            return Err(invalid(
                "Enter an application MsgType with 1 to 16 letters or digits.",
            ));
        }
        if fields.len() > FIELDS_MAX - 8 {
            return Err(invalid("The message has too many fields."));
        }
        for (number, value) in &fields {
            if reserved(dict, *number) {
                return Err(invalid(format!(
                    "Tag {number} is reserved or requires a length/data pair."
                )));
            }
            if !wire_text(value, TEXT_MAX) {
                return Err(invalid(format!(
                    "Enter 1 to {TEXT_MAX} printable ASCII characters for tag {number}."
                )));
            }
        }
        Ok(Self {
            version,
            kind: kind.to_owned(),
            fields,
        })
    }
}

fn reserved(dict: &Dictionary, number: u32) -> bool {
    number == 0
        || dict.header.contains(number)
        || dict.trailer.contains(number)
        || tag::DATA_FIELDS
            .iter()
            .any(|&(length, data)| number == length || number == data)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderKind {
    New,
    Replace,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderValues {
    pub symbol: String,
    pub side: String,
    pub quantity: String,
    pub price: String,
}

impl Default for OrderValues {
    fn default() -> Self {
        Self {
            symbol: String::new(),
            side: "1".into(),
            quantity: String::new(),
            price: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Order {
    pub kind: OrderKind,
    pub values: OrderValues,
    pub original: String,
    carried: Vec<(u32, String)>,
}

impl Order {
    pub fn new(kind: OrderKind, values: OrderValues, original: String) -> Self {
        Self {
            kind,
            values,
            original,
            carried: Vec::new(),
        }
    }

    pub fn from_sent(kind: OrderKind, version: Version, message: Message<'_>) -> Result<Self> {
        let fields = message.fields();
        if kind == OrderKind::Replace && fields.get(40) != Some(b"2") {
            return Err(invalid(
                "The compact Replace form requires a sent Limit order.",
            ));
        }
        let value = |tag| {
            fields
                .get(tag)
                .and_then(|raw| std::str::from_utf8(raw).ok())
                .unwrap_or_default()
                .to_owned()
        };
        let mut order = Self::new(
            kind,
            OrderValues {
                symbol: value(55),
                side: value(54),
                quantity: value(38),
                price: value(44),
            },
            value(11),
        );
        order.carried = selected_fields(version, kind, message)?;
        Ok(order)
    }

    pub fn carried(&self) -> &[(u32, String)] {
        &self.carried
    }

    pub fn validate(&self) -> Result<()> {
        if !wire_text(&self.values.symbol, 64) || !matches!(self.values.side.as_str(), "1" | "2") {
            return Err(invalid("Enter a symbol and select Buy or Sell."));
        }
        if self.kind != OrderKind::New && !wire_text(&self.original, 128) {
            return Err(invalid("Enter the original ClOrdID."));
        }
        if !positive_decimal(&self.values.quantity) {
            return Err(invalid("Enter a positive decimal order quantity."));
        }
        if self.kind != OrderKind::Cancel && !positive_decimal(&self.values.price) {
            return Err(invalid("Enter a positive decimal limit price."));
        }
        Ok(())
    }

    pub(crate) fn application(
        &self,
        version: Version,
        id: String,
        time: UtcMillis,
    ) -> Result<Application> {
        let values = &self.values;
        let mut timestamp = [0; 21];
        let time = String::from_utf8_lossy(write_utc_timestamp(time, &mut timestamp)).into_owned();
        let mut fields = vec![(11, id)];
        if self.kind != OrderKind::New {
            fields.push((41, self.original.clone()));
        }
        if self.kind != OrderKind::Cancel {
            if version != Version::Fix44 && !self.carried.iter().any(|(tag, _)| *tag == 21) {
                fields.push((21, "1".into()));
            }
            fields.extend([(40, "2".into()), (44, values.price.clone())]);
        }
        fields.extend([
            (55, values.symbol.clone()),
            (54, values.side.clone()),
            (60, time),
            (38, values.quantity.clone()),
        ]);
        fields.extend_from_slice(&self.carried);
        Application::new(version, self.kind.msg_type(), fields)
    }
}

impl OrderKind {
    fn msg_type(self) -> &'static str {
        match self {
            Self::New => "D",
            Self::Replace => "G",
            Self::Cancel => "F",
        }
    }
}

fn selected_fields(
    version: Version,
    kind: OrderKind,
    message: Message<'_>,
) -> Result<Vec<(u32, String)>> {
    let dict = version.dictionary();
    let source_kind = std::str::from_utf8(message.msg_type()).unwrap_or_default();
    let source = dict
        .message(source_kind)
        .ok_or_else(|| invalid("Unknown source order type."))?;
    let target = dict
        .message(kind.msg_type())
        .ok_or_else(|| invalid("Unknown order type."))?;
    let mut fields = message.fields();
    let mut selected = Vec::new();
    while let Some((number, raw)) = fields.iter().next() {
        if let Some(definition) = source
            .body
            .group(number)
            .or_else(|| target.body.group(number))
        {
            let length = group_length(fields, definition).ok_or_else(|| {
                invalid(format!("The selected order has invalid group {number}."))
            })?;
            if let Some(destination) = target.body.group(number)
                && group_length(fields, destination) == Some(length)
                && !selected.iter().any(|(tag, _)| *tag == number)
            {
                selected.extend(
                    fields
                        .iter()
                        .take(length)
                        .map(|(tag, raw)| retained(dict, tag, raw))
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            invalid(format!("Cannot carry group {number} into the new message."))
                        })?,
                );
            }
            fields = fields.skip(length);
            continue;
        }
        if target.body.contains(number)
            && !matches!(number, 11 | 38 | 40 | 41 | 44 | 54 | 55 | 60)
            && !selected.iter().any(|(tag, _)| *tag == number)
            && let Some(field) = retained(dict, number, raw)
        {
            selected.push(field);
        }
        fields = fields.skip(1);
    }
    Ok(selected)
}

fn group_length(fields: Fields<'_>, def: &fix_codec::GroupDef<'_>) -> Option<usize> {
    fields
        .group(def)
        .ok()
        .flatten()
        .map(|group| group.entries().map(|entry| entry.len()).sum::<usize>() + 1)
}

fn retained(dict: &Dictionary, number: u32, raw: &[u8]) -> Option<(u32, String)> {
    let value = std::str::from_utf8(raw).ok()?;
    let field = dict.field(number)?;
    (!reserved(dict, number)
        && wire_text(value, TEXT_MAX)
        && matches_type(field.ty, raw)
        && field.accepts_value(value))
    .then(|| (number, value.to_owned()))
}

fn positive_decimal(text: &str) -> bool {
    let mut parts = text.split('.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    text.len() <= 24
        && digits(whole)
        && fraction.is_none_or(digits)
        && parts.next().is_none()
        && text.bytes().any(|b| (b'1'..=b'9').contains(&b))
}
