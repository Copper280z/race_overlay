//! Opt-in visual snapshots of the real application UI.
//!
//! These render the shell offscreen and write PNGs for manual review; they are
//! not assertions. They need a GPU adapter and the local `mychron_data`
//! recordings, so they stay ignored by default:
//!
//! ```sh
//! RACE_OVERLAY_SNAPSHOT_DIR=/tmp/ui cargo test --release -p race-overlay \
//!     render_ui_snapshots -- --ignored --nocapture
//! ```

use super::RaceOverlayApp;
use eframe::egui;
use egui_kittest::{Harness, kittest::Queryable};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

type Shell<'a> = Harness<'a, RaceOverlayApp>;

fn shoot(harness: &mut Shell<'_>, dir: &Path, name: &str) {
    harness.run_steps(6);
    let image = harness.render().expect("offscreen render");
    image
        .save(dir.join(format!("{name}.png")))
        .expect("write png");
    eprintln!("wrote {name}.png");
}

fn settle(harness: &mut Shell<'_>) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        harness.run_steps(2);
        if harness.state().analysis.is_settled() {
            break;
        }
        assert!(Instant::now() < deadline, "import did not finish");
        std::thread::sleep(Duration::from_millis(50));
    }
    harness.run_steps(4);
}

/// Clicks the nth control whose label contains `text`.
fn open_menu(harness: &mut Shell<'_>, text: &str, nth: usize) {
    harness
        .get_all_by_label_contains(text)
        .nth(nth)
        .unwrap_or_else(|| panic!("nothing labelled {text}"))
        .click();
    harness.run_steps(4);
}

fn close_menus(harness: &mut Shell<'_>) {
    harness.key_press(egui::Key::Escape);
    harness.run_steps(4);
}

fn focus(harness: &mut Shell<'_>, title: &str) {
    assert!(
        harness.state_mut().analysis.focus_tab_for_test(title),
        "{title}"
    );
}

#[test]
#[ignore = "writes PNGs; needs a GPU adapter and local mychron_data recordings"]
fn render_ui_snapshots() {
    let Some(dir) = std::env::var_os("RACE_OVERLAY_SNAPSHOT_DIR").map(PathBuf::from) else {
        eprintln!("set RACE_OVERLAY_SNAPSHOT_DIR to choose an output directory");
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let mut harness = Harness::builder()
        .wgpu()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .build_eframe(|cc| RaceOverlayApp::build(&*cc, false));
    shoot(&mut harness, &dir, "01-welcome");

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../mychron_data");
    let files = [
        "gvkc/a_0065.xrk",
        "8_30_26_autox/a_0078.xrk",
        "8_30_26_autox/a_0091.xrk",
    ]
    .map(|name| root.join(name));
    harness.state_mut().analysis.add_files(files.into());
    settle(&mut harness);
    shoot(&mut harness, &dir, "02-analysis-default");
    open_menu(&mut harness, "Legend (", 0);
    shoot(&mut harness, &dir, "02c-legend-overflow");
    close_menus(&mut harness);
    focus(&mut harness, "Time gain");
    shoot(&mut harness, &dir, "02b-time-gain");
    focus(&mut harness, "Channel plot");
    focus(&mut harness, "Timing");
    shoot(&mut harness, &dir, "03-setup-intervals");
    harness.state_mut().analysis.set_setup_tab_for_test(1);
    shoot(&mut harness, &dir, "03b-setup-sources");
    harness.state_mut().analysis.set_setup_tab_for_test(2);
    shoot(&mut harness, &dir, "03c-setup-gates");
    focus(&mut harness, "Recordings");
    focus(&mut harness, "Values");
    shoot(&mut harness, &dir, "03d-stats");
    focus(&mut harness, "GPS imagery");
    shoot(&mut harness, &dir, "03e-gps-map");
    focus(&mut harness, "Recordings");
    open_menu(&mut harness, "Panels", 0);
    shoot(&mut harness, &dir, "07-menu-panels");
    close_menus(&mut harness);
    open_menu(&mut harness, "Options", 0);
    shoot(&mut harness, &dir, "08-plot-options");
    close_menus(&mut harness);
    open_menu(&mut harness, "Channels", 0);
    shoot(&mut harness, &dir, "09-plot-channels");
    close_menus(&mut harness);
    open_menu(&mut harness, "More", 0);
    shoot(&mut harness, &dir, "10-recording-more");
    close_menus(&mut harness);
    open_menu(&mut harness, "Stats range", 0);
    shoot(&mut harness, &dir, "11-stats-range");
    close_menus(&mut harness);
    harness.set_size(egui::vec2(980.0, 640.0));
    shoot(&mut harness, &dir, "04-analysis-small");
    harness.set_size(egui::vec2(1440.0, 900.0));

    harness.state_mut().set_mode(false);
    harness.state_mut().editor.load_sample_for_test();
    harness.run_steps(10);
    shoot(&mut harness, &dir, "05-overlay");
    harness.state_mut().editor.show_data_plot_for_test(true);
    shoot(&mut harness, &dir, "06-overlay-data-plot");
    harness.state_mut().editor.show_data_plot_for_test(false);

    // Every built-in color scheme, in both workspaces.
    for scheme in crate::ui_kit::theme::THEMES {
        crate::ui_kit::theme::apply(&harness.ctx, &scheme.palette);
        harness.state_mut().set_mode(false);
        shoot(&mut harness, &dir, &format!("12-{}-overlay", scheme.id));
        harness.state_mut().set_mode(true);
        focus(&mut harness, "GPS imagery");
        shoot(&mut harness, &dir, &format!("12-{}-analysis", scheme.id));
    }
}

/// The MyChron window in its main states, with demo data (no logger needed).
#[test]
#[ignore = "writes PNGs; needs a GPU adapter"]
fn render_mychron_window() {
    use crate::mychron::window::Tab;
    let Some(dir) = std::env::var_os("RACE_OVERLAY_SNAPSHOT_DIR").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for (name, connected, tab, downloading) in [
        ("sessions", true, Tab::Sessions, false),
        ("downloading", true, Tab::Sessions, true),
        ("track-mode", true, Tab::TrackMode, false),
        ("not-connected", false, Tab::Sessions, false),
    ] {
        let mut harness = Harness::builder()
            .wgpu()
            .with_size(egui::vec2(1200.0, 800.0))
            .with_pixels_per_point(1.0)
            .build_eframe(|cc| RaceOverlayApp::build(&*cc, false));
        harness
            .state_mut()
            .mychron
            .demo_for_test(connected, tab, downloading);
        shoot(&mut harness, &dir, &format!("16-mychron-{name}"));
    }
}

/// Which decorative glyphs the bundled default fonts can actually draw.
#[test]
#[ignore = "writes a PNG; needs a GPU adapter"]
fn render_glyph_coverage() {
    let Some(dir) = std::env::var_os("RACE_OVERLAY_SNAPSHOT_DIR").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let glyphs = "▾ ▼ ▸ ▶ ⏸ ⏵ ⏹ ● • ○ ✓ ✔ ✕ ✖ × ⚠ ⚙ ☰ ≡ ⋯ … ← → ↔ ↺ ⟲ ★ ☆ ⌘ ⏷ ⌄ ⌃ ‹ › ⇥ ⤢ 📁 💾 ➕ ➖ ⬇ ⬆ ↩ ⟳ ⏮ ⏭ ⏪ ⏩ ◀ ▲ ◆ ■ □ ✎ ⋮ ⠿ ⚑ ⚐ ℹ ⓘ";
    let mut harness = Harness::builder()
        .wgpu()
        .with_size(egui::vec2(900.0, 160.0))
        .build_ui(|ui| {
            ui.horizontal_wrapped(|ui| {
                for g in glyphs.split(' ') {
                    ui.label(egui::RichText::new(g).size(22.0));
                    ui.label(egui::RichText::new("|").weak());
                }
            });
        });
    harness.run_steps(3);
    harness
        .render()
        .unwrap()
        .save(dir.join("glyphs.png"))
        .unwrap();
}

/// Drives the real settings menu with clicks: choosing a scheme must apply it.
#[test]
#[ignore = "needs a GPU adapter"]
fn theme_picker_changes_the_scheme() {
    use crate::ui_kit::theme;
    theme::apply(&egui::Context::default(), &theme::THEMES[0].palette);
    let mut harness = Harness::builder()
        .wgpu()
        .with_size(egui::vec2(1200.0, 800.0))
        .build_eframe(|cc| RaceOverlayApp::build(&*cc, false));
    harness.run_steps(3);
    open_menu(&mut harness, "⚙", 0);
    // The dropdown lives inside the settings popover; opening it must not
    // close that popover.
    harness.get_by_role(egui::accesskit::Role::ComboBox).click();
    harness.run_steps(4);
    harness.get_by_label_contains("Graphite").click();
    harness.run_steps(4);
    assert_eq!(theme::active().id, "graphite");
}

/// The reframing controls at a roomy and a cramped panel width.
#[test]
#[ignore = "writes PNGs; needs a GPU adapter"]
fn render_video_controls_widths() {
    let Some(dir) = std::env::var_os("RACE_OVERLAY_SNAPSHOT_DIR").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for (name, width) in [("wide", 760.0), ("narrow", 300.0)] {
        let mut config = overlay_core::VideoProcessingConfig::default();
        let mut harness = Harness::builder()
            .wgpu()
            .with_size(egui::vec2(width, 70.0))
            .build_ui(move |ui| {
                crate::ui_kit::theme::apply(ui.ctx(), &crate::ui_kit::theme::THEMES[0].palette);
                ui.horizontal_wrapped(|ui| {
                    ui.label("a_0082 / Autocross run");
                    ui.checkbox(&mut true, "Linked");
                    crate::video_processing::controls(ui, &mut config);
                });
            });
        harness.run_steps(4);
        harness
            .render()
            .unwrap()
            .save(dir.join(format!("13-video-controls-{name}.png")))
            .unwrap();
    }
}

/// The Video panel with a real INSV from the repository root, if present
/// (media files are git-ignored).
#[test]
#[ignore = "writes PNGs; needs a GPU adapter, FFmpeg, and an .insv in the repo root"]
fn render_video_panel() {
    let Some(dir) = std::env::var_os("RACE_OVERLAY_SNAPSHOT_DIR").map(PathBuf::from) else {
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let Some(insv) = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("insv"))
        })
    else {
        eprintln!("no .insv in the repository root; skipping");
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let mut harness = Harness::builder()
        .wgpu()
        .with_size(egui::vec2(1440.0, 900.0))
        .build_eframe(|cc| RaceOverlayApp::build(&*cc, false));
    let log = root.join("mychron_data/8_30_26_autox/a_0082.xrk");
    if log.exists() {
        // The video was recorded during this log.
        harness.state_mut().analysis.add_files(vec![log]);
        settle(&mut harness);
        harness.state_mut().analysis.attach_video_to_first(insv);
    } else {
        harness.state_mut().analysis.add_files(vec![insv]);
    }
    settle(&mut harness);
    // Frames decode on worker threads; give them time to arrive.
    harness.state_mut().analysis.layout_preset(0);
    for _ in 0..80 {
        harness.run_steps(2);
        std::thread::sleep(Duration::from_millis(100));
    }
    shoot(&mut harness, &dir, "14-video-wide");
    harness.set_size(egui::vec2(1000.0, 700.0));
    for _ in 0..10 {
        harness.run_steps(2);
        std::thread::sleep(Duration::from_millis(100));
    }
    shoot(&mut harness, &dir, "14-video-narrow");
}

/// A physical 3840×2160 display at common desktop scale factors. Only the
/// logical workspace size changes; fonts and controls retain their point sizes.
#[test]
#[ignore = "writes 4K PNGs; needs a GPU adapter, FFmpeg, and local recordings"]
fn render_4k_snapshots() {
    let Some(dir) = std::env::var_os("RACE_OVERLAY_SNAPSHOT_DIR").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let scales = [("100", 1.0), ("150", 1.5), ("200", 2.0)];
    let resize = |harness: &mut Shell<'_>, scale| {
        harness.set_pixels_per_point(scale);
        harness.set_size(egui::vec2(3840.0 / scale, 2160.0 / scale));
    };
    let capture = |harness: &mut Shell<'_>, name: &str| {
        harness.run_steps(6);
        let image = harness.render().expect("offscreen 4K render");
        assert_eq!(image.dimensions(), (3840, 2160));
        image.save(dir.join(format!("{name}.png"))).unwrap();
        eprintln!("wrote {name}.png (3840×2160)");
    };
    let build = || {
        Harness::builder()
            .wgpu()
            .with_size(egui::vec2(1440.0, 900.0))
            .with_pixels_per_point(1.0)
            .build_eframe(|cc| RaceOverlayApp::build(&*cc, false))
    };

    let mut harness = build();
    harness.state_mut().analysis.add_files(
        [
            "gvkc/a_0065.xrk",
            "8_30_26_autox/a_0078.xrk",
            "8_30_26_autox/a_0091.xrk",
        ]
        .map(|name| root.join("mychron_data").join(name))
        .into(),
    );
    settle(&mut harness);
    focus(&mut harness, "Values");
    for (name, scale) in scales {
        resize(&mut harness, scale);
        capture(&mut harness, &format!("4k-analysis-{name}"));
    }
    harness.state_mut().set_mode(false);
    harness.state_mut().editor.load_sample_for_test();
    for (name, scale) in scales {
        resize(&mut harness, scale);
        capture(&mut harness, &format!("4k-overlay-{name}"));
    }
    drop(harness);

    let mut harness = build();
    harness
        .state_mut()
        .analysis
        .add_files(vec![root.join("mychron_data/8_30_26_autox/a_0082.xrk")]);
    settle(&mut harness);
    harness
        .state_mut()
        .analysis
        .attach_video_to_first(root.join("VID_20260830_124108_00_017.insv"));
    harness.state_mut().analysis.layout_preset(0);
    settle(&mut harness);
    for _ in 0..80 {
        harness.run_steps(2);
        std::thread::sleep(Duration::from_millis(100));
    }
    for (name, scale) in scales {
        resize(&mut harness, scale);
        for _ in 0..20 {
            harness.run_steps(2);
            std::thread::sleep(Duration::from_millis(50));
        }
        capture(&mut harness, &format!("4k-video-{name}"));
    }
}

/// A video and a log recorded together end up in one entry, synchronized,
/// whichever is added first.
#[test]
#[ignore = "needs a GPU adapter, FFmpeg, the repo-root .insv and local mychron_data (takes ~20 s)"]
fn video_and_log_pair_and_sync_in_either_order() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let insv = root.join("VID_20260830_124108_00_017.insv");
    let logs = ["a_0080", "a_0082", "a_0083"]
        .map(|name| root.join(format!("mychron_data/8_30_26_autox/{name}.xrk")));
    if !insv.exists() || logs.iter().any(|log| !log.exists()) {
        eprintln!("local recordings missing; skipping");
        return;
    }
    for log_first in [false, true] {
        let mut harness = Harness::builder()
            .wgpu()
            .with_size(egui::vec2(1440.0, 900.0))
            .build_eframe(|cc| RaceOverlayApp::build(&*cc, false));
        let (first, second) = if log_first {
            (logs.to_vec(), vec![insv.clone()])
        } else {
            (vec![insv.clone()], logs.to_vec())
        };
        harness.state_mut().analysis.add_files(first);
        settle(&mut harness);
        harness.state_mut().analysis.add_files(second);
        settle(&mut harness);
        // Audio match, calibration, and the correlation run on workers.
        let deadline = Instant::now() + Duration::from_secs(120);
        let synced = loop {
            harness.run_steps(1);
            let synced = harness
                .state()
                .analysis
                .workspace
                .recordings
                .iter()
                .any(|r| r.video_offset_seconds != 0.0);
            if synced || Instant::now() > deadline {
                break synced;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let workspace = &harness.state().analysis.workspace;
        assert!(
            synced,
            "log_first={log_first}: {}",
            harness.state().analysis.message()
        );
        // Only the log recorded during the video joined it.
        assert_eq!(workspace.recordings.len(), 3, "log_first={log_first}");
        let entry = workspace
            .recordings
            .iter()
            .find(|r| r.video_path.is_some())
            .unwrap();
        assert_eq!(entry.sources.len(), 2);
        assert!(
            (entry.video_offset_seconds - 2.793).abs() < 0.1,
            "log_first={log_first}: {}",
            entry.video_offset_seconds
        );
    }
}
