use eframe::egui;
use fix_dict::Version;
use fixtester_desktop::model::OrderValues;
use fixtester_desktop::model::grouped::{
    BidType, Contract, GROUP_ROWS_MAX, GroupedOrder, Leg, Side,
};
use fixtester_desktop::transport::Send;

use super::{Dialog, Workspace, field, submit};

pub(in crate::ui) struct GroupedDraft {
    pub(super) id: i64,
    pub(super) waiting: Option<u64>,
    order: GroupedOrder,
}

impl Workspace {
    pub(in crate::ui) fn open_grouped(&mut self, multileg: bool) {
        let Some(saved) = self.selected() else {
            return;
        };
        let order = if multileg {
            if saved.profile.version == Version::Fix42 {
                return;
            }
            GroupedOrder::Multileg {
                symbol: String::new(),
                quantity: String::new(),
                price: String::new(),
                legs: vec![new_leg(Side::Buy), new_leg(Side::Sell)],
            }
        } else {
            GroupedOrder::List {
                bid_type: BidType::default(),
                orders: vec![OrderValues::default()],
            }
        };
        self.dialog = Some(Dialog::Grouped(GroupedDraft {
            id: saved.id,
            waiting: None,
            order,
        }));
        self.notice.clear();
    }

    pub(super) fn grouped_dialog(&mut self, ui: &mut egui::Ui, draft: &mut GroupedDraft) -> bool {
        match &mut draft.order {
            GroupedOrder::Multileg {
                symbol,
                quantity,
                price,
                legs,
            } => {
                ui.heading("New multileg limit order");
                ui.label("35=AB · The Buy or Sell direction comes from each leg.");
                egui::Grid::new("strategy").show(ui, |ui| {
                    field(ui, "Strategy symbol", symbol, 64);
                    field(ui, "Strategy quantity", quantity, 24);
                    field(ui, "Net limit price", price, 24);
                });
                entries(ui, legs, 2, "leg", leg_fields, || new_leg(Side::Buy));
            }
            GroupedOrder::List { bid_type, orders } => {
                ui.heading("New list of limit orders");
                ui.label("35=E · FIXTester assigns the list and order IDs.");
                egui::ComboBox::from_label("Bid type (394)")
                    .selected_text(bid_label(*bid_type))
                    .show_ui(ui, |ui| {
                        for kind in [BidType::None, BidType::NonDisclosed, BidType::Disclosed] {
                            ui.selectable_value(bid_type, kind, bid_label(kind));
                        }
                    });
                entries(ui, orders, 1, "order", list_fields, OrderValues::default);
            }
        }
        match submit(ui, "Send") {
            Some(false) => false,
            Some(true) => {
                match self.queue(draft.id, Send::GroupedOrder(draft.order.clone())) {
                    Ok(epoch) => {
                        draft.waiting = Some(epoch);
                        self.notice.clear();
                    }
                    Err(error) => self.notice = error.to_string(),
                }
                true
            }
            None => true,
        }
    }
}

fn entries<T>(
    ui: &mut egui::Ui,
    rows: &mut Vec<T>,
    minimum: usize,
    name: &str,
    fields: impl Fn(&mut egui::Ui, &mut T, usize),
    new: impl FnOnce() -> T,
) {
    let mut remove = None;
    let can_remove = rows.len() > minimum;
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            for (index, row) in rows.iter_mut().enumerate() {
                let label = if name == "leg" { "Leg" } else { "Order" };
                ui.push_id(index, |ui| {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.strong(format!("{label} {}", index + 1));
                        if ui
                            .add_enabled(
                                can_remove,
                                egui::Button::new(format!("Remove {name} {}", index + 1)),
                            )
                            .clicked()
                        {
                            remove = Some(index);
                        }
                    });
                    egui::Grid::new("entry").show(ui, |ui| fields(ui, row, index + 1));
                });
            }
        });
    if let Some(index) = remove {
        rows.remove(index);
    }
    if ui
        .add_enabled(
            rows.len() < GROUP_ROWS_MAX,
            egui::Button::new(format!("Add {name}")),
        )
        .clicked()
    {
        rows.push(new());
    }
}

fn new_leg(side: Side) -> Leg {
    Leg {
        side,
        ratio: "1".into(),
        ..Leg::default()
    }
}

fn leg_fields(ui: &mut egui::Ui, leg: &mut Leg, number: usize) {
    ui.label("Contract");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut leg.contract, Contract::Future, "Future");
        ui.selectable_value(&mut leg.contract, Contract::Call, "Call");
        ui.selectable_value(&mut leg.contract, Contract::Put, "Put");
    });
    ui.end_row();
    field(ui, &format!("Leg {number} symbol"), &mut leg.symbol, 64);
    field(ui, &format!("Leg {number} maturity"), &mut leg.maturity, 8)
        .on_hover_text("YYYYMM, YYYYMMDD, or YYYYMMwN");
    if leg.contract != Contract::Future {
        field(ui, "Strike", &mut leg.strike, 24);
    }
    field(ui, "Ratio", &mut leg.ratio, 24);
    ui.label("Side");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut leg.side, Side::Buy, "Buy");
        ui.selectable_value(&mut leg.side, Side::Sell, "Sell");
    });
    ui.end_row();
}

fn list_fields(ui: &mut egui::Ui, order: &mut OrderValues, _number: usize) {
    field(ui, "Symbol", &mut order.symbol, 64);
    ui.label("Side");
    ui.horizontal(|ui| {
        for (side, label) in [("1", "Buy"), ("2", "Sell")] {
            if ui.selectable_label(order.side == side, label).clicked() {
                order.side = side.into();
            }
        }
    });
    ui.end_row();
    field(ui, "Quantity", &mut order.quantity, 24);
    field(ui, "Limit price", &mut order.price, 24);
}

fn bid_label(kind: BidType) -> &'static str {
    match kind {
        BidType::None => "No bidding process",
        BidType::NonDisclosed => "Non-disclosed",
        BidType::Disclosed => "Disclosed",
    }
}
