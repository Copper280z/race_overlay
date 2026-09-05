# Race Overlay usage

## Requirements

- Rust 1.85 or newer
- FFmpeg and FFprobe available on `PATH`

Common installations:

- macOS: `brew install ffmpeg`
- Windows: install an FFmpeg build and add its `bin` directory to `PATH`
- Linux: install the `ffmpeg` package supplied by the distribution

Build and start the editor with:

```sh
cargo run --release -p race-overlay
```

## Workflow

1. Create a project and select a stitched or reframed video exported by
   Insta360 Studio. Raw dual-lens stitching is intentionally left to Studio.
2. Add one or more telemetry sources. For an Insta360 recording, select the
   matching LRV when available; it is much smaller than the INSV and contains
   the same sensor trailer. For an AiM logger, use **+ MyChron XRK** and select
   the original uncompressed `.xrk` file.
   Select a source and use **Remove source** to remove it. Removal clears that
   source's plot selections and widget bindings, but leaves the video, widgets,
   and other telemetry sources intact.
3. When an Insta360 source is longer than the Studio export, the editor
   automatically matches their audio and aligns telemetry to the exported-video
   timeline. Thus `0.0 s` always means the first exported frame. Manual raw
   offsets and re-sync controls remain under **Advanced timing** for diagnosis.
4. For camera IMU data, choose a stationary interval, calibrate gravity, and
   select the direction on the camera that points toward the front of the
   vehicle. Fine roll, pitch, and yaw trims correct small mounting or surface
   angle errors. Positive pitch raises the nose, positive roll raises the left
   side, and positive yaw turns toward the left. Set the derived-G low-pass cutoff before applying calibration;
   applying it again updates existing bound channels in place.
   If that interval is moving, the editor automatically uses the quietest
   stationary interval in the complete raw recording. Turn off **Auto-find a
   stationary interval** to force the range you entered. Range values may be
   negative, and **Advanced timing** can make them refer directly to raw source
   time instead of exported-video time.
   For a MyChron, CSV, or synthetic source, expand **Correlate to another
   source**. Choose one of its channels plus a channel from any other loaded
   source, set the signed adjustment search range, and click **Estimate
   correlation**. The estimate reports the signed Pearson `r`, overlap,
   sample count, raw lag, adjustment, and proposed target offset without
   changing the project. Inspect the result, then use **Apply candidate
   offset**. Enable absolute-correlation matching when equivalent sensors have
   opposite signs; the reported coefficient remains signed. Search bounds may
   be wider than either recording: the estimator automatically caps them to
   lags where the selected channels can overlap. Channel selectors show sample
   count and duration and prefer dense continuous sensors over sparse lap
   metadata.
5. Add individual widgets, or use **+ MyChron dashboard** to append a complete
   pre-arranged set without removing anything already on the canvas. The preset
   binds RPM/shift lights, speed, gear, G, water temperature, EGT, lap time,
   delta, and steering when those channels are available. Widgets can be bound
   to any source channel, dragged, and resized directly over the preview.
   Foreground opacity controls the data, labels, ticks, and lamps; background
   opacity independently controls the rounded panel behind the widget.
   Choose the default **Metric** or **Imperial** unit system in the toolbar.
   Bound widgets also have a compatible display-unit selector, so an individual
   speed, temperature, pressure, distance, angle, time, or acceleration widget
   can override the default without relabeling unconverted data.
   Use **Appearance** to choose a project-wide style preset: **Race Dark**,
   **Light**, **Transparent**, or **Custom**. The project palette has semantic
   colors for accent, text, panel, muted, positive, warning, and critical
   elements. A widget inherits that palette by default; turn off **Use global
   appearance** on an individual widget to expose its color overrides. This
   lets, for example, one widget use a different accent or warning color while
   the rest of the dashboard remains consistent. Color controls edit RGB only.
   Foreground opacity (data, labels, ticks, and lamps) and background opacity
   (the rounded panel) remain separate controls, both globally and per widget.
   Corner roundness is also available globally and as a per-widget override.
   Add **Track map** for GPS-equipped recordings. It automatically binds
   latitude and longitude from the same source when available and draws one
   representative path for multi-lap circuit logs. Choose **Auto**, **Circuit**,
   or **Point-to-point** mode in the widget settings; point-to-point maps use
   the optional **Capture** buttons to record start and finish coordinates at
   the current playhead. Lap **0** selects an automatic representative lap;
   positive lap numbers select that recorded full lap. The selected lap, map
   rotation, padding, line width, marker visibility, and captured coordinates
   are saved in the project. GPS channels must remain in
   degrees and come from the same telemetry source.
6. Export a burned-in MP4. **Match source** is the default: it preserves the
   source codec family, resolution, frame rate, approximate video bitrate,
   color metadata, and audio. HEVC is written with Apple's `hvc1` tag, YUV420
   output, and a fast-start MP4 index so the result works with macOS players.
   Choose **H.264 (most compatible)** if the video must also work with older
   hardware or software. A custom bitrate and the compatibility details are
   available in the export dialog.

Camera-button recordings do not contain GPS unless a GPS-capable companion was
used while recording. Gyroscope and accelerometer data can produce honest G and
turn-rate displays, but the editor never invents speed by integrating the
accelerometer.

Insta360 raw acceleration channels are stored internally by Race Overlay as
SI `m/s²`. Binding one directly to the XY G meter automatically applies the
visible `1 / 9.80665` binding scale. Camera-derived longitudinal, lateral,
vertical, and combined channels are already expressed in `g`.

## Data graph and filtering

Select a source, expand **Plot channels**, check any number of channels, and
open **Data graph**. The graph shows the selected exported-video-time window,
units from the selected default system, and min/max values. Source offsets are applied invisibly.
Robust scaling
ignores the most extreme one percent by default so isolated IMU spikes do not
flatten the useful signal.

Every low-pass filter is zero-phase and non-causal. Its displayed cutoff is the
final two-pass -3 dB point, so it does not shift events relative to the video.

- **Graph preview low-pass** is non-destructive: it affects only the graph and
  is useful for choosing a cutoff.
- **Source low-pass** filters every imported continuous channel before any
  camera-derived channels are calculated; held/discrete values such as gear are
  unchanged. Change it and click **Reload & apply source smoothing**; the
  original file is reloaded, so repeated changes never compound.
- **Derived G smoothing** runs during camera calibration and filters only the
  derived longitudinal, lateral, and vertical G channels; combined G is then
  calculated from those components. It does not filter derived turn-rate
  channels.
- **Widget low-pass** filters only the continuous inputs bound to that widget.

Stages can be enabled independently, but filters on the same signal cascade:
for example, source filtering feeds derived-G filtering, and either may then
feed a widget filter. Their settings are stored in the project. Calibration
warns when the selected interval has enough accelerometer or gyroscope motion
that it is unlikely to be stationary.

## Generic CSV

The current editor uses a conventional CSV default: the first row contains
headers, the first column is time in seconds, and every other numeric column is
imported as a channel. The core importer also supports semicolon/tab delimiters,
header and timestamp selection, timestamp units, fixed sample rates, per-column
units, hold/linear interpolation, and gap thresholds through the source settings
stored in project JSON.

## AiM MyChron XRK

XRK import retains the configured logger channels, native units and sample
timing, GPS-derived channels, session metadata, and lap boundaries. The supplied
logs provide RPM, EGT and water temperature, three-axis acceleration and gyro,
steering angle, gear, battery voltages, GPS position/speed/acceleration, and lap
timing. Channel names are normalized for stable project bindings; for example,
`Exhaust Temp` becomes `exhaust_temperature` and `Water Temp` becomes
`water_temperature`.

XRK source time is zeroed at the beginning of the first recorded lap segment.
Use **Advanced timing → Raw source offset** to align it with the exported video;
unlike the Insta360 source, an XRK has no audio track to synchronize
automatically. The optional channel-correlation tool can instead estimate its
offset from a matching camera, CSV, synthetic, or other logger trace. Its
search range is an adjustment around the XRK's current offset and accepts
negative values. Every XRK channel can be inspected under **Plot channels**.

For a terminal summary of one or more logs:

```sh
cargo run -p overlay-core --example xrk_inspect -- path/to/session.xrk
```

XRK is a reverse-engineered format. The importer validates message framing and
retains diagnostic counts for packets emitted by the logger without a matching
channel definition. These hidden/internal packets are not presented as named
telemetry because the file supplies neither their labels nor calibration.

## Project files

Projects use the `.race-overlay.json` suffix. They store media paths, layout,
bindings, source calibration and synchronization, but do not copy the source
media.
