# Race Overlay contributor guide

Race Overlay is a Rust desktop application for comparing racing telemetry and
burning configurable overlays into onboard video. Analysis and Overlay are
complementary workflows over the same recordings and project data. Camera video
can be an ordinary stitched/reframed video or standard SDR X4 Air
dual-track INSV. Raw projection and optional gyro stabilization share a pipeline
across live Analysis, Overlay preview, and export. Advanced stitching remains
subject to the live-preview release gate described in `docs/raw-video.md`.

## Design principles

- **Keep domain logic below the UI.** The app coordinates workflows; telemetry,
  timing, calibration, filtering, and project semantics belong in
  `overlay-core`. Rendering and FFmpeg operations stay in their own crates.
- **Use one source-agnostic telemetry model.** Importers normalize data into
  shared channels and units. UI and widgets must not grow source-specific paths.
- **Preserve user data.** Adding, editing, or removing one source must not disturb
  unrelated recordings, selections, dashboards, or layouts. Preserve unknown
  JSON fields across load/save for forward compatibility.
- **Make time domains explicit.** UI/video time is exported-video time; source,
  recording, and segment-relative clocks are distinct. Course position and
  traveled distance are also distinct comparison domains. Never extrapolate
  beyond available telemetry or video coverage.
- **Keep processing physically meaningful.** Convert values when changing units.
  Calibration must preserve orientation and units. Low-pass filters are optional,
  zero-phase, non-causal, and scoped explicitly.
- **Keep preview and export consistent.** Both must resolve the same widget,
  appearance, timing, and unit settings. Cache prepared geometry and filtered
  series instead of repeating full-series work per frame.
- **Prefer explicit gaps over plausible fiction.** GPS dropouts, failed course
  matches, missing channels, and uncertain alignment should remain visible to
  callers and users.

## Project organization

- `app`: `eframe`/`egui` application composition and user workflows.
  - `analysis_app.rs`: Analysis composition, navigation, and shared view queries.
  - `analysis_model.rs`: serializable tabs/options and view-runtime models.
  - `analysis_io.rs`: imports, background work, and media attachment coordination.
  - `analysis_views.rs`, `analysis_workflow.rs`: plots, maps, video, and
    statistics panels; workspace open/save and gate actions.
  - `analysis_chrome.rs`: Analysis toolbar, transport bar, and first-run state.
  - `analysis_setup/`: the "Timing & course setup" panel, one module per tab
    (intervals, sources and sync, course gates).
  - `analysis_recordings.rs`: recording rows and the actions they emit.
  - `analysis_sync/`: recording-scoped audio sync, camera calibration,
    logger/camera correlation, log/video pairing, persistence compatibility,
    and presentation.
  - `analysis_decimate.rs`: reduces plot lines to screen resolution per view.
    Prepared series stay complete; only what egui draws is reduced.
  - `analysis_maps.rs`, `analysis_imagery.rs`: map interaction and registration.
  - `imagery_sources.rs`: user-added ArcGIS imagery services (USGS is the
    built-in fallback) and fetching an image from one, drawn or tiled;
    `imagery_search.rs` finds and verifies services near a course in the
    ArcGIS Online catalog; `settings_window.rs` edits them. Sources are
    app-level preferences saved in eframe storage, not workspace data.
  - `mychron/`: MyChron Wi-Fi downloads: the window (session browser and Track
    mode), the background worker (`worker.rs`), pure decisions
    (`policy.rs`), the download library and its ledger (`library.rs`), and
    persisted settings. The service lives on the shell so Track mode keeps
    running in both modes.
  - `ui_kit/`: the shared theme and small widgets (segmented control, cards,
    chips, popovers). Panels take colors from `theme::surface`, `theme::text`,
    `theme::accent()`, and `Tone`, never from literals. A color scheme is one
    `Palette` in `ui_kit/theme/themes/`: copy `race_dark.rs`, register it in
    `themes/mod.rs`, and the contrast tests in `theme/mod.rs` check it.
  - `native_menu/`: the macOS menu bar (`muda`); an inert stand-in elsewhere so
    the shell has no platform conditionals. Do not add a native Edit menu: its
    Cut/Copy/Paste shortcuts would keep egui text fields from receiving them.
  - `race_app.rs`: application shell (header, status bar, shortcuts) and the
    Overlay root.
  - `race_app/`: Overlay controllers, lifecycle, policy, panels, preview, export,
    and tests. Put pure decisions in `*_policy.rs`; keep I/O in the owning
    controller/module.
- `crates/overlay-core`: shared models and algorithms. `telemetry.rs` defines
  channels/units, `project.rs` persisted projects, `analysis.rs` workspace and
  course geometry, `comparison/` preparation and alignment strategies,
  `processing.rs` filtering, `calibration.rs` IMU calibration, and
  `adapters.rs`/`xrk.rs` data ingestion.
- `crates/overlay-render`: widget preparation and RGBA rendering; `video/` owns
  offscreen wgpu/CPU projection and stitching backends. No UI/media orchestration.
- `crates/overlay-logger`: the MyChron6 Wi-Fi protocol client
  (`docs/mychron-protocol.md`), UDP discovery, Wi-Fi control (`wifi/`: nmcli,
  CoreWLAN, inert elsewhere), and a scriptable fake logger behind the `fake`
  feature for tests. Depends on no other workspace crate.
- `crates/overlay-media`: FFmpeg discovery, probing, preview, synchronization,
  audio, and final export. `processed/` pairs raw lens frames by integer PTS and
  accepts a caller-provided processor, without depending on the app or renderer.
- `docs/usage.md`: user-visible behavior. `README.md`: installation and project
  introduction. `scripts/` and `packaging/`: release packaging.
- `mychron_data`: optional local recordings for manual XRK validation; never a
  routine test dependency or committed fixture.

Dependency direction is `app` → media/render/core/logger and `overlay-render` →
`overlay-core`; no library crate couples back to `app`. When a coordinator grows,
extract a cohesive module with a narrow interface instead of creating a generic
dumping-ground utility module.

## Working agreements

- Prefer deterministic synthetic fixtures. Tests needing local recordings,
  network access, or FFmpeg should remain explicit and ignored by default.
- Keep real GPS coordinates out of the repository. Tests, fixtures, examples,
  and docs use made-up positions (for example near 40°N 100°W), never ones
  taken from recordings, downloaded imagery, logger output, or real venues.
- Add regression tests near the owning module. Test pure policy independently
  from UI rendering or external processes when possible.
- Visual changes can be reviewed offscreen: the ignored `render_ui_snapshots`
  test writes PNGs of the real UI (see `app/src/race_app/snapshots.rs`). It
  needs a GPU adapter and the local `mychron_data` recordings, so it is opt-in.
  The bundled fonts lack some symbols (for example `▾ ● ✓ ← →`); prefer the
  glyphs `render_glyph_coverage` shows are drawable, or paint the shape.
- `widgets::popover` is self-managed on purpose. egui allows one open popup
  per window, so a dropdown inside an egui menu closes the menu; use `popover`
  (and `close_popover`) for panels that contain dropdowns.
- An idle window must not repaint. Workers hand results over channels without
  waking the UI, so a panel asks for `request_repaint_after` only while it has
  something outstanding (`AnalysisApp::has_background_work`, a pending video
  frame); never from a constant loop.
- Update `docs/usage.md` for visible behavior and `README.md` for setup or scope.
- Do not commit `target/`, recordings, videos, exports, or local project files.
- Work is not finished until `cargo build --release -p race-overlay`
  succeeds: run it before reporting any change as done, not only before
  publication.
- Before publication, run:

  ```sh
  cargo fmt --all --check
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo build --release -p race-overlay
  ```
