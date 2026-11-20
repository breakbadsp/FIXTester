use eframe::egui;
use fix_dict::Version;
use fixtester_desktop::Result;
use fixtester_desktop::model::{
    TEXT_MAX,
    execution::{Action, Context, Report, Source},
};
use fixtester_desktop::store::Entry;
use fixtester_desktop::transport::Send;

use super::{Dialog, Workspace, additional, field, submit};

pub(in crate::ui) struct ExecutionDraft {
    pub(super) id: i64,
    pub(super) waiting: Option<u64>,
    report: Report,
    additional: String,
}

pub(in crate::ui) fn menu(
    ui: &mut egui::Ui,
    version: Version,
    row: &Entry,
) -> Option<Result<Report>> {
    let context = match Context::read(version, row) {
        Ok(context) => context,
        Err(error) => {
            ui.label(error.to_string());
            return None;
        }
    };
    let source = context.source();
    if source != Source::List {
        return actions(ui, source.actions()).map(|action| Report::draft(&context, None, action));
    }
    ui.label("Select a list order");
    let mut choice = None;
    egui::ScrollArea::vertical()
        .max_height(300.0)
        .show(ui, |ui| {
            for (index, name) in context.children() {
                if let Some(Some(action)) = ui
                    .menu_button(name, |ui| actions(ui, source.actions()))
                    .inner
                {
                    choice = Some(Report::draft(&context, Some(index), action));
                    ui.close();
                }
            }
        });
    choice
}

fn actions(ui: &mut egui::Ui, actions: &[Action]) -> Option<Action> {
    let grouped = actions.iter().any(|action| !action.less_common());
    let mut choice = action_buttons(
        ui,
        actions
            .iter()
            .copied()
            .filter(|action| !grouped || !action.less_common()),
    );
    if grouped
        && actions.iter().any(|action| action.less_common())
        && let Some(Some(action)) = ui
            .menu_button("More execution actions", |ui| {
                action_buttons(
                    ui,
                    actions
                        .iter()
                        .copied()
                        .filter(|action| action.less_common()),
                )
            })
            .inner
    {
        choice = Some(action);
        ui.close();
    }
    choice
}

fn action_buttons(ui: &mut egui::Ui, actions: impl Iterator<Item = Action>) -> Option<Action> {
    let mut choice = None;
    for action in actions {
        if ui.button(action.label()).clicked() {
            choice = Some(action);
            ui.close();
        }
    }
    choice
}

impl Workspace {
    pub(in crate::ui) fn open_execution(&mut self, draft: Option<Result<Report>>) {
        let (Some(id), Some(result)) = (self.selected, draft) else {
            return;
        };
        match result {
            Ok(report) => {
                self.notice.clear();
                self.dialog = Some(Dialog::Execution(ExecutionDraft {
                    id,
                    waiting: None,
                    report,
                    additional: String::new(),
                }));
            }
            Err(error) => self.notice = error.to_string(),
        }
    }

    pub(super) fn execution_dialog(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut ExecutionDraft,
    ) -> bool {
        ui.heading(draft.report.action.label());
        let cancel_reject = matches!(
            draft.report.action,
            Action::RejectCancel | Action::RejectReplace
        );
        if cancel_reject {
            ui.label("OrderCancelReject (35=9)");
        } else {
            ui.label(if draft.report.action == Action::Status {
                "ExecutionReport (35=8) · ExecID: 0"
            } else {
                "ExecutionReport (35=8) · ExecID assigned on Send"
            });
        }
        let height = (ui.ctx().content_rect().height() - 160.0).clamp(100.0, 540.0);
        egui::ScrollArea::vertical()
            .max_height(height)
            .show(ui, |ui| {
                report_fields(ui, &mut draft.report, false);
                if let Some(value) = draft.report.reference_fill_type() {
                    referenced_fill_type(ui, value);
                }
                additional::show(ui, &mut draft.additional, &mut self.notice);
                egui::CollapsingHeader::new("Additional FIX fields").show(ui, |ui| {
                    report_fields(ui, &mut draft.report, true);
                    if !draft.report.carried_groups().is_empty() {
                        ui.label("Instrument groups from the selected message");
                        for (tag, value) in draft.report.carried_groups() {
                            ui.monospace(format!("{tag}={value}"));
                        }
                    }
                });
            });
        match submit(ui, "Send") {
            Some(false) => return false,
            Some(true) => match draft.report.validate().and_then(|()| {
                self.queue(
                    draft.id,
                    Send::Execution(draft.report.clone()),
                    &draft.additional,
                )
            }) {
                Ok(epoch) => {
                    draft.waiting = Some(epoch);
                    self.notice.clear();
                }
                Err(error) => self.notice = error.to_string(),
            },
            None => {}
        }
        true
    }
}

fn referenced_fill_type(ui: &mut egui::Ui, value: &mut String) {
    ui.horizontal(|ui| {
        let label = ui.label("Referenced fill type");
        egui::ComboBox::from_id_salt("reference-fill-type")
            .selected_text(match value.as_str() {
                "1" => "Partial fill",
                "2" => "Full fill",
                _ => "Select a fill type",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(value, "1".into(), "Partial fill");
                ui.selectable_value(value, "2".into(), "Full fill");
            })
            .response
            .labelled_by(label.id);
    });
}

fn report_fields(ui: &mut egui::Ui, report: &mut Report, detail: bool) {
    let dict = report.version().dictionary();
    egui::Grid::new(("execution-fields", detail))
        .num_columns(2)
        .spacing([18.0, 6.0])
        .show(ui, |ui| {
            for item in report
                .fields_mut()
                .iter_mut()
                .filter(|item| item.detail == detail)
            {
                let label = format!("{} ({})", item.label, item.tag);
                if matches!(item.tag, 39 | 54 | 102 | 103 | 378)
                    && let Some(def) = dict.field(item.tag)
                    && !def.values.is_empty()
                {
                    let name = def.values.iter().find(|entry| entry.value == item.value);
                    let caption = name.map_or("Select a value".to_owned(), |entry| {
                        format!("{} · {}", entry.value, entry.name)
                    });
                    let label = ui.label(label);
                    egui::ComboBox::from_id_salt(item.tag)
                        .selected_text(caption)
                        .show_ui(ui, |ui| {
                            if !item.required {
                                ui.selectable_value(&mut item.value, String::new(), "None");
                            }
                            for entry in def.values {
                                ui.selectable_value(
                                    &mut item.value,
                                    entry.value.to_owned(),
                                    format!("{} · {}", entry.value, entry.name),
                                );
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    ui.end_row();
                } else {
                    field(ui, &label, &mut item.value, TEXT_MAX);
                }
            }
        });
}
