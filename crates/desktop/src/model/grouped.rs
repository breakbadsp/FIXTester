use fix_codec::{GroupDef, Member, UtcMillis, write_utc_timestamp};
use fix_dict::{Version, schema::FieldType, value::matches_type};

use super::{Application, Order, OrderKind, OrderValues, positive_decimal, wire_text};
use crate::{Result, invalid};

pub const GROUP_ROWS_MAX: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Side {
    #[default]
    Buy,
    Sell,
}

impl Side {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Buy => "1",
            Self::Sell => "2",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Contract {
    #[default]
    Future,
    Call,
    Put,
}

#[derive(Debug, Clone, Default)]
pub struct Leg {
    pub symbol: String,
    pub maturity: String,
    pub contract: Contract,
    pub strike: String,
    pub ratio: String,
    pub side: Side,
}

impl Leg {
    fn fields(&self, version: Version) -> Result<Vec<(u32, String)>> {
        if !wire_text(&self.symbol, 64) {
            return Err(invalid("Enter a leg symbol."));
        }
        let maturity = self.maturity.as_bytes();
        let maturity_tag = if version == Version::Fix43 {
            match maturity.len() {
                6 if matches_type(FieldType::MonthYear, maturity) => 610,
                8 if matches_type(FieldType::LocalMktDate, maturity) => 611,
                _ => return Err(invalid("Enter FIX 4.3 leg maturity as YYYYMM or YYYYMMDD.")),
            }
        } else if matches_type(FieldType::MonthYear, maturity) {
            610
        } else {
            return Err(invalid(
                "Enter leg maturity as YYYYMM, YYYYMMDD, or YYYYMMwN.",
            ));
        };
        if !positive_decimal(&self.ratio) {
            return Err(invalid("Enter a positive leg ratio."));
        }
        let mut fields = vec![
            (600, self.symbol.clone()),
            (maturity_tag, self.maturity.clone()),
            (623, self.ratio.clone()),
            (624, self.side.wire().into()),
        ];
        let (cfi, security_type) = match self.contract {
            Contract::Future => ("FXXXXX", "FUT"),
            Contract::Call => ("OCXXXX", "OPT"),
            Contract::Put => ("OPXXXX", "OPT"),
        };
        fields.push((608, cfi.into()));
        if version == Version::Fix44 {
            fields.push((609, security_type.into()));
        }
        if self.contract != Contract::Future {
            if !positive_decimal(&self.strike) {
                return Err(invalid("Enter a positive option strike."));
            }
            fields.push((612, self.strike.clone()));
        }
        Ok(fields)
    }
}

#[derive(Debug, Clone)]
pub enum GroupedOrder {
    Multileg(MultilegOrder),
    List(Vec<OrderValues>),
}

#[derive(Debug, Clone)]
pub struct MultilegOrder {
    pub symbol: String,
    pub quantity: String,
    pub price: String,
    pub cfi: String,
    pub legs: Vec<Leg>,
}

impl GroupedOrder {
    pub(crate) fn application(
        &self,
        version: Version,
        id: String,
        time: UtcMillis,
    ) -> Result<Application> {
        let mut timestamp = [0; 21];
        let time = String::from_utf8_lossy(write_utc_timestamp(time, &mut timestamp));
        match self {
            Self::Multileg(order) => order.application(version, id, &time),
            Self::List(orders) => order_list(version, &id, &time, orders),
        }
    }
}

impl MultilegOrder {
    fn application(&self, version: Version, id: String, time: &str) -> Result<Application> {
        let Self {
            symbol,
            quantity,
            price,
            cfi,
            legs,
        } = self;
        let group = group(version, "AB", 555)?;
        if !(2..=GROUP_ROWS_MAX).contains(&legs.len()) {
            return Err(invalid("Enter 2 to 16 legs."));
        }
        if !wire_text(symbol, 64) || !positive_decimal(quantity) {
            return Err(invalid("Enter a strategy symbol and positive quantity."));
        }
        if price.len() > 24 || !matches_type(FieldType::Price, price.as_bytes()) {
            return Err(invalid("Enter a decimal net limit price."));
        }
        // FIX 4.3 and 4.4 Volume 7, page 27, specify FMXXS (five letters) and OMXXXN.
        let cfi = if cfi.is_empty() {
            if legs.iter().all(|leg| leg.contract == Contract::Future) {
                "FMXXS"
            } else {
                "OMXXXN"
            }
        } else if cfi.len() == 6 && cfi.bytes().all(|byte| byte.is_ascii_uppercase()) {
            cfi
        } else {
            return Err(invalid("Enter a six-letter uppercase strategy CFI code."));
        };
        let mut fields = vec![(11, id)];
        if version == Version::Fix43 {
            fields.push((21, "1".into()));
        }
        fields.extend([
            (55, symbol.clone()),
            (167, "MLEG".into()),
            (461, cfi.into()),
            (54, "B".into()),
            (60, time.into()),
            (38, quantity.clone()),
            (40, "2".into()),
            (44, price.clone()),
            (555, legs.len().to_string()),
        ]);
        for (index, leg) in legs.iter().enumerate() {
            let values = leg
                .fields(version)
                .map_err(|error| invalid(format!("Leg {}. {error}", index + 1)))?;
            append_entry(group, &mut fields, values)?;
        }
        Application::new(version, "AB", fields)
    }
}

fn order_list(
    version: Version,
    id: &str,
    time: &str,
    orders: &[OrderValues],
) -> Result<Application> {
    let group = group(version, "E", 73)?;
    if !(1..=GROUP_ROWS_MAX).contains(&orders.len()) {
        return Err(invalid("Enter 1 to 16 orders."));
    }
    let count = orders.len().to_string();
    let mut fields = vec![
        (66, id.into()),
        (394, "3".into()),
        (68, count.clone()),
        (73, count),
    ];
    for (index, values) in orders.iter().enumerate() {
        let number = index + 1;
        Order::new(OrderKind::New, values.clone(), String::new())
            .validate()
            .map_err(|error| invalid(format!("Order {number}. {error}")))?;
        let mut entry = vec![
            (11, format!("{id}-{number}")),
            (67, number.to_string()),
            (55, values.symbol.clone()),
            (54, values.side.clone()),
            (60, time.into()),
            (38, values.quantity.clone()),
            (40, "2".into()),
            (44, values.price.clone()),
        ];
        if version != Version::Fix44 {
            entry.push((21, "1".into()));
        }
        append_entry(group, &mut fields, entry)?;
    }
    Application::new(version, "E", fields)
}

fn group(version: Version, kind: &str, count_tag: u32) -> Result<&'static GroupDef<'static>> {
    version
        .dictionary()
        .message(kind)
        .and_then(|message| {
            message
                .body
                .groups
                .iter()
                .find(|group| group.count_tag == count_tag)
        })
        .copied()
        .ok_or_else(|| {
            invalid(format!(
                "MsgType {kind} is unavailable for this FIX version."
            ))
        })
}

fn append_entry(
    group: &GroupDef<'_>,
    output: &mut Vec<(u32, String)>,
    mut entry: Vec<(u32, String)>,
) -> Result<()> {
    for member in group.members {
        if let Member::Field(tag) = member
            && let Some(index) = entry.iter().position(|(number, _)| number == tag)
        {
            output.push(entry.swap_remove(index));
        }
    }
    if entry.is_empty() {
        Ok(())
    } else {
        Err(invalid(
            "An order field is unavailable for this FIX version.",
        ))
    }
}
