mod additional;
mod automation;
pub(super) mod execution;
mod grouped;

use std::path::PathBuf;
use std::sync::mpsc::sync_channel;

use eframe::egui::{self, Key, Modifiers, RichText};
use fix_codec::Decoder;
use fix_dict::Version;
use fix_session::Role;
use fixtester_desktop::model::{
    Application, FIELDS_MAX, MESSAGE_MAX, Order, OrderKind, OrderValues, Profile,
};
use fixtester_desktop::store::{Direction, SavedProfile};
use fixtester_desktop::transport::Send;
use fixtester_desktop::{Result, invalid};

use super::{ACCENT, MUTED, Workspace};

pub(super) enum Dialog {
    Profile(ProfileDraft),
    Order(OrderDraft),
    Tags(TagDraft),
    Grouped(grouped::GroupedDraft),
    Execution(execution::ExecutionDraft),
    Automation(i64),
    Reset(i64),
    Export(String),
}

pub(super) struct ProfileDraft {
    id: Option<i64>,
    name: String,
    version: Version,
    sender: String,
    target: String,
    role: Role,
    address: String,
    heartbeat: String,
}

impl Default for ProfileDraft {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            version: Version::Fix44,
            sender: String::new(),
            target: String::new(),
            role: Role::Initiator,
            address: "127.0.0.1:9876".into(),
            heartbeat: "30".into(),
        }
    }
}

impl From<&SavedProfile> for ProfileDraft {
    fn from(saved: &SavedProfile) -> Self {
        let profile = &saved.profile;
        Self {
            id: Some(saved.id),
            name: profile.name.clone(),
            version: profile.version,
            sender: profile.sender.clone(),
            target: profile.target.clone(),
            role: profile.role,
            address: profile.address.to_string(),
            heartbeat: profile.heartbeat.to_string(),
        }
    }
}

pub(super) struct OrderDraft {
    id: i64,
    order: Order,
    additional: String,
    recalled_symbol: String,
    selected_reference: bool,
    focus: bool,
    waiting: Option<u64>,
}

pub(super) struct TagDraft {
    id: i64,
    version: Version,
    kind: String,
    text: String,
    waiting: Option<u64>,
}

impl Workspace {
    pub(super) fn open_order(&mut self, kind: OrderKind, from_row: bool) {
        let Some(id) = self.selected else { return };
        self.notice.clear();
        let mut order = Order::new(kind, OrderValues::default(), String::new());
        let mut selected_reference = false;
        if kind == OrderKind::New {
            match self.store.last_values(id, None) {
                Ok(Some(values)) => order.values = values,
                Ok(None) => {}
                Err(error) => self.notice = error.to_string(),
            }
        } else if from_row {
            match self.order_from_row(kind) {
                Ok(Some(selected)) => {
                    order = selected;
                    selected_reference = true;
                }
                Ok(None) => {}
                Err(error) => self.notice = error.to_string(),
            }
        }
        self.dialog = Some(Dialog::Order(OrderDraft {
            id,
            recalled_symbol: order.values.symbol.clone(),
            selected_reference,
            order,
            additional: String::new(),
            focus: true,
            waiting: None,
        }));
    }

    fn order_from_row(&self, kind: OrderKind) -> Result<Option<Order>> {
        let Some(saved) = self.selected() else {
            return Ok(None);
        };
        let Some(entry) = self
            .selected_row
            .and_then(|id| self.entries.iter().find(|row| row.id == id))
        else {
            return Ok(None);
        };
        if entry.direction != Direction::Sent
            || entry.status != "Sent"
            || !matches!(entry.kind.as_str(), "D" | "G")
        {
            return Ok(None);
        }
        let mut decoder = Decoder::new(FIELDS_MAX);
        let message = decoder
            .decode(&entry.raw)
            .map_err(|error| invalid(format!("Cannot read selected order: {error:?}")))?;
        Order::from_sent(kind, saved.profile.version, message).map(Some)
    }

    pub(super) fn open_tags(&mut self) {
        if let Some(saved) = self.selected() {
            self.dialog = Some(Dialog::Tags(TagDraft {
                id: saved.id,
                version: saved.profile.version,
                kind: String::new(),
                text: String::new(),
                waiting: None,
            }));
            self.notice.clear();
        }
    }

    pub(super) fn show_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        if self.send_finished(&mut dialog) {
            self.refresh = true;
            return;
        }
        let pending = match &dialog {
            Dialog::Order(draft) => draft.waiting.is_some(),
            Dialog::Tags(draft) => draft.waiting.is_some(),
            Dialog::Grouped(draft) => draft.waiting.is_some(),
            Dialog::Execution(draft) => draft.waiting.is_some(),
            _ => false,
        };
        let mut open = true;
        egui::Modal::new(egui::Id::unique("dialog")).show(ctx, |ui| {
            ui.set_width(370.0);
            if pending {
                ui.heading("Sending message");
                ui.spinner();
                return;
            }
            match &mut dialog {
                Dialog::Order(draft) => open = self.order_dialog(ui, draft),
                Dialog::Tags(draft) => open = self.tags_dialog(ui, draft),
                Dialog::Grouped(draft) => open = self.grouped_dialog(ui, draft),
                Dialog::Execution(draft) => open = self.execution_dialog(ui, draft),
                Dialog::Profile(draft) => open = self.profile_dialog(ui, draft),
                Dialog::Automation(id) => open = self.automation_dialog(ui, *id),
                Dialog::Reset(id) => open = self.reset_dialog(ui, *id),
                Dialog::Export(path) => open = self.export_dialog(ui, path),
            }
            if !self.notice.is_empty() && additional::error(&self.notice).is_none() {
                ui.label(RichText::new(&self.notice).color(ui.visuals().warn_fg_color));
            }
        });
        if open
            && (pending || !ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)))
        {
            self.dialog = Some(dialog);
        } else {
            self.notice.clear();
        }
    }

    fn send_finished(&mut self, dialog: &mut Dialog) -> bool {
        let (id, waiting) = match dialog {
            Dialog::Order(draft) => (draft.id, &mut draft.waiting),
            Dialog::Tags(draft) => (draft.id, &mut draft.waiting),
            Dialog::Grouped(draft) => (draft.id, &mut draft.waiting),
            Dialog::Execution(draft) => (draft.id, &mut draft.waiting),
            _ => return false,
        };
        let (Some(previous), Some(status)) = (*waiting, self.sessions.status(id)) else {
            return false;
        };
        if status.completed > previous {
            *waiting = None;
            if let Some(error) = status.send_error {
                self.notice = error;
                return false;
            }
            self.notice.clear();
            return true;
        }
        if !self.sessions.is_running(id) {
            *waiting = None;
            self.notice = status.notice;
        }
        false
    }

    fn order_dialog(&mut self, ui: &mut egui::Ui, draft: &mut OrderDraft) -> bool {
        let kind = draft.order.kind;
        ui.heading(match kind {
            OrderKind::New => "New limit order",
            OrderKind::Replace => "Replace limit order",
            OrderKind::Cancel => "Cancel order",
        });
        ui.add_space(6.0);
        self.order_fields(ui, draft);
        if draft.selected_reference && ui.link("Enter original details manually").clicked() {
            draft.order = Order::new(kind, OrderValues::default(), String::new());
            draft.selected_reference = false;
            self.notice.clear();
        }
        if !draft.order.carried().is_empty() {
            egui::CollapsingHeader::new("Carried FIX fields").show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(108.0)
                    .show(ui, |ui| {
                        for (tag, value) in draft.order.carried() {
                            ui.monospace(format!("{tag}={value}"));
                        }
                    });
            });
        }
        additional::show(ui, &mut draft.additional, &mut self.notice);
        if let Some(confirm) = submit(ui, "Send") {
            if !confirm {
                return false;
            }
            let result = draft.order.validate().and_then(|()| {
                self.queue(
                    draft.id,
                    Send::Order(draft.order.clone()),
                    &draft.additional,
                )
            });
            match result {
                Ok(epoch) => {
                    draft.waiting = Some(epoch);
                    self.notice.clear();
                }
                Err(error) => self.notice = error.to_string(),
            }
        }
        true
    }

    fn order_fields(&mut self, ui: &mut egui::Ui, draft: &mut OrderDraft) {
        let kind = draft.order.kind;
        let values = &mut draft.order.values;
        egui::Grid::new("order")
            .num_columns(2)
            .spacing([18.0, 8.0])
            .show(ui, |ui| {
                if draft.selected_reference {
                    ui.label("OrigClOrdID (read-only)");
                    ui.monospace(&draft.order.original);
                    ui.end_row();
                } else if kind != OrderKind::New {
                    field(ui, "OrigClOrdID", &mut draft.order.original, 128);
                }
                let symbol = field(ui, "Symbol", &mut values.symbol, 64);
                if kind == OrderKind::New {
                    if draft.focus {
                        symbol.request_focus();
                    }
                    if symbol.lost_focus() && values.symbol != draft.recalled_symbol {
                        draft.recalled_symbol.clone_from(&values.symbol);
                        match self.store.last_values(draft.id, Some(&values.symbol)) {
                            Ok(Some(saved)) => *values = saved,
                            Ok(None) => {
                                values.price.clear();
                            }
                            Err(error) => self.notice = error.to_string(),
                        }
                    }
                }
                ui.label("Side");
                egui::ComboBox::from_id_salt("side")
                    .selected_text(match values.side.as_str() {
                        "1" => "Buy",
                        "2" => "Sell",
                        _ => "Select side",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut values.side, "1".into(), "Buy");
                        ui.selectable_value(&mut values.side, "2".into(), "Sell");
                    });
                ui.end_row();
                field(ui, "Quantity", &mut values.quantity, 24);
                if kind != OrderKind::Cancel {
                    let price = field(ui, "Limit price", &mut values.price, 24);
                    if draft.focus && kind == OrderKind::Replace && !draft.order.original.is_empty()
                    {
                        price.request_focus();
                    }
                }
            });
        draft.focus = false;
    }

    fn tags_dialog(&mut self, ui: &mut egui::Ui, draft: &mut TagDraft) -> bool {
        ui.heading("Send application message");
        egui::Grid::new("tag-header").show(ui, |ui| {
            field(ui, "MsgType", &mut draft.kind, 16);
        });
        let tag_label = ui.label(
            RichText::new("One Tag=Value pair per line. FIXTester adds the header and checksum.")
                .color(MUTED),
        );
        ui.add(
            egui::TextEdit::multiline(&mut draft.text)
                .code_editor()
                .desired_width(360.0)
                .desired_rows(9)
                .char_limit(MESSAGE_MAX)
                .hint_text("11=DEMO-1\n9001=Example"),
        )
        .labelled_by(tag_label.id);
        if let Some(confirm) = submit(ui, "Send") {
            if !confirm {
                return false;
            }
            let result = Application::tags(draft.version, &draft.kind, &draft.text)
                .and_then(|message| self.queue(draft.id, Send::Tags(message), ""));
            match result {
                Ok(epoch) => {
                    draft.waiting = Some(epoch);
                    self.notice.clear();
                }
                Err(error) => self.notice = error.to_string(),
            }
        }
        true
    }

    fn queue(&self, id: i64, message: Send, additional: &str) -> Result<u64> {
        let epoch = self
            .sessions
            .status(id)
            .map_or(0, |status| status.completed);
        self.sessions
            .send_with_additional_tags(id, message, additional)?;
        Ok(epoch)
    }

    fn profile_dialog(&mut self, ui: &mut egui::Ui, draft: &mut ProfileDraft) -> bool {
        ui.heading(if draft.id.is_some() {
            "Edit profile"
        } else {
            "Add session profile"
        });
        profile_fields(ui, draft);
        match submit(ui, "Save profile") {
            Some(false) => false,
            Some(true) => match self.save_profile(draft) {
                Ok(()) => false,
                Err(error) => {
                    self.notice = error.to_string();
                    true
                }
            },
            None => true,
        }
    }

    fn save_profile(&mut self, draft: &ProfileDraft) -> Result<()> {
        if let Some(id) = draft.id {
            self.sessions.require_stopped(id)?;
        }
        let profile = Profile {
            name: draft.name.trim().into(),
            version: draft.version,
            sender: draft.sender.trim().into(),
            target: draft.target.trim().into(),
            role: draft.role,
            address: draft.address.trim().parse().map_err(|_| {
                invalid("Enter a numeric address and port, for example 127.0.0.1:9876.")
            })?,
            heartbeat: draft
                .heartbeat
                .parse()
                .map_err(|_| invalid("Enter the heartbeat in whole seconds."))?,
        };
        let id = self.store.save_profile(draft.id, &profile)?;
        self.profiles = self.store.profiles()?;
        self.disarm_automation(id);
        self.select(id);
        Ok(())
    }

    fn reset_dialog(&mut self, ui: &mut egui::Ui, id: i64) -> bool {
        ui.heading("Reset sequence numbers?");
        ui.label("This sets the next send and receive numbers to 1. Set up the peer to use ResetSeqNumFlag=Y on the next Logon.");
        match submit(ui, "Reset to 1") {
            Some(false) => false,
            Some(true) => match self.sessions.reset(&self.store, id) {
                Ok(()) => {
                    self.disarm_automation(id);
                    false
                }
                Err(error) => {
                    self.notice = error.to_string();
                    true
                }
            },
            None => true,
        }
    }

    fn export_dialog(&mut self, ui: &mut egui::Ui, filename: &mut String) -> bool {
        ui.heading("Export matching FIX bytes");
        ui.label("Exports captured bytes in timeline order to a new file.");
        ui.add(
            egui::TextEdit::singleline(filename)
                .desired_width(360.0)
                .char_limit(4096)
                .hint_text("Path to a new .fix file"),
        );
        match submit(ui, "Export") {
            Some(false) => false,
            Some(true) => match self.begin_export(filename) {
                Ok(()) => false,
                Err(error) => {
                    self.notice = error.to_string();
                    true
                }
            },
            None => true,
        }
    }

    fn begin_export(&mut self, filename: &str) -> Result<()> {
        let id = self.selected.ok_or_else(|| invalid("Select a profile."))?;
        if filename.trim().is_empty() {
            return Err(invalid("Enter an export path."));
        }
        let file = PathBuf::from(filename);
        let open = self.store.connector();
        let filter = self.filter.clone();
        let (tx, rx) = sync_channel(1);
        std::thread::Builder::new()
            .name("fix-export".into())
            .spawn(move || {
                let _ = tx.send(open().and_then(|store| store.export(id, &filter, &file)));
            })?;
        self.exporting = Some(rx);
        Ok(())
    }
}

fn profile_fields(ui: &mut egui::Ui, draft: &mut ProfileDraft) {
    egui::Grid::new("profile")
        .num_columns(2)
        .spacing([18.0, 8.0])
        .show(ui, |ui| {
            field(ui, "Name", &mut draft.name, 80);
            ui.label("Role");
            egui::ComboBox::from_id_salt("role")
                .selected_text(if draft.role == Role::Acceptor {
                    "Acceptor"
                } else {
                    "Initiator"
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.role, Role::Initiator, "Initiator");
                    ui.selectable_value(&mut draft.role, Role::Acceptor, "Acceptor");
                });
            ui.end_row();
            ui.label("FIX version");
            ui.add_enabled_ui(draft.id.is_none(), |ui| {
                egui::ComboBox::from_id_salt("version")
                    .selected_text(draft.version.begin_string())
                    .show_ui(ui, |ui| {
                        for version in Version::ALL {
                            ui.selectable_value(
                                &mut draft.version,
                                version,
                                version.begin_string(),
                            );
                        }
                    });
            });
            ui.end_row();
            let sender = ui.label("SenderCompID");
            ui.add_enabled(
                draft.id.is_none(),
                egui::TextEdit::singleline(&mut draft.sender)
                    .char_limit(64)
                    .desired_width(210.0),
            )
            .labelled_by(sender.id);
            ui.end_row();
            let target = ui.label("TargetCompID");
            ui.add_enabled(
                draft.id.is_none(),
                egui::TextEdit::singleline(&mut draft.target)
                    .char_limit(64)
                    .desired_width(210.0),
            )
            .labelled_by(target.id);
            ui.end_row();
            field(
                ui,
                if draft.role == Role::Acceptor {
                    "Listen address"
                } else {
                    "Peer address"
                },
                &mut draft.address,
                64,
            );
            field(ui, "Heartbeat (s)", &mut draft.heartbeat, 4);
        });
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, limit: usize) -> egui::Response {
    let label = ui.label(label);
    let response = text(ui, value, limit).labelled_by(label.id);
    ui.end_row();
    response
}

fn text(ui: &mut egui::Ui, value: &mut String, limit: usize) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(210.0)
            .char_limit(limit),
    )
}

fn submit(ui: &mut egui::Ui, label: &str) -> Option<bool> {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui
            .button(RichText::new(label).strong().color(ACCENT))
            .clicked()
        {
            return Some(true);
        }
        if ui.button("Close").clicked() {
            return Some(false);
        }
        None
    })
    .inner
}
