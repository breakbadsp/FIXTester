mod orders;

use fix_codec::{Fields, Message, tag};
use fix_dict::Version;
use std::collections::{BTreeMap, BTreeSet};

use super::Source;
use super::context::{OrderContext, read_order};
use crate::model::{OrderKind, wire_text};
use crate::{Result, invalid};
use orders::LiveOrder;
pub(crate) use orders::Plan;

pub const REQUESTS: [(OrderKind, &str); 3] = [
    (OrderKind::New, "New"),
    (OrderKind::Replace, "Replace"),
    (OrderKind::Cancel, "Cancel"),
];
const MAX_IDS: usize = 4096;
const MAX_LIVE: usize = 256;
const MAX_ID_LEN: usize = 128;
const MAX_REJECT_TEXT: usize = 120;
const SCALE: u64 = 1_000_000;
const INSTRUMENT_AND_CURRENCY: &[u32] = &[
    15, 22, 48, 54, 55, 65, 167, 200, 201, 202, 205, 206, 207, 223, 231, 460, 461, 541, 762,
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Response {
    #[default]
    Manual,
    Acknowledge,
    Reject,
    PartialFill,
    FullFill,
}

impl Response {
    pub const ALL: [Self; 5] = [
        Self::Manual,
        Self::Acknowledge,
        Self::Reject,
        Self::PartialFill,
        Self::FullFill,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::Acknowledge => "Acknowledge",
            Self::Reject => "Reject",
            Self::PartialFill => "Partial fill",
            Self::FullFill => "Full fill",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Rule {
    pub response: Response,
    pub quantity: String,
    pub price: String,
    pub text: String,
}

#[derive(Debug, Default, Clone)]
pub struct Settings {
    pub enabled: bool,
    pub rules: [Rule; 3],
}

#[derive(Debug, Clone)]
pub(crate) struct Rules([RuleAction; 3]);

#[derive(Debug, Clone)]
enum RuleAction {
    Manual,
    Acknowledge,
    Reject(String),
    Fill {
        quantity: Option<u64>,
        price: Option<u64>,
    },
}

impl Settings {
    pub(crate) fn compile(&self) -> Result<Option<Rules>> {
        if !self.enabled {
            return Ok(None);
        }
        let compile = |index: usize| {
            self.rules[index]
                .compile()
                .map_err(|err| invalid(format!("Automation for {}. {err}", REQUESTS[index].1)))
        };
        let rules = [compile(0)?, compile(1)?, compile(2)?];
        Ok(rules
            .iter()
            .any(|rule| !matches!(rule, RuleAction::Manual))
            .then_some(Rules(rules)))
    }
}

impl Rule {
    fn compile(&self) -> Result<RuleAction> {
        Ok(match self.response {
            Response::Manual => RuleAction::Manual,
            Response::Acknowledge => RuleAction::Acknowledge,
            Response::Reject => {
                let text = self.text.trim();
                if !text.is_empty() && !wire_text(text, MAX_REJECT_TEXT) {
                    return Err(invalid(
                        "Enter up to 120 printable ASCII characters for rejection text.",
                    ));
                }
                RuleAction::Reject(text.into())
            }
            Response::PartialFill | Response::FullFill => {
                let quantity = (self.response == Response::PartialFill)
                    .then(|| {
                        positive(
                            self.quantity.trim(),
                            "Enter a positive partial quantity with up to six decimal places.",
                        )
                    })
                    .transpose()?;
                let price = self.price.trim();
                let price = (!price.is_empty())
                    .then(|| {
                        positive(
                            price,
                            "Enter a positive fill price with up to six decimal places.",
                        )
                    })
                    .transpose()?;
                RuleAction::Fill { quantity, price }
            }
        })
    }
}

fn positive(value: &str, error: &str) -> Result<u64> {
    let parse = || {
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        let fraction = fraction.trim_end_matches('0');
        if !whole.bytes().all(|byte| byte.is_ascii_digit())
            || fraction.len() > 6
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let mut scaled = whole.parse::<u64>().ok()?.checked_mul(SCALE)?;
        if !fraction.is_empty() {
            let exponent = u32::try_from(6 - fraction.len()).ok()?;
            scaled = scaled.checked_add(fraction.parse::<u64>().ok()? * 10_u64.pow(exponent))?;
        }
        (scaled > 0).then_some(scaled)
    };
    parse().ok_or_else(|| invalid(error))
}

fn decimal(value: u64) -> String {
    let fraction = value % SCALE;
    if fraction == 0 {
        (value / SCALE).to_string()
    } else {
        format!("{}.{fraction:06}", value / SCALE)
            .trim_end_matches('0')
            .to_owned()
    }
}

pub(crate) const SKIPPED: &str = "Automation skipped: ";

#[derive(Debug)]
pub(crate) struct Automation<'a> {
    rules: &'a Rules,
    seen: BTreeSet<String>,
    live: BTreeMap<String, LiveOrder>,
}

impl<'a> Automation<'a> {
    pub(crate) fn new(rules: &'a Rules) -> Self {
        Self {
            rules,
            seen: BTreeSet::new(),
            live: BTreeMap::new(),
        }
    }

    pub(crate) fn clear_live(&mut self) {
        self.live.clear();
    }

    pub(crate) fn plan(
        &mut self,
        version: Version,
        message: &Message<'_>,
        next_id: impl FnOnce() -> Result<String>,
    ) -> Result<Option<Plan>> {
        let (index, source, kind) = match message.msg_type() {
            b"D" => (0, Source::New, "D"),
            b"G" => (1, Source::Replace, "G"),
            b"F" => (2, Source::Cancel, "F"),
            _ => return Ok(None),
        };
        let fields = message.fields();
        let rule = &self.rules.0[index];
        let id = identifier(fields, 11)?;
        if self.seen.contains(id) {
            return Err(invalid(
                "This ClOrdID is already in use on this connection.",
            ));
        }
        if self.seen.len() >= MAX_IDS {
            return Err(invalid(
                "The connection reached the automation request limit.",
            ));
        }
        self.seen.insert(id.to_owned());
        let manual = matches!(rule, RuleAction::Manual);
        let resent = [tag::POSS_DUP_FLAG, tag::POSS_RESEND]
            .into_iter()
            .any(|flag| fields.get_bool(flag) != Some(false));
        if manual || resent {
            if source != Source::New
                && let Ok(original) = identifier(fields, 41)
            {
                self.live.remove(original);
            }
            return if manual {
                Ok(None)
            } else {
                Err(invalid("Handle resent requests manually."))
            };
        }
        let terms = read_terms(version, kind, fields)?;
        if source == Source::New {
            if self.live.len() >= MAX_LIVE
                && !matches!(
                    rule,
                    RuleAction::Reject(_) | RuleAction::Fill { quantity: None, .. }
                )
            {
                return Err(invalid("The connection reached the live order limit."));
            }
            return Plan::new_order(version, terms, rule, next_id).map(Some);
        }
        let original = identifier(fields, 41)?;
        let order = self.live.get(original).ok_or_else(|| {
            invalid("OrigClOrdID has no live automated order on this connection.")
        })?;
        Plan::change_order(version, source, order, terms, rule).map(Some)
    }

    pub(crate) fn apply(&mut self, plan: Plan) {
        if let Some(previous) = plan.previous {
            self.live.remove(&previous);
        }
        if let Some(order) = plan.live {
            self.live.insert(order.terms.value(11).to_owned(), order);
        }
    }
}

fn identifier(fields: Fields<'_>, tag: u32) -> Result<&str> {
    fields
        .get(tag)
        .and_then(|value| std::str::from_utf8(value).ok())
        .filter(|value| wire_text(value, MAX_ID_LEN))
        .ok_or_else(|| {
            invalid(format!(
                "Tag {tag} needs an identifier with 1 to {MAX_ID_LEN} printable ASCII characters."
            ))
        })
}

fn read_terms(version: Version, kind: &str, fields: Fields<'_>) -> Result<OrderContext> {
    let dict = version.dictionary();
    let (Some(request), Some(report)) = (dict.message(kind), dict.message("8")) else {
        return Err(invalid("This request is not in the FIX dictionary."));
    };
    if request.body.groups.iter().any(|group| {
        fields
            .get(group.count_tag)
            .is_some_and(|count| count != b"0")
    }) || [66, 99, 152, 468, 442]
        .iter()
        .any(|tag| fields.get(*tag).is_some())
    {
        return Err(invalid(
            "Use manual actions for groups, list orders, stop orders, or alternate quantities.",
        ));
    }
    let mut seen = BTreeSet::new();
    if fields.iter().any(|(tag, _)| !seen.insert(tag)) {
        return Err(invalid("This request contains repeated FIX tags."));
    }
    let order = read_order(version, fields, &request.body, &report.body)?;
    for tag in [11, 37, 38, 40, 41, 44, 59, 423]
        .into_iter()
        .chain(INSTRUMENT_AND_CURRENCY.iter().copied())
    {
        if fields
            .get(tag)
            .is_some_and(|raw| raw != order.value(tag).as_bytes())
        {
            return Err(invalid(format!("Check tag {tag}.")));
        }
    }
    if !matches!(order.value(54), "1" | "2")
        || (order.value(55).is_empty() && order.value(48).is_empty())
        || (order.value(48).is_empty() != order.value(22).is_empty())
    {
        return Err(invalid(
            "Supply Buy or Sell and a symbol or a security identifier pair.",
        ));
    }
    Ok(order)
}
