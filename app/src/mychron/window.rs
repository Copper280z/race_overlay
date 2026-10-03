//! The MyChron window: a device card, the logger's sessions to pick from,
//! and Track mode settings.

use super::MyChron;
use super::keep_awake::KeepAwake;
use super::settings::{AfterDownload, Connection, Since};
use super::worker::{Activity, Command, Presence, Row};
use crate::ui_kit::{
    Tone,
    theme::{self, surface, text},
    widgets,
};
use eframe::egui::{self, Align, Layout, RichText, Ui};
use overlay_logger::wifi::Permission;
use std::{collections::BTreeSet, path::Path};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Sessions,
    TrackMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    New,
    All,
    Downloaded,
}

#[derive(Default)]
pub struct View {
    pub tab: Tab,
    pub filter: Filter,
    pub search: String,
    /// Logger file names picked for download.
    pub selected: BTreeSet<String>,
}

impl View {
    pub fn retain_selection(&mut self, rows: &[Row]) {
        self.selected
            .retain(|name| rows.iter().any(|row| row.log.name == *name));
    }

    pub fn deselect(&mut self, name: &str) {
        self.selected.remove(name);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    New,
    Downloaded,
    Imported,
}

pub fn row_state(row: &Row, in_workspace: &dyn Fn(&Path) -> bool) -> RowState {
    match &row.downloaded {
        None => RowState::New,
        Some(path) if in_workspace(path) => RowState::Imported,
        Some(_) => RowState::Downloaded,
    }
}

fn shows(filter: Filter, state: RowState) -> bool {
    match filter {
        Filter::New => state == RowState::New,
        Filter::All => true,
        Filter::Downloaded => state != RowState::New,
    }
}

fn matches_search(row: &Row, search: &str) -> bool {
    let search = search.trim().to_lowercase();
    search.is_empty()
        || [
            &row.log.track,
            &row.log.driver,
            &row.log.vehicle,
            &row.log.name,
        ]
        .iter()
        .any(|field| field.to_lowercase().contains(&search))
}

fn megabytes(bytes: u64) -> String {
    if bytes < 1_000_000 {
        format!("{} KB", bytes.div_ceil(1_000))
    } else {
        format!("{:.1} MB", bytes as f64 / 1e6)
    }
}

fn lap_time(ms: u32) -> String {
    widgets::clock(f64::from(ms) / 1000.0)
}

fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Fixed-width column so rows line up.
fn cell(ui: &mut Ui, width: f32, text: RichText) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_width(width);
            ui.add(egui::Label::new(text).truncate());
        },
    );
}

/// A painted check box (the row itself takes the click).
fn check_mark(ui: &mut Ui, checked: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let painter = ui.painter();
    if checked {
        painter.rect_filled(rect, 4.0, theme::accent());
        let stroke = egui::Stroke::new(2.0, theme::on_accent());
        let points = [
            rect.left_top() + egui::vec2(4.0, 8.5),
            rect.left_top() + egui::vec2(7.0, 11.5),
            rect.left_top() + egui::vec2(12.0, 5.0),
        ];
        painter.line_segment([points[0], points[1]], stroke);
        painter.line_segment([points[1], points[2]], stroke);
    } else {
        painter.rect_stroke(
            rect.shrink(0.5),
            4.0,
            egui::Stroke::new(1.0, surface::control_border()),
            egui::StrokeKind::Inside,
        );
    }
}

fn reveal(path: &Path) {
    let _ = std::fs::create_dir_all(path);
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(path).spawn();
}

impl MyChron {
    /// Draws the window when open. `in_workspace` tells whether a file is
    /// already a source in the Analysis workspace.
    pub fn show(&mut self, ctx: &egui::Context, in_workspace: &dyn Fn(&Path) -> bool) {
        if !self.open {
            return;
        }
        if self.wifi.is_none() {
            self.wifi = (self.wifi_factory)();
            self.interfaces = self
                .wifi
                .as_ref()
                .map(|wifi| wifi.interfaces())
                .unwrap_or_default();
        }
        let mut open = true;
        egui::Window::new("MyChron")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .pivot(egui::Align2::CENTER_TOP)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(0.0, 64.0))
            .default_size([720.0, 600.0])
            .min_width(560.0)
            .min_height(360.0)
            .show(ctx, |ui| {
                if let Err(error) = &self.addr {
                    widgets::callout(ui, Tone::Bad, format!("Logger address: {error}"));
                    return;
                }
                self.device_card(ui);
                ui.add_space(8.0);
                widgets::segmented(
                    ui,
                    &mut self.view.tab,
                    &[
                        (Tab::Sessions, "Sessions", ""),
                        (Tab::TrackMode, "Track mode", ""),
                    ],
                );
                ui.add_space(6.0);
                match self.view.tab {
                    Tab::Sessions => self.sessions(ui, in_workspace),
                    Tab::TrackMode => self.track_mode(ui),
                }
            });
        self.open = open;
    }

    fn device_card(&mut self, ui: &mut Ui) {
        let presence = self.presence.clone();
        widgets::card(ui, false, |ui| {
            ui.horizontal(|ui| {
                let (tone, subtitle) = match &presence {
                    Presence::Connected { .. } => {
                        let mut parts = vec![format!("Connected · {} sessions", self.rows.len())];
                        if let Some(device) = &self.device {
                            parts.extend(device.driver.clone());
                            parts.extend(device.vehicle.clone());
                        }
                        (Tone::Good, parts.join(" · "))
                    }
                    Presence::Visible { .. } => (Tone::Info, "In range".to_owned()),
                    Presence::Absent if !self.rows.is_empty() => {
                        (Tone::Warn, "Out of range".to_owned())
                    }
                    Presence::Absent => (Tone::Neutral, "Searching…".to_owned()),
                };
                widgets::dot(ui, tone.color());
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(self.logger_name().unwrap_or_else(|| "No logger".into()))
                            .strong()
                            .color(text::strong()),
                    );
                    ui.label(RichText::new(subtitle).small().color(text::weak()));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let idle = self.activity == Activity::Idle;
                    match &presence {
                        Presence::Connected { ssid, .. } => {
                            let joined = ssid.is_some()
                                && self.settings.connection == Connection::AppSwitches;
                            widgets::action_menu(ui, "More ⏷", |ui| {
                                if widgets::menu_item(
                                    ui,
                                    "Set logger clock",
                                    Some("From this computer"),
                                ) {
                                    self.send(Command::SetClock);
                                    ui.close();
                                }
                                if widgets::menu_item(ui, "Open library folder", None) {
                                    reveal(&self.settings.library_root());
                                    ui.close();
                                }
                                if joined && widgets::menu_item(ui, "Leave Wi-Fi", None) {
                                    self.send(Command::Disconnect);
                                    ui.close();
                                }
                            });
                            if ui.add_enabled(idle, egui::Button::new("Refresh")).clicked() {
                                self.send(Command::Refresh);
                            }
                        }
                        Presence::Visible { .. } => {
                            if idle && widgets::primary_button(ui, "Join").clicked() {
                                self.send(Command::Connect);
                            }
                        }
                        Presence::Absent => {
                            if ui.add_enabled(idle, egui::Button::new("Retry")).clicked() {
                                self.send(Command::Refresh);
                            }
                        }
                    }
                    if !idle {
                        ui.spinner();
                    }
                });
            });
        });
    }

    fn sessions(&mut self, ui: &mut Ui, in_workspace: &dyn Fn(&Path) -> bool) {
        if self.rows.is_empty() {
            self.no_sessions(ui);
            return;
        }
        let states = self
            .rows
            .iter()
            .map(|row| row_state(row, in_workspace))
            .collect::<Vec<_>>();
        let count = |filter| states.iter().filter(|s| shows(filter, **s)).count();
        let labels = [
            format!("New {}", count(Filter::New)),
            format!("All {}", count(Filter::All)),
            format!("Downloaded {}", count(Filter::Downloaded)),
        ];
        ui.horizontal(|ui| {
            widgets::segmented(
                ui,
                &mut self.view.filter,
                &[
                    (Filter::New, &labels[0], ""),
                    (Filter::All, &labels[1], ""),
                    (Filter::Downloaded, &labels[2], ""),
                ],
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.view.search)
                        .hint_text("Track or driver")
                        .desired_width(160.0),
                );
            });
        });
        ui.add_space(4.0);

        let visible = self
            .rows
            .iter()
            .zip(&states)
            .filter(|(row, state)| {
                shows(self.view.filter, **state) && matches_search(row, &self.view.search)
            })
            .collect::<Vec<_>>();
        let mut toggled = None;
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 44.0).max(80.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if visible.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| ui.weak("Nothing here"));
                }
                let mut group = None;
                for (row, state) in &visible {
                    let heading = format!(
                        "{} · {}",
                        row.log.recorded.map_or_else(
                            || "Undated".into(),
                            |t| t.format("%a %-d %b %Y").to_string()
                        ),
                        if row.log.track.is_empty() {
                            "No track"
                        } else {
                            &row.log.track
                        }
                    );
                    if group.as_ref() != Some(&heading) {
                        widgets::section_label(ui, &heading);
                        group = Some(heading);
                    }
                    let selected = self.view.selected.contains(&row.log.name);
                    let response = widgets::list_row(ui, selected, |ui| {
                        ui.horizontal(|ui| {
                            check_mark(ui, selected);
                            let log = &row.log;
                            cell(
                                ui,
                                48.0,
                                RichText::new(
                                    log.recorded.map_or_else(String::new, |t| {
                                        t.format("%H:%M").to_string()
                                    }),
                                )
                                .strong(),
                            );
                            cell(
                                ui,
                                56.0,
                                RichText::new(log.laps.map_or_else(String::new, |n| {
                                    format!("{n} lap{}", if n == 1 { "" } else { "s" })
                                })),
                            );
                            cell(
                                ui,
                                92.0,
                                RichText::new(log.best_lap_ms.map_or_else(String::new, |ms| {
                                    format!("best {}", lap_time(ms))
                                })),
                            );
                            cell(
                                ui,
                                44.0,
                                RichText::new(log.duration_ms.map_or_else(String::new, duration))
                                    .color(text::weak()),
                            );
                            cell(
                                ui,
                                64.0,
                                RichText::new(megabytes(log.size)).color(text::weak()),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let (label, tone) = match state {
                                    RowState::New => ("New", Tone::Info),
                                    RowState::Downloaded => ("Downloaded", Tone::Neutral),
                                    RowState::Imported => ("Imported", Tone::Good),
                                };
                                widgets::chip(ui, label, tone);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&log.name).small().color(text::weak()),
                                    )
                                    .truncate(),
                                );
                            });
                        });
                    });
                    if response.response.clicked() {
                        toggled = Some(row.log.name.clone());
                    }
                }
            });
        if let Some(name) = toggled
            && !self.view.selected.remove(&name)
        {
            self.view.selected.insert(name);
        }
        ui.separator();
        self.sessions_footer(ui, &states);
    }

    fn sessions_footer(&mut self, ui: &mut Ui, states: &[RowState]) {
        if let Activity::Downloading {
            name,
            index,
            count,
            done,
            total,
        } = &self.activity
        {
            let (name, index, count) = (name.clone(), *index, *count);
            let fraction = if *total > 0 {
                *done as f32 / *total as f32
            } else {
                0.0
            };
            ui.horizontal(|ui| {
                ui.label(format!("{name} · {} of {count}", index + 1));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        self.cancel();
                    }
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_width(ui.available_width().min(320.0))
                            .show_percentage(),
                    );
                });
            });
            return;
        }
        let (names, bytes) = self
            .rows
            .iter()
            .filter(|row| self.view.selected.contains(&row.log.name))
            .fold((Vec::new(), 0u64), |(mut names, bytes), row| {
                names.push(row.log.name.clone());
                (names, bytes + row.log.size)
            });
        let ready = !names.is_empty()
            && self.activity == Activity::Idle
            && matches!(self.presence, Presence::Connected { .. });
        ui.horizontal(|ui| {
            let new = self
                .rows
                .iter()
                .zip(states)
                .filter(|(_, state)| **state == RowState::New)
                .map(|(row, _)| row.log.name.clone())
                .collect::<Vec<_>>();
            if ui
                .add_enabled(
                    !new.is_empty(),
                    egui::Button::new("Select new").frame(false),
                )
                .clicked()
            {
                self.view.selected.extend(new);
            }
            if ui
                .add_enabled(!names.is_empty(), egui::Button::new("Clear").frame(false))
                .clicked()
            {
                self.view.selected.clear();
            }
            if !names.is_empty() {
                ui.weak(format!("{} selected · {}", names.len(), megabytes(bytes)));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if names.is_empty() {
                    "Download & import".to_owned()
                } else {
                    format!("Download & import {}", names.len())
                };
                let import = ui.add_enabled_ui(ready, |ui| widgets::primary_button(ui, &label));
                if import.inner.clicked() {
                    self.send(Command::Download {
                        names: names.clone(),
                        import: true,
                    });
                }
                if ui
                    .add_enabled(ready, egui::Button::new("Download only"))
                    .clicked()
                {
                    self.send(Command::Download {
                        names: names.clone(),
                        import: false,
                    });
                }
            });
        });
    }

    fn no_sessions(&mut self, ui: &mut Ui) {
        let switching = self.settings.connection == Connection::AppSwitches;
        match self.presence.clone() {
            Presence::Connected { fingerprint, .. } => {
                if self.activity == Activity::Listing {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                        ui.weak("Reading sessions…");
                    });
                } else if self.listed_for.as_deref() == Some(fingerprint.as_str()) {
                    widgets::empty_state(
                        ui,
                        "No sessions",
                        "The logger's memory is empty.",
                        |_| {},
                    );
                } else {
                    let mut retry = false;
                    widgets::empty_state(ui, "Couldn't read sessions", "", |ui| {
                        retry = ui.button("Retry").clicked();
                    });
                    if retry {
                        self.send(Command::Refresh);
                    }
                }
            }
            Presence::Visible { ssid } => {
                let mut join = false;
                widgets::empty_state(ui, &format!("{ssid} is in range"), "", |ui| {
                    join = self.activity == Activity::Idle
                        && widgets::primary_button(ui, "Join").clicked();
                });
                if join {
                    self.send(Command::Connect);
                }
            }
            Presence::Absent => {
                let body = if switching {
                    "Turn on the logger's Wi-Fi."
                } else {
                    "Turn on its Wi-Fi, then join AiM-MYC6-… on this computer."
                };
                widgets::empty_state(ui, "Connect to your MyChron", body, |_| {});
                if switching {
                    ui.vertical_centered(|ui| self.permission_prompt(ui));
                }
            }
        }
    }

    /// Location access is how macOS reveals Wi-Fi names.
    fn permission_prompt(&mut self, ui: &mut Ui) {
        let Some(wifi) = &self.wifi else {
            return;
        };
        match wifi.permission() {
            Permission::NotDetermined => {
                ui.horizontal(|ui| {
                    widgets::chip(ui, "Location access needed", Tone::Warn);
                    if ui.button("Allow…").clicked() {
                        wifi.request_permission();
                    }
                });
            }
            Permission::Denied => {
                ui.horizontal(|ui| {
                    widgets::chip(ui, "Location access off", Tone::Bad)
                        .on_hover_text("System Settings › Privacy › Location Services");
                });
            }
            Permission::Granted | Permission::NotNeeded => {}
        }
    }

    fn track_mode(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.track_hero(ui);
                ui.add_space(6.0);
                widgets::section_label(ui, "Logger");
                widgets::card(ui, false, |ui| self.track_logger(ui));
                ui.add_space(6.0);
                widgets::section_label(ui, "Connection");
                widgets::card(ui, false, |ui| self.track_connection(ui));
                ui.add_space(6.0);
                widgets::section_label(ui, "Sessions");
                widgets::card(ui, false, |ui| self.track_sessions(ui));
                ui.add_space(6.0);
                widgets::section_label(ui, "After download");
                widgets::card(ui, false, |ui| self.track_after(ui));
                ui.add_space(6.0);
                widgets::section_label(ui, "Activity");
                self.activity_feed(ui);
            });
    }

    fn track_hero(&mut self, ui: &mut Ui) {
        let on = self.settings.track_mode;
        let status = if !on {
            "Downloads new sessions when the logger is in range".to_owned()
        } else {
            let mut parts = vec![
                match self.presence {
                    Presence::Connected { .. } => "Connected",
                    Presence::Visible { .. } => "In range",
                    Presence::Absent => "Watching",
                }
                .to_owned(),
            ];
            if let Some(at) = &self.last_sync {
                parts.push(format!("checked {at}"));
            }
            if self.fetched > 0 {
                parts.push(format!("{} new", self.fetched));
            }
            parts.join(" · ")
        };
        widgets::card(ui, on, |ui| {
            ui.horizontal(|ui| {
                widgets::toggle(ui, &mut self.settings.track_mode);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Track mode")
                            .size(15.0)
                            .strong()
                            .color(text::strong()),
                    );
                    ui.label(RichText::new(status).small().color(text::weak()));
                });
            });
        });
    }

    fn track_logger(&mut self, ui: &mut Ui) {
        ui.radio_value(&mut self.settings.watch, None, "Any MyChron");
        for logger in &mut self.settings.loggers {
            ui.horizontal(|ui| {
                ui.radio_value(
                    &mut self.settings.watch,
                    Some(logger.fingerprint.clone()),
                    "",
                );
                ui.add(egui::TextEdit::singleline(&mut logger.name).desired_width(180.0));
                if let Some(ssid) = &logger.ssid
                    && *ssid != logger.name
                {
                    ui.weak(ssid);
                }
            });
        }
        if self.settings.loggers.is_empty() {
            widgets::hint(ui, "Loggers appear here once connected");
        }
    }

    fn track_connection(&mut self, ui: &mut Ui) {
        if self.wifi.is_none() {
            self.settings.connection = Connection::UserJoins;
            widgets::hint(ui, "Join the logger's Wi-Fi on this computer");
            return;
        }
        widgets::segmented(
            ui,
            &mut self.settings.connection,
            &[
                (Connection::UserJoins, "I join its Wi-Fi", ""),
                (Connection::AppSwitches, "Switch Wi-Fi for me", ""),
            ],
        );
        if self.settings.connection != Connection::AppSwitches {
            return;
        }
        ui.add_space(4.0);
        self.permission_prompt(ui);
        if self.interfaces.len() > 1 {
            ui.horizontal(|ui| {
                ui.label("Interface");
                let current = self
                    .settings
                    .interface
                    .clone()
                    .unwrap_or_else(|| "Default".into());
                egui::ComboBox::from_id_salt("mychron-interface")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.settings.interface, None, "Default");
                        for interface in &self.interfaces {
                            ui.selectable_value(
                                &mut self.settings.interface,
                                Some(interface.clone()),
                                interface,
                            );
                        }
                    });
            });
        } else {
            widgets::hint(ui, "Internet pauses while syncing");
        }
    }

    fn track_sessions(&mut self, ui: &mut Ui) {
        widgets::segmented(
            ui,
            &mut self.settings.since,
            &[
                (Since::AnyDate, "Any date", ""),
                (Since::Today, "Today", ""),
                (Since::LastWeek, "Last 7 days", ""),
            ],
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Wait in range");
            ui.add(
                egui::DragValue::new(&mut self.settings.settle_seconds)
                    .range(0..=600)
                    .suffix(" s"),
            )
            .on_hover_text("Skips karts passing by");
            ui.add_space(12.0);
            ui.label("Re-check every");
            ui.add(
                egui::DragValue::new(&mut self.settings.recheck_minutes)
                    .range(1..=60)
                    .suffix(" min"),
            );
        });
    }

    fn track_after(&mut self, ui: &mut Ui) {
        widgets::segmented(
            ui,
            &mut self.settings.after_download,
            &[
                (AfterDownload::Import, "Import to Analysis", ""),
                (AfterDownload::KeepInLibrary, "Keep in library", ""),
            ],
        );
        ui.add_space(4.0);
        let root = self.settings.library_root();
        ui.horizontal(|ui| {
            ui.label("Library");
            if ui.button("Open").clicked() {
                reveal(&root);
            }
            if ui.button("Change…").clicked()
                && let Some(folder) = rfd::FileDialog::new().set_directory(&root).pick_folder()
            {
                self.settings.library = Some(folder);
            }
            ui.add(
                egui::Label::new(RichText::new(root.display().to_string()).color(text::weak()))
                    .truncate(),
            )
            .on_hover_text(root.display().to_string());
        });
        if KeepAwake::SUPPORTED {
            ui.checkbox(&mut self.settings.keep_awake, "Keep computer awake");
        }
    }

    fn activity_feed(&self, ui: &mut Ui) {
        if self.feed.is_empty() {
            widgets::hint(ui, "Nothing yet");
            return;
        }
        for line in &self.feed {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&line.at).monospace().color(text::weak()));
                let color = match line.tone {
                    Tone::Neutral => text::normal(),
                    tone => tone.color(),
                };
                ui.add(egui::Label::new(RichText::new(&line.text).color(color)).wrap());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn row_states_follow_downloads_and_the_workspace() {
        let mut device = overlay_logger::fake::FakeDevice::default();
        device.add_recording("a_0001.xrz", "2026-08-30 15:00:00", "KELLYS", vec![0; 10]);
        let log = device.recordings.remove(0).log;
        let mut row = Row {
            log,
            downloaded: None,
        };
        let workspace = PathBuf::from("/lib/a.xrz");
        let in_workspace = |path: &Path| path == workspace;
        assert_eq!(row_state(&row, &in_workspace), RowState::New);
        row.downloaded = Some(PathBuf::from("/lib/b.xrz"));
        assert_eq!(row_state(&row, &in_workspace), RowState::Downloaded);
        row.downloaded = Some(workspace.clone());
        assert_eq!(row_state(&row, &in_workspace), RowState::Imported);
        assert!(shows(Filter::Downloaded, RowState::Imported));
        assert!(!shows(Filter::New, RowState::Imported));
        assert!(matches_search(&row, "kel"));
        assert!(!matches_search(&row, "gvkc"));
        assert_eq!(megabytes(477_678), "478 KB");
        assert_eq!(megabytes(1_400_000), "1.4 MB");
        assert_eq!(duration(141_383), "2:21");
    }
}
