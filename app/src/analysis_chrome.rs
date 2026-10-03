//! Analysis window chrome: the command toolbar, the media-style transport bar,
//! and the first-run state. Panel content lives elsewhere.

use super::*;
use crate::ui_kit::{
    theme::{self, Tone, surface},
    widgets,
};
use eframe::egui::Margin;

/// Layout presets offered by the Panels menu, as (name, description).
pub const LAYOUT_PRESETS: [(&str, &str); 3] = [
    ("Quick Compare", "Plots and maps with two videos"),
    ("Data Focus", "No videos; maximum plot area"),
    ("Video Compare", "Large side-by-side videos"),
];

/// A kind of panel that can be added to the Analysis workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelChoice {
    ChannelPlot,
    ScatterPlot,
    TimeDelta,
    Video,
    CourseMap,
    GpsImagery,
    Stats,
    Recordings,
    Setup,
}

impl PanelChoice {
    pub const ALL: [PanelChoice; 9] = [
        Self::ChannelPlot,
        Self::ScatterPlot,
        Self::TimeDelta,
        Self::Video,
        Self::CourseMap,
        Self::GpsImagery,
        Self::Stats,
        Self::Recordings,
        Self::Setup,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ChannelPlot => "Channel plot",
            Self::ScatterPlot => "X/Y scatter plot",
            Self::TimeDelta => "Time gain / loss",
            Self::Video => "Video",
            Self::CourseMap => "Channel course map",
            Self::GpsImagery => "GPS imagery map",
            Self::Stats => "Values & statistics",
            Self::Recordings => "Recordings & laps",
            Self::Setup => "Timing & course setup",
        }
    }

    fn kind(self) -> TabKind {
        match self {
            Self::ChannelPlot => TabKind::Plot(Default::default()),
            Self::ScatterPlot => TabKind::Scatter(Default::default()),
            Self::TimeDelta => TabKind::Plot(PlotOptions {
                delta: true,
                ..Default::default()
            }),
            Self::Video => TabKind::Video(VideoOptions {
                slot: 0,
                segment: None,
                linked: true,
                time: 0.0,
                unknown: Default::default(),
            }),
            Self::CourseMap => TabKind::Map {
                channel: "gps_speed".into(),
                settings: Default::default(),
            },
            Self::GpsImagery => TabKind::Map {
                channel: "gps_speed".into(),
                settings: Box::new(MapSettings {
                    actual_gps: true,
                    small_multiples: false,
                    ..Default::default()
                }),
            },
            Self::Stats => TabKind::Stats,
            Self::Recordings => TabKind::Browser,
            Self::Setup => TabKind::Setup,
        }
    }
}

impl AnalysisApp {
    /// Left-hand commands, drawn inside the application header.
    pub fn header_left(&mut self, ui: &mut egui::Ui) {
        self.file_commands(ui);
        ui.separator();
        self.panel_menu(ui);
    }

    /// Right-hand controls, drawn inside the application header.
    pub fn header_right(&mut self, ui: &mut egui::Ui) {
        self.axis_selector(ui);
    }

    fn file_commands(&mut self, ui: &mut egui::Ui) {
        if ui.button("Add files…").clicked() {
            self.add_files_dialog();
        }
        if ui.button("MyChron…").clicked() {
            self.actions.push(AnalysisAction::OpenMyChron);
        }
        widgets::action_menu(ui, "File ⏷", |ui| {
            if widgets::menu_item(ui, "Open workspace…", None) {
                self.open_workspace_dialog();
                ui.close();
            }
            if widgets::menu_item(ui, "Save", None) {
                self.save(false);
                ui.close();
            }
            if widgets::menu_item(ui, "Save as…", None) {
                self.save(true);
                ui.close();
            }
        });
        if self.dirty && widgets::primary_button(ui, "Save").clicked() {
            self.save(false);
        }
    }

    /// Asks for telemetry, video, or workspace files and imports them.
    pub fn add_files_dialog(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter(
                "Data / video",
                &[
                    "xrk", "xrz", "hrz", "csv", "insv", "lrv", "mp4", "mov", "mkv",
                ],
            )
            .pick_files()
        {
            self.add_paths(paths);
        }
    }

    pub fn save_workspace_as(&mut self) {
        self.save(true);
    }

    pub fn open_workspace_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Workspace / overlay project", &["json"])
            .pick_file()
        {
            self.open_path(path);
        }
    }

    fn panel_menu(&mut self, ui: &mut egui::Ui) {
        widgets::action_menu(ui, "Panels ⏷", |ui| {
            widgets::section_label(ui, "Add a panel");
            for choice in PanelChoice::ALL {
                if widgets::menu_item(ui, choice.label(), None) {
                    self.add_panel(choice);
                    ui.close();
                }
            }
            ui.separator();
            widgets::section_label(ui, "Layout presets");
            for (index, (label, caption)) in LAYOUT_PRESETS.iter().enumerate() {
                if widgets::menu_item(ui, label, Some(caption)) {
                    self.layout(index);
                    ui.close();
                }
            }
        });
    }

    /// Adds a panel to the focused area of the dock.
    pub fn add_panel(&mut self, choice: PanelChoice) {
        self.initial_layout_pending = false;
        let tab = self.tab(choice.kind());
        self.dock.push_to_focused_leaf(tab);
        self.dirty = true;
    }

    /// Replaces the layout with preset `index` of [`LAYOUT_PRESETS`].
    pub fn layout_preset(&mut self, index: usize) {
        self.initial_layout_pending = false;
        self.layout(index.min(LAYOUT_PRESETS.len() - 1));
    }

    fn axis_selector(&mut self, ui: &mut egui::Ui) {
        let aligned =
            automatic_alignment_complete(&self.prepared, self.workspace.reference.as_ref());
        let time_label = if aligned { "Aligned time" } else { "Time" };
        let before = self.state.mode;
        let mut mode = self.state.mode;
        widgets::segmented(
            ui,
            &mut mode,
            &[
                (
                    XMode::Time,
                    time_label,
                    "Compare by elapsed time from each run's start",
                ),
                (
                    XMode::Course,
                    "Course",
                    "Compare at the same position along the reference GPS course",
                ),
                (
                    XMode::Distance,
                    "Distance",
                    "Compare by each run's own traveled distance",
                ),
            ],
        );
        if ui.ctx().content_rect().width() > 1250.0 {
            ui.weak("Compare by");
        }
        if mode != before {
            self.state.mode = mode;
            self.automatic_mode = false;
            self.plot_cache.clear();
            self.scatter_cache.clear();
            self.dirty = true;
        }
        if self.state.mode == XMode::Course && self.prepared.course.is_none() {
            widgets::chip(ui, "No reference GPS · showing time", Tone::Warn);
        }
    }

    /// Media-style transport pinned to the bottom of the workspace.
    pub(super) fn transport(&mut self, ui: &mut egui::Ui) {
        let duration = self
            .reference_run()
            .map_or(1.0, |run| run.end - run.start)
            .max(0.001);
        egui::Panel::bottom("analysis-transport")
            .frame(theme::bar_frame(surface::bar(), Margin::symmetric(10, 6)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (glyph, tip) = if self.playing {
                        ("⏸", "Pause (Space)")
                    } else {
                        ("▶", "Play all linked panels (Space)")
                    };
                    if widgets::primary_button(ui, glyph)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.toggle_play();
                    }
                    ui.monospace(format!(
                        "{} / {}",
                        widgets::clock(self.state.cursor),
                        widgets::clock(duration)
                    ))
                    .on_hover_text("Reference elapsed time");
                    let reserved = 210.0;
                    ui.spacing_mut().slider_width = (ui.available_width() - reserved).max(120.0);
                    ui.add(
                        egui::Slider::new(&mut self.state.cursor, 0.0..=duration).show_value(false),
                    );
                    self.range_controls(ui, duration);
                });
            });
    }

    fn range_controls(&mut self, ui: &mut egui::Ui, duration: f64) {
        let label = match self.state.range {
            Some([start, end]) => format!("Range {start:.1}–{end:.1} s ⏷"),
            None => "Stats range ⏷".into(),
        };
        widgets::popover(ui, label, |ui| {
            widgets::hint(
                ui,
                "Limit the statistics panel to part of the reference run. Move the playhead, then mark the range edges.",
            );
            ui.horizontal(|ui| {
                if ui.button("Start here").clicked() {
                    self.state
                        .range
                        .get_or_insert([self.state.cursor, duration])[0] = self.state.cursor;
                }
                if ui.button("Finish here").clicked() {
                    self.state.range.get_or_insert([0.0, self.state.cursor])[1] = self.state.cursor;
                }
                if ui
                    .add_enabled(self.state.range.is_some(), egui::Button::new("Clear"))
                    .clicked()
                {
                    self.state.range = None;
                }
            });
        });
        if ui.ctx().content_rect().width() > 900.0 {
            ui.weak("◀ ▶ keys step");
        }
    }

    /// First-run content: what this window does and how to start.
    pub(super) fn welcome(&mut self, ui: &mut egui::Ui) {
        let mut add_files = false;
        let mut open_workspace = false;
        let mut from_mychron = false;
        widgets::empty_state(
            ui,
            "Compare laps and runs",
            "Drop XRK or CSV logs anywhere in this window.\nLaps are detected automatically; video and saved tracks are optional.",
            |ui| {
                if widgets::primary_button(ui, "Add files…").clicked() {
                    add_files = true;
                }
                if ui.button("Open workspace…").clicked() {
                    open_workspace = true;
                }
                if ui.button("From MyChron…").clicked() {
                    from_mychron = true;
                }
                ui.add_space(14.0);
                for step in [
                    "1 · Drop one or more logs",
                    "2 · Tick the laps to compare and pin a reference",
                    "3 · Optionally attach video and align it to the log",
                ] {
                    widgets::hint(ui, step);
                }
            },
        );
        if add_files
            && let Some(paths) = rfd::FileDialog::new()
                .add_filter(
                    "Data / video",
                    &[
                        "xrk", "xrz", "hrz", "csv", "insv", "lrv", "mp4", "mov", "mkv",
                    ],
                )
                .pick_files()
        {
            self.add_paths(paths);
        }
        if open_workspace {
            self.open_workspace_dialog();
        }
        if from_mychron {
            self.actions.push(AnalysisAction::OpenMyChron);
        }
    }
}
