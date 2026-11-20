use std::time::Instant;

use eframe::egui::{self, RichText, TextStyle};
use fix_codec::{Decoder, write_utc_timestamp};
use fix_dict::Version;
use fixtester_desktop::model::{FIELDS_MAX, OrderKind, execution::Source};
use fixtester_desktop::store::{Direction, Entry, PAGE_ROWS, escape};

use super::{
    ACCENT, MUTED, SURFACE, Workspace,
    dialogs::{Dialog, execution},
};
use fixtester_desktop::store::SavedProfile;

impl Workspace {
    pub(super) fn timeline(&mut self, ui: &mut egui::Ui, saved: &SavedProfile) {
        self.timeline_toolbar(ui);
        ui.separator();
        let selected = self
            .selected_row
            .and_then(|id| self.entries.iter().find(|row| row.id == id))
            .cloned();
        egui::Panel::bottom("inspector")
            .resizable(true)
            .default_size(220.0)
            .min_size(110.0)
            .show(ui, |ui| {
                if let Some(entry) = selected.as_ref() {
                    self.inspector(ui, saved.profile.version, entry);
                } else {
                    ui.strong("Message details");
                    ui.label(
                        RichText::new("Select a message to inspect its fields or raw bytes.")
                            .color(MUTED),
                    );
                }
            });
        self.rows(ui, saved.profile.version);
    }

    fn timeline_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Timeline");
            let search = ui
                .add(
                    egui::TextEdit::singleline(&mut self.filter.text)
                        .char_limit(256)
                        .hint_text("Search tags, values, or status")
                        .desired_width(230.0),
                )
                .on_hover_text("Ctrl+F");
            if std::mem::take(&mut self.search_focus) {
                search.request_focus();
            }
            if search.changed() {
                self.search_changed = Some(Instant::now());
                self.reset_page();
            }
            let previous = self.filter.direction;
            egui::ComboBox::from_id_salt("direction")
                .selected_text(direction_name(previous))
                .show_ui(ui, |ui| {
                    for value in [None, Some(Direction::Sent), Some(Direction::Received)] {
                        ui.selectable_value(
                            &mut self.filter.direction,
                            value,
                            direction_name(value),
                        );
                    }
                });
            let changed = ui
                .checkbox(&mut self.filter.application_only, "Application only")
                .changed();
            if changed || previous != self.filter.direction {
                self.reset_page();
            }
            if ui
                .add_enabled(self.exporting.is_none(), egui::Button::new("Export…"))
                .clicked()
            {
                self.notice.clear();
                self.dialog = Some(Dialog::Export(String::new()));
            }
        });
    }

    fn rows(&mut self, ui: &mut egui::Ui, version: Version) {
        self.timeline_pagination(ui);
        ui.label(
            RichText::new(format!(
                "{:<21} {:<3} {:>6}  {:<26} {}",
                "UTC", "I/O", "SEQ", "MESSAGE", "SUMMARY"
            ))
            .monospace()
            .color(MUTED),
        );
        if self.entries.is_empty() {
            ui.label(RichText::new("No matching messages.").color(MUTED));
            return;
        }
        let active = self.active();
        let mut execution_draft = None;
        egui::ScrollArea::vertical()
            .id_salt(("messages", self.selected, self.pages.len()))
            .max_height(ui.available_height())
            .show_rows(ui, 25.0, self.entries.len(), |ui, range| {
                for index in range {
                    let row = &self.entries[index];
                    let color = if row.direction == Direction::Received {
                        ACCENT
                    } else {
                        ui.visuals().text_color()
                    };
                    let label = row_label(row, version);
                    let button = egui::Button::selectable(
                        self.selected_row == Some(row.id),
                        RichText::new(label).monospace().color(color),
                    )
                    .truncate()
                    .min_size(egui::vec2(ui.available_width(), 25.0));
                    let row_scope =
                        egui::UiBuilder::new().scope_id(egui::Id::unique(("timeline-row", row.id)));
                    let response = ui
                        .scope_builder(row_scope, |ui| ui.add(button))
                        .inner
                        .on_hover_text(format!("{} · {}", row.status, row.summary));
                    let can_report = active && Source::identify(version, row).is_some();
                    if response.clicked() || (can_report && response.secondary_clicked()) {
                        self.selected_row = Some(row.id);
                    }
                    if can_report {
                        response.context_menu(|ui| {
                            if let Some(draft) = execution::menu(ui, version, row) {
                                execution_draft = Some(draft);
                            }
                        });
                    }
                }
            });
        self.open_execution(execution_draft);
    }

    fn timeline_pagination(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.pages.is_empty(), egui::Button::new("Newer"))
                .clicked()
            {
                self.pages.pop();
                self.refresh = true;
                self.selected_row = None;
            }
            if ui
                .add_enabled(
                    self.entries.len() == PAGE_ROWS as usize,
                    egui::Button::new("Older"),
                )
                .clicked()
            {
                self.pages.push(self.entries.last().map(|row| row.id));
                self.refresh = true;
                self.selected_row = None;
            }
            ui.label(
                RichText::new(format!(
                    "Page {}  ·  {} messages",
                    self.pages.len() + 1,
                    self.entries.len()
                ))
                .color(MUTED),
            );
        });
    }

    fn inspector(&mut self, ui: &mut egui::Ui, version: Version, row: &Entry) {
        let mut raw_view =
            ui.data_mut(|data| *data.get_temp_mut_or_default::<bool>(egui::Id::unique("show-raw")));
        ui.horizontal_wrapped(|ui| {
            ui.strong(message_name(version, &row.kind));
            ui.label(
                RichText::new(format!("{} · {} bytes", row.status, row.transferred)).color(MUTED),
            );
            ui.separator();
            ui.selectable_value(&mut raw_view, false, "Fields");
            ui.selectable_value(&mut raw_view, true, "Raw");
            if ui
                .button("Copy raw")
                .on_hover_text("Copy bytes as printable ASCII and \\xNN escapes.")
                .clicked()
            {
                ui.ctx().copy_text(escape(&row.raw));
            }
            if self.active() && Source::identify(version, row).is_some() {
                let scope = egui::UiBuilder::new()
                    .scope_id(egui::Id::unique(("inspector-execution", row.id)));
                let draft = ui
                    .scope_builder(scope, |ui| {
                        ui.menu_button("Execution actions", |ui| execution::menu(ui, version, row))
                            .inner
                            .flatten()
                    })
                    .inner;
                self.open_execution(draft);
            }
            if self.active()
                && row.direction == Direction::Sent
                && row.status == "Sent"
                && matches!(row.kind.as_str(), "D" | "G")
            {
                if ui.button("Use for replace").clicked() {
                    self.open_order(OrderKind::Replace, true);
                }
                if ui.button("Use for cancel").clicked() {
                    self.open_order(OrderKind::Cancel, true);
                }
            }
        });
        ui.data_mut(|data| data.insert_temp(egui::Id::unique("show-raw"), raw_view));
        ui.separator();
        egui::ScrollArea::both()
            .id_salt(("detail", row.id, raw_view))
            .show(ui, |ui| {
                if raw_view {
                    raw_bytes(ui, row);
                } else {
                    decoded(ui, version, row);
                }
            });
    }
}

fn direction_name(direction: Option<Direction>) -> &'static str {
    match direction {
        None => "Both directions",
        Some(Direction::Sent) => "Sent",
        Some(Direction::Received) => "Received",
    }
}

fn row_label(row: &Entry, version: Version) -> String {
    let mut time = [0; 21];
    let time = std::str::from_utf8(write_utc_timestamp(row.time, &mut time)).unwrap_or_default();
    let side = if row.direction == Direction::Received {
        "RX"
    } else {
        "TX"
    };
    let message = message_name(version, &row.kind);
    let suffix = if matches!(row.status.as_str(), "Received" | "Sent") {
        row.summary.as_str()
    } else {
        row.status.as_str()
    };
    format!(
        "{time:<21} {side:<3} {:>6}  {message:<26.26} {suffix}",
        row.seq
    )
}

fn message_name(version: Version, kind: &str) -> &str {
    if kind.is_empty() {
        return "Unframed data";
    }
    version
        .dictionary()
        .message(kind)
        .map_or(kind, |message| message.name)
}

fn raw_bytes(ui: &mut egui::Ui, row: &Entry) {
    let raw = escape(&row.raw);
    if row.direction == Direction::Sent && row.status != "Sent" {
        ui.label("The raw view shows the planned frame. Bytes recorded as written are shown in the byte count.");
    }
    ui.label(RichText::new("SOH is \\x01. Export keeps the original byte values.").color(MUTED));
    ui.add(
        egui::TextEdit::multiline(&mut raw.as_str())
            .font(TextStyle::Monospace)
            .desired_width(f32::INFINITY),
    );
}

fn decoded(ui: &mut egui::Ui, version: Version, row: &Entry) {
    let mut decoder = Decoder::new(FIELDS_MAX);
    let Ok(message) = decoder.decode(&row.raw) else {
        ui.label("These bytes do not decode as a complete FIX message. Open Raw to inspect them.");
        return;
    };
    let fields = message.fields();
    let dict = version.dictionary();
    if row.kind == "8" {
        ui.horizontal_wrapped(|ui| {
            for (tag, label) in [
                (150, "Execution"),
                (39, "Order status"),
                (14, "CumQty"),
                (151, "LeavesQty"),
                (32, "LastQty"),
                (31, "LastPx"),
            ] {
                if let Some(value) = fields.get(tag) {
                    let description = field_value(version, tag, value);
                    ui.label(
                        RichText::new(format!("{label}  {description}"))
                            .color(ACCENT)
                            .background_color(SURFACE),
                    );
                }
            }
        });
    }
    for tag in [58, 102, 103, 373, 380] {
        if let Some(value) = fields.get(tag) {
            let name = dict.field(tag).map_or("Text", |def| def.name);
            ui.label(
                RichText::new(format!("{name}  {}", field_value(version, tag, value)))
                    .color(ui.visuals().warn_fg_color),
            );
        }
    }
    egui::Grid::new("fields")
        .num_columns(3)
        .striped(true)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            ui.strong("Tag");
            ui.strong("Field");
            ui.strong("Value");
            ui.end_row();
            for (tag, value) in fields.iter() {
                ui.monospace(tag.to_string());
                ui.label(dict.field(tag).map_or("Custom", |def| def.name));
                ui.monospace(field_value(version, tag, value));
                ui.end_row();
            }
        });
}

fn field_value(version: Version, tag: u32, value: &[u8]) -> String {
    let raw = escape(value);
    let name = version.dictionary().field(tag).and_then(|def| {
        def.values
            .iter()
            .find(|item| item.value.as_bytes() == value)
    });
    name.map_or_else(|| raw.clone(), |item| format!("{raw}  {}", item.name))
}
