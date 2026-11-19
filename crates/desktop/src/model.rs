use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use fix_codec::{UtcMillis, tag, write_utc_timestamp};
use fix_dict::Version;
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
            let is_data = tag::DATA_FIELDS
                .iter()
                .any(|(len, data)| number == len || number == data);
            if *number == 0
                || dict.header.contains(*number)
                || dict.trailer.contains(*number)
                || is_data
            {
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
}

impl Order {
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
            if version != Version::Fix44 {
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
        Application::new(
            version,
            match self.kind {
                OrderKind::New => "D",
                OrderKind::Replace => "G",
                OrderKind::Cancel => "F",
            },
            fields,
        )
    }
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
