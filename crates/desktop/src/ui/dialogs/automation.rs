use eframe::egui;
use fixtester_desktop::model::{
    OrderKind,
    execution::automation::{REQUESTS, Response, Rule},
};

use super::{MUTED, Workspace, field};

impl Workspace {
    pub(super) fn automation_dialog(&mut self, ui: &mut egui::Ui, id: i64) -> bool {
        ui.heading("Automation");
        let running = self.sessions.is_running(id);
        ui.label(if running {
            "Stop the listener to change the rules or turn automation off."
        } else {
            "Set rules before Listen. Stop turns automation off."
        });
        let settings = self.automation.entry(id).or_default();
        ui.add_enabled_ui(!running, |ui| {
            ui.checkbox(&mut settings.enabled, "Enable automation");
            for ((kind, label), rule) in REQUESTS.into_iter().zip(&mut settings.rules) {
                ui.separator();
                rule_row(ui, kind, label, rule);
            }
        });
        if settings.rules.iter().any(|rule| is_fill(rule.response)) {
            ui.label(egui::RichText::new("Leave the fill price blank to use the limit price. Market orders need an override.").color(MUTED));
        }
        ui.separator();
        ui.label(
            egui::RichText::new(
                "Only single orders on this connection. Use manual actions when a rule cannot run.",
            )
            .color(MUTED),
        );
        !ui.button("Done").clicked()
    }
}

fn rule_row(ui: &mut egui::Ui, kind: OrderKind, label: &str, rule: &mut Rule) {
    ui.horizontal(|ui| {
        let label = ui.strong(label);
        egui::ComboBox::from_id_salt(label.id)
            .selected_text(rule.response.label())
            .show_ui(ui, |ui| {
                for response in Response::ALL {
                    ui.selectable_value(&mut rule.response, response, response.label());
                }
            })
            .response
            .labelled_by(label.id);
    });
    let fills = is_fill(rule.response);
    if fills {
        match kind {
            OrderKind::Replace => ui.label("Accept replace, then fill."),
            OrderKind::Cancel if rule.response == Response::FullFill => {
                ui.label("Fill before cancel, then reject the cancel as too late.")
            }
            OrderKind::Cancel => ui.label("Fill before cancel, then cancel the remainder."),
            OrderKind::New => ui.label("Acknowledge the order, then fill."),
        };
    }
    egui::Grid::new(label).show(ui, |ui| {
        if rule.response == Response::PartialFill {
            field(
                ui,
                &format!("{label} partial quantity"),
                &mut rule.quantity,
                32,
            );
        }
        if fills {
            field(
                ui,
                &format!("{label} fill price override"),
                &mut rule.price,
                32,
            );
        }
        if rule.response == Response::Reject {
            field(
                ui,
                &format!("{label} reject text (optional)"),
                &mut rule.text,
                120,
            );
        }
    });
}

fn is_fill(response: Response) -> bool {
    matches!(response, Response::PartialFill | Response::FullFill)
}
