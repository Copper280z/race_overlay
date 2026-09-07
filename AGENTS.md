# Race Overlay contributor guide

Race Overlay is a Rust desktop application for comparing racing telemetry and
burning configurable overlays into onboard video. Analysis and Overlay are
complementary workflows over the same recordings and project data. Camera video
is expected to have already been stitched or reframed; this project does not
process raw dual-lens footage.

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
  - `analysis_app.rs`: Analysis state, preparation, and shared coordination.
  - `analysis_views.rs`, `analysis_workflow.rs`: plots, setup, and workspace actions.
  - `analysis_recordings.rs`: recording rows and the actions they emit.
  - `analysis_sync/`: recording-scoped audio sync, camera calibration,
    logger/camera correlation, persistence compatibility, and presentation.
  - `analysis_maps.rs`, `analysis_imagery.rs`: map interaction and registration.
  - `race_app.rs`: thin Overlay application root.
  - `race_app/`: Overlay controllers, lifecycle, policy, panels, preview, export,
    and tests. Put pure decisions in `*_policy.rs`; keep I/O in the owning
    controller/module.
- `crates/overlay-core`: shared models and algorithms. `telemetry.rs` defines
  channels/units, `project.rs` persisted projects, `analysis.rs` comparison
  geometry/timing, `processing.rs` filtering, `calibration.rs` IMU calibration,
  and `adapters.rs`/`xrk.rs` data ingestion.
- `crates/overlay-render`: widget preparation and RGBA rendering; no UI or media
  orchestration.
- `crates/overlay-media`: FFmpeg discovery, probing, preview, synchronization,
  audio, and final export.
- `docs/usage.md`: user-visible behavior. `README.md`: installation and project
  introduction. `scripts/` and `packaging/`: release packaging.
- `mychron_data`: optional local recordings for manual XRK validation; never a
  routine test dependency or committed fixture.

Dependency direction is `app` → media/render/core and `overlay-render` →
`overlay-core`; no library crate couples back to `app`. When a coordinator grows,
extract a cohesive module with a narrow interface instead of creating a generic
dumping-ground utility module.

## Working agreements

- Prefer deterministic synthetic fixtures. Tests needing local recordings,
  network access, or FFmpeg should remain explicit and ignored by default.
- Add regression tests near the owning module. Test pure policy independently
  from UI rendering or external processes when possible.
- Update `docs/usage.md` for visible behavior and `README.md` for setup or scope.
- Do not commit `target/`, recordings, videos, exports, or local project files.
- Before publication, run:

  ```sh
  cargo fmt --all --check
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo build --release -p race-overlay
  ```
