use eframe::egui;
use fixtester_desktop::model::{ADDITIONAL_TAGS_ERROR_PREFIX, MESSAGE_MAX};

pub(super) fn error(notice: &str) -> Option<&str> {
    notice.strip_prefix(ADDITIONAL_TAGS_ERROR_PREFIX)
}

pub(super) fn show(ui: &mut egui::Ui, value: &mut String, notice: &mut String) {
    let open = (!value.is_empty() || error(notice).is_some()).then_some(true);
    egui::CollapsingHeader::new("Additional FIX tags")
        .open(open)
        .show(ui, |ui| {
            let label = ui.label("Tag=Value pairs separated by semicolons");
            if ui
                .add(
                    egui::TextEdit::singleline(value)
                        .desired_width(290.0)
                        .char_limit(MESSAGE_MAX)
                        .hint_text("9001=test;58=Example"),
                )
                .labelled_by(label.id)
                .changed()
                && error(notice).is_some()
            {
                notice.clear();
            }
            if let Some(error) = error(notice) {
                ui.colored_label(ui.visuals().warn_fg_color, error);
            }
        });
}
