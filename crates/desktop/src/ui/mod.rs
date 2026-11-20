mod dialogs;
mod timeline;

use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::Receiver,
};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Key, Modifiers, RichText, TextStyle};
use fix_dict::Version;
use fix_session::{Role, State};
use fixtester_desktop::Result;
use fixtester_desktop::model::{OrderKind, SESSIONS_MAX};
use fixtester_desktop::store::{Entry, Filter, SavedProfile, Store};
use fixtester_desktop::transport::{Phase, Sessions, Status};

use dialogs::{Dialog, ProfileDraft};

const ACCENT: Color32 = Color32::from_rgb(99, 222, 193);
const MUTED: Color32 = Color32::from_rgb(156, 173, 195);
const SURFACE: Color32 = Color32::from_rgb(21, 30, 44);
const BACKGROUND: Color32 = Color32::from_rgb(13, 21, 34);

pub fn style(ctx: &egui::Context) {
    let mut theme = egui::Style {
        visuals: egui::Visuals::dark(),
        ..Default::default()
    };
    theme.visuals.panel_fill = BACKGROUND;
    theme.visuals.window_fill = SURFACE;
    theme.visuals.extreme_bg_color = Color32::from_rgb(10, 17, 28);
    theme.visuals.faint_bg_color = SURFACE;
    theme.visuals.widgets.inactive.bg_fill = Color32::from_rgb(30, 44, 63);
    theme.visuals.widgets.hovered.bg_fill = Color32::from_rgb(43, 64, 82);
    theme.visuals.selection.bg_fill = Color32::from_rgb(26, 73, 74);
    theme.visuals.selection.stroke.color = ACCENT;
    theme.spacing.item_spacing = egui::vec2(8.0, 8.0);
    theme.spacing.interact_size.y = 28.0;
    theme
        .text_styles
        .get_mut(&TextStyle::Body)
        .iter_mut()
        .for_each(|font| font.size = 14.0);
    ctx.set_style_of(egui::Theme::Dark, theme);
    ctx.set_theme(egui::Theme::Dark);
}

pub struct Workspace {
    store: Store,
    sessions: Sessions,
    profiles: Vec<SavedProfile>,
    selected: Option<i64>,
    entries: Vec<Entry>,
    selected_row: Option<i64>,
    pages: Vec<Option<i64>>,
    filter: Filter,
    last_revision: Option<u64>,
    refresh: bool,
    search_changed: Option<Instant>,
    search_focus: bool,
    dialog: Option<Dialog>,
    notice: String,
    next_purge: Instant,
    smoke: Option<Arc<AtomicBool>>,
    exporting: Option<Receiver<Result<usize>>>,
    close_started: Option<Instant>,
}

impl Workspace {
    pub fn open(path: &Path, smoke: Option<Arc<AtomicBool>>) -> Result<Self> {
        let store = Store::open(path)?;
        let purged = store.purge()?;
        let profiles = store.profiles()?;
        let selected = profiles.first().map(|profile| profile.id);
        Ok(Self {
            store,
            sessions: Sessions::default(),
            profiles,
            selected,
            entries: Vec::new(),
            selected_row: None,
            pages: Vec::new(),
            filter: Filter::default(),
            last_revision: None,
            refresh: true,
            search_changed: None,
            search_focus: false,
            dialog: None,
            notice: String::new(),
            next_purge: Instant::now() + purge_interval(purged),
            smoke,
            exporting: None,
            close_started: None,
        })
    }

    fn select(&mut self, id: i64) {
        if self.selected != Some(id) {
            self.selected = Some(id);
            self.notice.clear();
            self.reset_page();
        }
    }

    fn reset_page(&mut self) {
        self.pages.clear();
        self.selected_row = None;
        self.last_revision = None;
        self.refresh = true;
    }

    fn selected(&self) -> Option<&SavedProfile> {
        self.profiles
            .iter()
            .find(|saved| Some(saved.id) == self.selected)
    }

    fn status(&self) -> Option<Status> {
        let id = self.selected?;
        self.sessions.status(id).or_else(|| {
            let checkpoint = self.store.checkpoint(id).ok()?;
            Some(Status {
                phase: Phase::Stopped,
                seq: checkpoint.seq,
                notice: if checkpoint.reset {
                    "Sequence reset armed.".into()
                } else {
                    String::new()
                },
                ..Status::default()
            })
        })
    }

    fn active(&self) -> bool {
        self.selected
            .and_then(|id| self.sessions.status(id))
            .is_some_and(|status| status.phase == Phase::Link(State::Active))
    }

    fn show_result(&mut self, result: Result<()>) {
        self.notice = result
            .err()
            .map_or_else(String::new, |error| error.to_string());
    }

    fn start_stop(&mut self) {
        let Some(saved) = self.selected().cloned() else {
            return;
        };
        if self.sessions.is_running(saved.id) {
            self.sessions.stop(saved.id);
            self.notice = "Stopping session…".into();
        } else {
            let result = self.sessions.start(&self.store, saved);
            self.show_result(result);
            self.refresh = true;
        }
    }

    fn poll(&mut self) {
        if Instant::now() >= self.next_purge {
            let done = match self.store.purge() {
                Ok(done) => {
                    self.refresh = true;
                    done
                }
                Err(error) => {
                    self.notice = error.to_string();
                    true
                }
            };
            self.next_purge = Instant::now() + purge_interval(done);
        }
        if let Some(rx) = &self.exporting {
            match rx.try_recv() {
                Ok(result) => {
                    self.notice = result.map_or_else(
                        |error| error.to_string(),
                        |count| format!("Exported {count} records."),
                    );
                    self.exporting = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.notice = "Export stopped.".into();
                    self.exporting = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }

        let Some(id) = self.selected else { return };
        if let Some(changed) = self.search_changed {
            if changed.elapsed() < Duration::from_millis(180) {
                return;
            }
            self.search_changed = None;
        }
        let revision = self.sessions.status(id).map(|status| status.revision);
        if self.refresh || (self.pages.is_empty() && revision != self.last_revision) {
            self.last_revision = revision;
            self.refresh = false;
            match self
                .store
                .history(id, &self.filter, self.pages.last().copied().flatten())
            {
                Ok(rows) => self.entries = rows,
                Err(error) => self.notice = error.to_string(),
            }
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("FIXTester").size(25.0).strong());
        ui.label(RichText::new("FIX session workspace").color(MUTED));
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.strong("Sessions");
            ui.label(
                RichText::new(format!("{} / {SESSIONS_MAX}", self.sessions.count()))
                    .color(MUTED)
                    .monospace(),
            );
        });
        ui.add_space(5.0);
        if ui.button("+  Add profile").clicked() {
            self.dialog = Some(Dialog::Profile(ProfileDraft::default()));
        }
        ui.add_space(6.0);
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("profiles")
            .show(ui, |ui| {
                let mut next = None;
                for saved in &self.profiles {
                    let phase = self
                        .sessions
                        .status(saved.id)
                        .map_or(Phase::Stopped, |status| status.phase);
                    let color = if phase == Phase::Link(State::Active) {
                        ACCENT
                    } else {
                        MUTED
                    };
                    let role = if saved.profile.role == Role::Acceptor {
                        "Listen"
                    } else {
                        "Connect"
                    };
                    let label = format!(
                        "{}\n{} · {role} · {}",
                        saved.profile.name,
                        saved.profile.version.begin_string(),
                        phase.label()
                    );
                    let button = egui::Button::selectable(
                        self.selected == Some(saved.id),
                        RichText::new(label).color(color),
                    )
                    .min_size(egui::vec2(ui.available_width(), 48.0));
                    if ui.add(button).clicked() {
                        next = Some(saved.id);
                    }
                }
                if let Some(id) = next {
                    self.select(id);
                }
            });
    }

    fn session_bar(&mut self, ui: &mut egui::Ui, saved: &SavedProfile) {
        let status = self.status().unwrap_or_default();
        let running = self.sessions.is_running(saved.id);
        ui.horizontal_wrapped(|ui| {
            ui.heading(&saved.profile.name);
            let color = if self.active() { ACCENT } else { MUTED };
            ui.label(RichText::new(status.phase.label()).color(color).strong());
            ui.separator();
            ui.monospace(format!(
                "Next TX {:>3}   Next RX {:>3}",
                status.seq.next_sender, status.seq.next_target
            ));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "{}  ·  {} → {}  ·  {}",
                    saved.profile.version.begin_string(),
                    saved.profile.sender,
                    saved.profile.target,
                    saved.profile.address
                ))
                .color(MUTED),
            );
            if ui
                .button(if running {
                    "Stop"
                } else if saved.profile.role == Role::Acceptor {
                    "Listen"
                } else {
                    "Connect"
                })
                .on_hover_text("F5")
                .clicked()
            {
                self.start_stop();
            }
            if ui
                .add_enabled(!running, egui::Button::new("Edit profile"))
                .clicked()
            {
                self.dialog = Some(Dialog::Profile(ProfileDraft::from(saved)));
            }
            if ui
                .add_enabled(!running, egui::Button::new("Reset sequences…"))
                .clicked()
            {
                self.dialog = Some(Dialog::Reset(saved.id));
            }
        });
        let notice = if self.notice.is_empty() {
            &status.notice
        } else {
            &self.notice
        };
        if !notice.is_empty() {
            ui.label(RichText::new(notice).color(MUTED));
        }
        ui.separator();
        self.actions(ui);
    }

    fn actions(&mut self, ui: &mut egui::Ui) {
        let active = self.active();
        ui.horizontal_wrapped(|ui| {
            for (kind, name, key) in [
                (OrderKind::New, "New limit", "Ctrl+N"),
                (OrderKind::Replace, "Replace", "Ctrl+R"),
                (OrderKind::Cancel, "Cancel order", "Ctrl+K"),
            ] {
                let label = if kind == OrderKind::New {
                    RichText::new(name).strong().color(ACCENT)
                } else {
                    RichText::new(name)
                };
                if ui
                    .add_enabled(active, egui::Button::new(label))
                    .on_hover_text(key)
                    .clicked()
                {
                    self.open_order(kind, true);
                }
            }
            let multileg = self
                .selected()
                .is_some_and(|saved| saved.profile.version != Version::Fix42);
            if ui
                .add_enabled(active && multileg, egui::Button::new("Multileg order"))
                .on_disabled_hover_text("Multileg orders require FIX 4.3 or 4.4.")
                .clicked()
            {
                self.open_grouped(true);
            }
            if ui
                .add_enabled(active, egui::Button::new("Order list"))
                .clicked()
            {
                self.open_grouped(false);
            }
            if ui
                .add_enabled(active, egui::Button::new("Tag editor"))
                .on_hover_text("Ctrl+T")
                .clicked()
            {
                self.open_tags();
            }
        });
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        let command = |key| ctx.input_mut(|input| input.consume_key(Modifiers::CTRL, key));
        if command(Key::F) {
            self.search_focus = true;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::F5)) {
            self.start_stop();
        }
        if self.active() {
            for (key, kind) in [
                (Key::N, OrderKind::New),
                (Key::R, OrderKind::Replace),
                (Key::K, OrderKind::Cancel),
            ] {
                if command(key) {
                    self.open_order(kind, true);
                    return;
                }
            }
            if command(Key::T) {
                self.open_tags();
            }
        }
    }

    fn smoke_frame(&mut self, ctx: &egui::Context) {
        if let Some(report) = self.smoke.take() {
            let ctx = ctx.clone();
            ctx.clone().request_screenshot(move |image| {
                let has_content = image.pixels.first().is_some_and(|first| {
                    image
                        .pixels
                        .iter()
                        .filter(|pixel| *pixel != first)
                        .take(1024)
                        .count()
                        == 1024
                });
                report.store(has_content, Ordering::Relaxed);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            });
        }
    }

    fn close(&mut self, ctx: &egui::Context) {
        if self.sessions.count() == 0 {
            return;
        }
        if ctx.input(|input| input.viewport().close_requested()) && self.close_started.is_none() {
            self.close_started = Some(Instant::now());
            for profile in &self.profiles {
                self.sessions.stop(profile.id);
            }
        }
        if self
            .close_started
            .is_some_and(|start| start.elapsed() < Duration::from_secs(4))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let was_closing = self.close_started.is_some();
        self.close(&ctx);
        if was_closing
            && (self.sessions.count() == 0
                || self
                    .close_started
                    .is_some_and(|time| time.elapsed() >= Duration::from_secs(4)))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.poll();
        self.keyboard(&ctx);
        egui::Panel::left("sessions")
            .exact_size(205.0)
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(saved) = self.selected().cloned() {
                self.session_bar(ui, &saved);
                self.timeline(ui, &saved);
            } else {
                ui.add_space(32.0);
                ui.heading("Set up a FIX session");
                ui.label("Add a profile with the FIX version, CompIDs, and TCP address.");
                if ui.button("Add profile").clicked() {
                    self.dialog = Some(Dialog::Profile(ProfileDraft::default()));
                }
            }
        });
        self.show_dialog(&ctx);
        self.smoke_frame(&ctx);
        ctx.request_repaint_after(Duration::from_millis(125));
    }
}

impl eframe::App for Workspace {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

#[cfg(test)]
mod verification;

fn purge_interval(done: bool) -> Duration {
    if done {
        Duration::from_secs(30)
    } else {
        Duration::from_millis(125)
    }
}
