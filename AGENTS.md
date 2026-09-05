# Race Overlay contributor guide

## Project overview

Race Overlay is a cross-platform Rust desktop editor that synchronizes racing
telemetry with onboard video and burns configurable data widgets into an
exported video. The source video is expected to have already been stitched or
reframed by Insta360 Studio; this project does not implement raw dual-lens
stitching.

The workspace is divided into:

- `app`: the `eframe`/`egui` desktop editor and user workflows.
- `crates/overlay-core`: telemetry models, projects, calibration, filtering,
  correlation, and Insta360/CSV/synthetic/AiM XRK adapters.
- `crates/overlay-render`: widget conversion and RGBA overlay rendering.
- `crates/overlay-media`: FFmpeg discovery, preview, audio synchronization,
  probing, and final video export.
- `mychron_data`: local XRK recordings used while reverse-engineering and
  manually validating the AiM importer; these are not committed by default.
- `docs/usage.md`: the user-facing workflow and behavior reference.

## Product goals and required behavior

- Normal UI time is exported-video time: `0.0 s` means the first frame of the
  Insta360 Studio export. Raw-source timing is an explicit advanced override,
  and time/range controls may accept negative values where relevant.
- Sources include Insta360 INSV/LRV telemetry, AiM MyChron XRK logs, generic
  CSV, and deterministic synthetic data. Sources can be added and removed
  without disturbing unrelated sources or widget layout.
- XRK parsing is reverse-engineered. Preserve unknown records safely while
  extracting all well-supported channels, including GPS, lap boundaries, RPM,
  EGT, water temperature, accelerometers, gyros, steering, gear, and voltages.
- Camera calibration removes stationary gravity and supports axis flips plus
  fine roll, pitch, and yaw trim. Derived longitudinal/lateral/vertical G must
  remain in physically meaningful units and orientation.
- Every low-pass filter is optional, zero-phase, and non-causal. Source-level,
  derived-IMU, graph-preview, and per-widget filters have distinct scopes; do
  not silently turn any of them into a causal real-time filter.
- Non-camera source alignment can correlate any suitable channel against any
  other source/channel. Correlation ranges are capped to feasible overlap, and
  the UI reports the signed Pearson coefficient before applying an offset.
- The default unit system is configurable, and compatible units can be
  overridden per widget without merely relabeling unconverted values.
- Widget foreground opacity and background opacity are independent settings.
- Appearance is project-level and saved with the project. It provides Race
  Dark, Light, Transparent, and Custom presets plus semantic accent, text,
  panel, muted, positive, warning, and critical colors. Widgets inherit the
  project palette by default and may opt into per-widget color overrides.
  Corner roundness follows the same global/inherited or per-widget override
  model.
  Color alpha is not used as a second opacity control: RGB colors and
  foreground/background opacity remain separate. Preview and export must use
  the same resolved palette.
- GPS track maps draw one representative path for multi-lap recordings rather
  than stacking every lap. They also support point-to-point/autocross routes,
  playhead-captured start and finish locations, manual full-lap selection,
  current-position display, rotation, padding, and GPS dropouts.
- Video export defaults to matching the source video. macOS-compatible HEVC
  uses the `hvc1` tag, YUV420 output, and a fast-start MP4 index; H.264 remains
  available as the compatibility option. Preserve source audio.
- Preview and export should avoid repeated full-series work per frame. Prepare
  static geometry and cache filtered series when rendering a complete export.

## Development practices

- Keep project-file compatibility: known widget/source settings live in the
  existing JSON fields, and unknown fields must survive a load/save round trip.
- Use the shared telemetry model and project bindings rather than introducing
  source-specific paths in widgets.
- Validate changes proportionally. The normal pre-commit checks are:

  ```sh
  cargo fmt --all --check
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo build --release -p race-overlay
  ```

- Tests that require supplied recordings should remain optional/ignored.
  Prefer deterministic synthetic fixtures for routine tests; local XRK files
  may be used for manual real-data validation when available.
- Do not add generated `target` contents, local `.race-overlay.json` projects,
  XRK logs, raw camera recordings, or exported videos to Git. The root
  `.gitignore` deliberately excludes these data and media formats.
- Update `README.md` or `docs/usage.md` when behavior visible to users changes.
