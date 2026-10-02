//! The Settings window: preferences the application keeps across workspaces.
//! Currently the aerial imagery sources maps download from, including a
//! search for public sources near a course.

use crate::analysis_imagery::GeoBounds;
use crate::imagery_search::{self, Found, Progress};
use crate::imagery_sources::{self, ImagerySource};
use crate::ui_kit::{Tone, theme::text, widgets};
use eframe::egui::{self, RichText};
use std::{
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{self, Receiver},
    },
    time::Duration,
};

const OPEN_REQUEST: &str = "race-overlay.open-settings";
const FIND_REQUEST: &str = "race-overlay.find-imagery";

/// Asks the application to open Settings, from anywhere that has a `Ui`.
pub fn request_open(ctx: &egui::Context) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(OPEN_REQUEST), true));
}

/// Opens Settings and searches for imagery sources covering `course`.
pub fn request_find(ctx: &egui::Context, course: GeoBounds) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(FIND_REQUEST), Some(course)));
}

/// A search for sources near one course, running or finished.
struct Search {
    course: GeoBounds,
    progress: Arc<Progress>,
    pending: Option<Receiver<Result<Found, String>>>,
    result: Option<Result<Found, String>>,
}

impl Search {
    fn start(course: GeoBounds) -> Self {
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let worker = progress.clone();
        // Picked up by `show`, which repaints while the search runs.
        std::thread::spawn(move || {
            let _ = tx.send(imagery_search::find(course, &worker));
        });
        Self {
            course,
            progress,
            pending: Some(rx),
            result: None,
        }
    }
}

#[derive(Default)]
pub struct SettingsWindow {
    pub open: bool,
    link: String,
    probe: Option<PendingProbe>,
    error: Option<String>,
    /// The source whose link is being changed, and the draft link.
    relinking: Option<(usize, String)>,
    search: Option<Search>,
}

struct PendingProbe {
    target: ProbeTarget,
    result: Receiver<Result<ImagerySource, String>>,
}

#[derive(Clone, Copy)]
enum ProbeTarget {
    Add,
    Replace(usize),
}

impl SettingsWindow {
    pub fn show(&mut self, ctx: &egui::Context, sources: &mut Vec<ImagerySource>) {
        if ctx.data_mut(|data| data.remove_temp::<bool>(egui::Id::new(OPEN_REQUEST))) == Some(true)
        {
            self.open = true;
        }
        if let Some(course) = ctx
            .data_mut(|data| data.remove_temp::<Option<GeoBounds>>(egui::Id::new(FIND_REQUEST)))
            .flatten()
        {
            self.open = true;
            let running_here = self
                .search
                .as_ref()
                .is_some_and(|search| search.course == course && search.pending.is_some());
            if !running_here {
                self.search = Some(Search::start(course));
            }
        }
        self.finish_probe(sources);
        if let Some(search) = &mut self.search
            && let Some(result) = search.pending.as_ref().and_then(|rx| rx.try_recv().ok())
        {
            search.pending = None;
            search.result = Some(result);
        }
        if !self.open {
            return;
        }
        let mut open = true;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(480.0)
            .default_height(560.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(ctx.content_rect().height() * 0.75)
                    .show(ui, |ui| {
                        self.found_sources(ui, sources);
                        self.imagery_sources(ui, sources);
                    });
            });
        self.open = open;
        if self.probe.is_some()
            || self
                .search
                .as_ref()
                .is_some_and(|search| search.pending.is_some())
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Search results: verified services near the course, to add with one
    /// click.
    fn found_sources(&mut self, ui: &mut egui::Ui, sources: &mut Vec<ImagerySource>) {
        let Some(search) = &self.search else {
            return;
        };
        widgets::section_label(ui, "Found near this course");
        let mut restart = false;
        match &search.result {
            None => {
                let total = search.progress.total.load(Ordering::Relaxed);
                let checked = search.progress.checked.load(Ordering::Relaxed);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(if total == 0 {
                        "Searching…".to_owned()
                    } else {
                        format!("Checking {checked} of {total} services…")
                    });
                });
            }
            Some(Err(error)) => {
                widgets::callout(ui, Tone::Bad, error);
                restart = ui.small_button("Search again").clicked();
            }
            Some(Ok(found)) => {
                if found.sources.is_empty() {
                    widgets::hint(ui, "No public imagery services found here.");
                }
                for source in &found.sources {
                    let added = sources.iter().any(|mine| mine.url == source.url);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(ui.available_width() - 70.0);
                            ui.label(&source.name);
                            let credit = [
                                source.attribution.as_str(),
                                imagery_search::coverage_label(source),
                            ]
                            .into_iter()
                            .filter(|part| !part.is_empty())
                            .collect::<Vec<_>>()
                            .join(" · ");
                            ui.label(RichText::new(credit).small().color(text::weak()));
                        });
                        if added {
                            ui.label(RichText::new("Added").small().color(text::weak()));
                        } else if ui.button("Add").clicked() {
                            match apply_probe(sources, ProbeTarget::Add, source.clone()) {
                                Ok(()) => self.error = None,
                                Err(error) => self.error = Some(error),
                            }
                        }
                    });
                    ui.separator();
                }
                if found.hidden > 0 {
                    widgets::hint(
                        ui,
                        format!(
                            "{} more hidden: no imagery here, sign-in required, or unreachable.",
                            found.hidden
                        ),
                    );
                }
                restart = ui.small_button("Search again").clicked();
            }
        }
        if restart {
            let course = search.course;
            self.search = Some(Search::start(course));
        }
        ui.add_space(8.0);
    }

    fn imagery_sources(&mut self, ui: &mut egui::Ui, sources: &mut Vec<ImagerySource>) {
        widgets::section_label(ui, "Your imagery sources");
        widgets::hint(ui, "Tried in order. USGS is the fallback.");
        let busy = self.probe.is_some();
        let mut action = None;
        let count = sources.len();
        for (index, source) in sources.iter_mut().enumerate() {
            widgets::card(ui, false, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut source.enabled, "")
                        .on_hover_text("Use this source");
                    ui.add(
                        egui::TextEdit::singleline(&mut source.name)
                            .desired_width(ui.available_width() - 150.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::danger_button(ui, "Remove").clicked() {
                            action = Some(Action::Remove(index));
                        }
                        if ui
                            .add_enabled(index + 1 < count, egui::Button::new("⏷").small())
                            .on_hover_text("Try later")
                            .clicked()
                        {
                            action = Some(Action::Swap(index, index + 1));
                        }
                        if ui
                            .add_enabled(index > 0, egui::Button::new("⏶").small())
                            .on_hover_text("Try sooner")
                            .clicked()
                        {
                            action = Some(Action::Swap(index - 1, index));
                        }
                    });
                });
                ui.label(RichText::new(&source.url).small().color(text::weak()));
                if !source.attribution.is_empty() {
                    ui.label(
                        RichText::new(&source.attribution)
                            .small()
                            .color(text::weak()),
                    );
                }
                if source.coverage.is_none() {
                    ui.label(
                        RichText::new("Coverage unknown; tried for every course")
                            .small()
                            .color(Tone::Warn.color()),
                    );
                }
                match &mut self.relinking {
                    Some((editing, link)) if *editing == index => {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(link)
                                    .desired_width(ui.available_width() - 120.0),
                            );
                            if ui
                                .add_enabled(
                                    !busy && !link.trim().is_empty(),
                                    egui::Button::new("Save"),
                                )
                                .clicked()
                            {
                                action =
                                    Some(Action::Probe(ProbeTarget::Replace(index), link.clone()));
                            }
                            if ui.button("Cancel").clicked() {
                                action = Some(Action::CancelRelink);
                            }
                        });
                    }
                    _ => {
                        if ui.small_button("Change link").clicked() {
                            action = Some(Action::Relink(index, source.url.clone()));
                        }
                    }
                }
            });
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.link)
                    .hint_text("ArcGIS MapServer or ImageServer link")
                    .desired_width(ui.available_width() - 60.0),
            );
            if ui
                .add_enabled(
                    !busy && !self.link.trim().is_empty(),
                    egui::Button::new("Add"),
                )
                .clicked()
            {
                action = Some(Action::Probe(ProbeTarget::Add, self.link.clone()));
            }
        });
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking the service…");
            });
        }
        if let Some(error) = &self.error {
            widgets::callout(ui, Tone::Bad, error);
        }
        match action {
            Some(Action::Remove(index)) => {
                sources.remove(index);
                self.relinking = None;
            }
            Some(Action::Swap(a, b)) => {
                sources.swap(a, b);
                self.relinking = None;
            }
            Some(Action::Relink(index, url)) => self.relinking = Some((index, url)),
            Some(Action::CancelRelink) => self.relinking = None,
            Some(Action::Probe(target, link)) => self.start_probe(target, link),
            None => {}
        }
    }

    fn start_probe(&mut self, target: ProbeTarget, link: String) {
        let (tx, rx) = mpsc::channel();
        // Results are picked up by `show`, which repaints while one is due.
        std::thread::spawn(move || {
            let _ = tx.send(imagery_sources::probe(&link));
        });
        self.error = None;
        self.probe = Some(PendingProbe { target, result: rx });
    }

    fn finish_probe(&mut self, sources: &mut Vec<ImagerySource>) {
        let Some(result) = self
            .probe
            .as_ref()
            .and_then(|probe| probe.result.try_recv().ok())
        else {
            return;
        };
        let target = self.probe.take().map(|probe| probe.target);
        match (result, target) {
            (Err(error), _) => self.error = Some(error),
            (Ok(found), Some(target)) => {
                if let Err(error) = apply_probe(sources, target, found) {
                    self.error = Some(error);
                } else if matches!(target, ProbeTarget::Add) {
                    self.link.clear();
                } else {
                    self.relinking = None;
                }
            }
            (Ok(_), None) => {}
        }
    }
}

enum Action {
    Remove(usize),
    Swap(usize, usize),
    Relink(usize, String),
    CancelRelink,
    Probe(ProbeTarget, String),
}

/// Adds a probed source, or replaces one's service while keeping the name and
/// on/off choice the user gave it.
fn apply_probe(
    sources: &mut Vec<ImagerySource>,
    target: ProbeTarget,
    found: ImagerySource,
) -> Result<(), String> {
    let duplicate = sources.iter().enumerate().any(|(index, source)| {
        source.url == found.url && !matches!(target, ProbeTarget::Replace(i) if i == index)
    });
    if duplicate || found.is_usgs() {
        return Err("That service is already in the list".into());
    }
    match target {
        ProbeTarget::Add => sources.push(found),
        ProbeTarget::Replace(index) => {
            let source = sources.get_mut(index).ok_or("That source was removed")?;
            *source = ImagerySource {
                name: std::mem::take(&mut source.name),
                enabled: source.enabled,
                unknown: std::mem::take(&mut source.unknown),
                ..found
            };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(name: &str, url: &str) -> ImagerySource {
        let mut source = ImagerySource::usgs();
        source.name = name.into();
        source.url = url.into();
        source
    }

    #[test]
    fn adding_rejects_duplicates_and_the_built_in_fallback() {
        let mut sources = vec![source("NYS", "https://a.example/rest/services/A/MapServer")];
        let again = source("again", "https://a.example/rest/services/A/MapServer");
        assert!(apply_probe(&mut sources, ProbeTarget::Add, again).is_err());
        assert!(apply_probe(&mut sources, ProbeTarget::Add, ImagerySource::usgs()).is_err());
        let other = source("B", "https://b.example/rest/services/B/MapServer");
        assert!(apply_probe(&mut sources, ProbeTarget::Add, other).is_ok());
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn changing_a_link_keeps_the_name_and_choice() {
        let mut sources = vec![source(
            "My venue",
            "https://a.example/rest/services/A/MapServer",
        )];
        sources[0].enabled = false;
        let replacement = source(
            "Service title",
            "https://a.example/rest/services/B/MapServer",
        );

        apply_probe(&mut sources, ProbeTarget::Replace(0), replacement).unwrap();

        assert_eq!(sources[0].name, "My venue");
        assert!(!sources[0].enabled);
        assert!(sources[0].url.ends_with("/B/MapServer"));
        // Re-saving the same link is not a duplicate of itself.
        let same = source("x", "https://a.example/rest/services/B/MapServer");
        assert!(apply_probe(&mut sources, ProbeTarget::Replace(0), same).is_ok());
    }
}
