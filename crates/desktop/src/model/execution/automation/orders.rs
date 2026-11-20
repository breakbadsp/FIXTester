use fix_dict::Version;

use super::super::{Action, Report, Source, context::OrderContext};
use super::{INSTRUMENT_AND_CURRENCY, RuleAction, decimal, positive};
use crate::{Result, invalid};

#[derive(Debug, Clone)]
pub(super) struct LiveOrder {
    pub(super) terms: OrderContext,
    order_id: String,
    quantity: u64,
    limit: Option<u64>,
    cum: u64,
    notional: u128,
    replaced: bool,
}

#[derive(Debug)]
pub(crate) struct Plan {
    pub(crate) reports: Vec<Report>,
    pub(super) previous: Option<String>,
    pub(super) live: Option<LiveOrder>,
}

impl Plan {
    pub(super) fn new_order(
        version: Version,
        terms: OrderContext,
        rule: &RuleAction,
        next_id: impl FnOnce() -> Result<String>,
    ) -> Result<Self> {
        let mut order = LiveOrder::new(terms, next_id()?)?;
        if let RuleAction::Reject(text) = rule {
            let mut report = order.report(version, Source::New, Action::Reject);
            report.set(103, "0");
            report.set(58, text.as_str());
            return Self::finish(vec![report], None, None);
        }
        let mut reports = vec![order.report(version, Source::New, Action::Acknowledge)];
        reports.extend(order.fill(version, rule)?);
        let live = (order.leaves() > 0).then_some(order);
        Self::finish(reports, None, live)
    }

    pub(super) fn change_order(
        version: Version,
        source: Source,
        original: &LiveOrder,
        request: OrderContext,
        rule: &RuleAction,
    ) -> Result<Self> {
        original.check_reference(source, &request)?;
        if let RuleAction::Reject(text) = rule {
            let report = original.reject_request(version, source, &request, "2", text);
            return Self::finish(vec![report], None, None);
        }

        let mut order = original.clone();
        let previous = Some(order.terms.value(11).to_owned());
        if source == Source::Replace {
            order.replace(request)?;
            let mut report = order.report(version, source, Action::Replace);
            report.set(39, order.status(version));
            let mut reports = vec![report];
            if order.leaves() > 0 {
                reports.extend(order.fill(version, rule)?);
            }
            let live = (order.leaves() > 0).then_some(order);
            return Self::finish(reports, previous, live);
        }

        let mut reports = order.fill(version, rule)?.into_iter().collect::<Vec<_>>();
        if order.leaves() == 0 {
            reports.push(order.reject_request(
                version,
                source,
                &request,
                "0",
                "Too late to cancel.",
            ));
        } else {
            let mut report = order.report(version, source, Action::Cancel);
            use_request_ids(&mut report, &request);
            reports.push(report);
        }
        Self::finish(reports, previous, None)
    }

    fn finish(
        reports: Vec<Report>,
        previous: Option<String>,
        live: Option<LiveOrder>,
    ) -> Result<Self> {
        for report in &reports {
            report.validate()?;
        }
        Ok(Self {
            reports,
            previous,
            live,
        })
    }
}

impl LiveOrder {
    fn new(terms: OrderContext, order_id: String) -> Result<Self> {
        let (quantity, limit) = Self::read_size(&terms)?;
        Ok(Self {
            terms,
            order_id,
            quantity,
            limit,
            cum: 0,
            notional: 0,
            replaced: false,
        })
    }

    fn read_size(terms: &OrderContext) -> Result<(u64, Option<u64>)> {
        if !matches!(terms.value(59), "" | "0" | "1") {
            return Err(invalid(
                "Use manual actions for orders other than Day or GTC.",
            ));
        }
        if !matches!(terms.value(423), "" | "2") {
            return Err(invalid(
                "Use manual actions for prices other than per-unit prices.",
            ));
        }
        let quantity = positive(
            terms.value(38),
            "Supply a positive OrderQty with up to six decimal places.",
        )?;
        let limit = match terms.value(40) {
            "1" if terms.value(44).is_empty() => None,
            "2" => Some(positive(
                terms.value(44),
                "Supply a positive limit price with up to six decimal places.",
            )?),
            _ => {
                return Err(invalid(
                    "Use a Market order without Price or a Limit order with Price.",
                ));
            }
        };
        Ok((quantity, limit))
    }

    fn check_reference(&self, source: Source, request: &OrderContext) -> Result<()> {
        if !request.value(37).is_empty() && request.value(37) != self.order_id {
            return Err(invalid("OrderID does not match the live automated order."));
        }
        let changed = INSTRUMENT_AND_CURRENCY.iter().any(|&tag| {
            let value = request.value(tag);
            (source == Source::Replace || !value.is_empty()) && value != self.terms.value(tag)
        });
        if changed {
            return Err(invalid(
                "The instrument, currency, or side does not match the live order.",
            ));
        }
        Ok(())
    }

    fn replace(&mut self, mut terms: OrderContext) -> Result<()> {
        let (quantity, limit) = Self::read_size(&terms)?;
        // C.3.c cancels the balance and keeps the executed quantity.
        if quantity < self.cum
            && let Some((_, value)) = terms.fields.iter_mut().find(|(tag, _)| *tag == 38)
        {
            *value = decimal(self.cum);
        }
        // A missing TimeInForce means Day. The replacement supplies the new terms.
        self.terms = terms;
        self.quantity = quantity.max(self.cum);
        self.limit = limit;
        self.replaced = true;
        Ok(())
    }

    fn leaves(&self) -> u64 {
        self.quantity - self.cum
    }

    fn status(&self, version: Version) -> &'static str {
        if self.leaves() == 0 {
            "2"
        } else if self.cum != 0 {
            "1"
        } else if self.replaced && version == Version::Fix42 {
            "5"
        } else {
            "0"
        }
    }

    fn fill(&mut self, version: Version, rule: &RuleAction) -> Result<Option<Report>> {
        let RuleAction::Fill { quantity, price } = rule else {
            return Ok(None);
        };
        let leaves = self.leaves();
        let size = match quantity {
            Some(size) if *size < leaves => *size,
            Some(_) => {
                return Err(invalid(
                    "Partial quantity must be less than the remaining quantity.",
                ));
            }
            None if leaves > 0 => leaves,
            None => return Err(invalid("There is no remaining quantity to fill.")),
        };
        let price = price
            .or(self.limit)
            .ok_or_else(|| invalid("Set a fill price override to fill a Market order."))?;
        self.cum += size;
        // Total size and each price fit in u64, so their weighted sum fits in u128.
        self.notional += u128::from(size) * u128::from(price);
        let action = if self.leaves() == 0 {
            Action::FullFill
        } else {
            Action::PartialFill
        };
        // Trades use the accepted ClOrdID. The change request IDs belong on the response.
        let mut report = self.report(version, Source::New, action);
        report.set(31, decimal(price));
        report.set(32, decimal(size));
        Ok(Some(report))
    }

    fn report(&self, version: Version, source: Source, action: Action) -> Report {
        let mut report =
            Report::for_order(version, source, &self.terms, &self.order_id, "", action);
        if version == Version::Fix44 {
            report.field(947, self.terms.value(947), false, true);
        }
        let average = if self.cum == 0 {
            0
        } else {
            let cum = u128::from(self.cum);
            // The weighted mean cannot exceed the largest fill price.
            u64::try_from((self.notional + cum / 2) / cum).unwrap_or(u64::MAX)
        };
        report.set(14, decimal(self.cum));
        report.set(6, decimal(average));
        let leaves = if matches!(action, Action::Reject | Action::Cancel) {
            0
        } else {
            self.leaves()
        };
        report.set(151, decimal(leaves));
        report
    }

    fn reject_request(
        &self,
        version: Version,
        source: Source,
        request: &OrderContext,
        reason: &str,
        text: &str,
    ) -> Report {
        let action = if source == Source::Replace {
            Action::RejectReplace
        } else {
            Action::RejectCancel
        };
        let mut report = self.report(version, source, action);
        use_request_ids(&mut report, request);
        report.set(39, self.status(version));
        report.set(102, reason);
        report.set(58, text);
        report
    }
}

fn use_request_ids(report: &mut Report, request: &OrderContext) {
    report.set(11, request.value(11));
    report.set(41, request.value(41));
}
