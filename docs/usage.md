# Race Overlay usage

## Requirements

For beginner-oriented installation and FFmpeg troubleshooting, see
[Install and run](../README.md#install-and-run) in the README.

- Rust 1.95 or newer (required by the current GUI dependencies)
- FFmpeg and FFprobe for video preview, synchronization, and export;
  telemetry-only analysis works without them. Race Overlay checks `PATH` and,
  on macOS, the standard Homebrew locations.

Common installations:

- macOS: `brew install ffmpeg`
- Windows: install an FFmpeg build and add its `bin` directory to `PATH`
- Linux: install the `ffmpeg` package supplied by the distribution

Build and start the editor during development with:

```sh
cargo run --release -p race-overlay
```

On macOS, a bare executable opened from Finder also opens Terminal. Build the
native application bundle for normal release use instead:

```sh
./scripts/build-macos-app.sh
open "target/release/Race Overlay.app"
```

The `.app` launches as a normal GUI application without a Terminal window. The
unbundled executable remains available for debug output and command-line use.

## Analysis: a quick comparison

The app starts in **Analysis**. Nothing requires a saved track or video.

1. Drop one or several XRK files, or use **Add files…**. Each file becomes a
   recording. Full circuit laps are read from logger metadata; out/in segments
   remain available but are not competitive. When an autocross log contains a
   logger finish boundary, the estimated run begins at the sustained launch and
   ends at that boundary; otherwise it receives a best-effort moving interval.
   Nearly stationary logs are marked noncompetitive. These estimates are not
   official timing-system results. When several telemetry files are imported
   together, every interval from those new recordings is selected initially.
2. Check the runs/laps to compare in **Recordings & laps**. **Ref** pins the
   comparison reference. **Select all** and **Deselect all** update every visible
   interval at once. Match and alignment details are shown only beneath selected
   intervals. New imports do not replace a pinned reference.
   When selected runs come from multiple files and no course gates exist, the
   app automatically attempts to align each run's start to the reference.
   Lateral acceleration is correlated in the traveled-distance domain first;
   yaw rate and GPS speed are guarded fallbacks. A source without speed or GPS,
   such as a camera IMU, can instead align on sustained motion onset. Successful
   matches open in **Aligned time**. If a spatial match is not trustworthy, the
   app uses **Traveled distance** when every run supports it, or segment-relative
   time otherwise.
3. A video is optional. Drop one log and one exported video together to pair
   them, or use **Attach video** on the recording. Multiple imported videos
   appear in a pairing selector; filenames are not used to guess synchronization.
   For a separately logged run, select the recording and use the **Video
   alignment** section directly inside that recording's expanded row: add the
   matching camera telemetry and let audio alignment finish. Expand **Camera
   orientation / vehicle axes**, choose a stationary interval and the sensor
   axis pointing toward the vehicle's nose, then apply calibration. This creates
   vehicle-frame lateral/longitudinal acceleration and rotation channels;
   lateral acceleration becomes the preferred logger-to-camera correlation
   pair when both sources provide it. Estimate the match over the complete
   feasible overlap, inspect the signed Pearson result, and apply it. This keeps
   the logger clock stable, so existing intervals and gates remain valid.
4. Click or drag across a plot to scrub all linked panels. **Play linked**
   advances the reference clock, with each video following its own run's
   corresponding point. Video selectors can follow the comparison selection or
   remain pinned to a particular lap. Uncheck **Linked** to inspect a video's
   independent exported-video timestamp. Left/right arrow keys step one video
   frame when frame-rate metadata is available, or one adjacent reference
   telemetry sample in a telemetry-only recording.
5. **Panels / layout** adds channel plots, X/Y scatter plots, delta plots,
   videos, maps, statistics, and setup panels. Drag tab headers to split, tab,
   reorder, or float panels. Try
   **Quick Compare**, **Data Focus**, or **Video Compare** for a fresh layout.
6. Save a `.race-analysis.json` workspace to retain recordings, video offsets,
   intervals, selections, reference, gates, anchors, and panel settings/layout.
   Media remains external. Paths beneath the workspace's folder are saved
   relatively; keep those files together when moving the workspace. Missing
   sources can be repaired with **Locate source file…** in setup. Removing a
   recording never deletes its files.

### Clocks, distance, and synchronization

- **Elapsed time** starts at zero for each selected interval. With multiple
  files and no course gates, **Aligned time** first compares lateral acceleration
  against traveled distance, so a faster or slower run does not get aligned by
  its middle. Yaw rate and speed are accepted only as confident spatial
  fallbacks. Spatial peaks near the search boundary or too far from the run
  start are rejected. Speedless sources fall back to an orientation-independent
  sustained-motion onset computed from three-axis acceleration or gyro data.
  The browser reports the spatial and time shifts plus signed Pearson coefficient
  for correlation matches; onset matches report their time shift. Source and
  video timestamps are not changed. Treat every result as a best-effort estimate.
- **Course position** is the default when the reference has GPS. One reference
  lap defines the course; other runs are projected onto that route with forward
  continuity and heading constraints. Videos and cursors follow the corresponding
  position, not equal recording timestamps. With no reference GPS, this mode
  falls back to elapsed time.
- **Traveled distance** uses each run's own distance channel, speed integration,
  or GPS distance, in that order. Different driving lines can accumulate different
  distances, so this is not the same as comparing identical course locations.
- **Time gain / loss** shows candidate elapsed time minus reference elapsed time
  at matched course positions: negative is ahead, positive is behind. When GPS
  gates have been applied, each curve is clipped to the gate-defined run and
  rebased to zero at the first jointly covered position after the start gate.
  `delta_time` is also available in an ordinary channel plot, so it can be
  stacked with other selected series.

The toolbar always displays **Reference elapsed s**, even when plots use meters.
Video labels show exported-video seconds. In **Timing / course**, the convention
is `video time = recording time + video offset`; source offsets are
`source time − recording time`. These controls accept signed values. Use
**Match one visible event** for a direct video/log offset. Automatic audio and
channel-correlation synchronization, including signed correlation coefficients,
remain available through **Edit overlay / advanced sync**. Returning to Analysis updates
only that recording. The Overlay editor still defines `0.0 s` as the first
exported-video frame.

GPS matching is an estimate, particularly around intersecting routes, poor GPS,
and different start locations. The browser reports matched-sample coverage, not
a statistical confidence score. Missing matches are gaps, not fabricated
interpolated values. Inspect the actual GPS view and, if needed, add a
run-specific **Manual course-position anchor** in setup. Course comparisons
require runs on the same route. Sparse GPS and long outages reduce usable
coverage; traveled-distance integration omits unobserved travel across outages.

### Intervals, gates, and channels

In **Timing / course**, refine each interval with signed numeric start/finish
controls or drag its timeline endpoints. Optionally capture a directed GPS gate
at the reference playhead: one gate extracts circuit crossing-to-crossing laps,
while start and finish gates extract point-to-point runs. Set gate width and
direction, then **Apply gates to all recordings**. Recordings without valid
crossings retain their existing intervals, and the status line reports matched
and unmatched recording counts. Continuous plot channels are interpolated at
the exact resulting interval boundaries rather than beginning at the next sensor
sample. The capture controls identify the pinned recording and interval
explicitly so a visible non-reference video cannot be mistaken for the gate
source. Applying gates resets the reference playhead and analysis range.
**Restore detected intervals** removes the gate-created run
windows without changing the gate coordinates, which makes it possible to seek
outside a bad window and recapture a gate. **Clear gates & restore intervals**
does both. Gate application and re-detection preserve segment identity by
matching time overlap and retain the existing selection whenever those
intervals still exist; they do not automatically add the final run. Thus the
pinned reference does not silently jump to another file.

Plots can contain any selected channels; each channel has its own vertical
scale, with shared horizontal zoom across plots. Same-name channels from
different sources remain distinct by recording. Advanced plot bindings resolve
different channel names across files. Units are converted, never just relabeled;
plot and map units can override the app's Metric/Imperial default. Pinch and
wheel zoom affect only the shared horizontal comparison axis; dragging either
axis scales that axis directly, with vertical scale remaining independent per
plot. Double-click resets a plot. Plot controls and legends can be collapsed, legends can be hidden,
and each run's legend text can be edited from the controls or the plot's
right-click menu. X/Y scatter panels select independent X and Y channels and can
optionally color samples by a third channel using a shared Z color scale.

Source filtering is staged until **Reload & apply**. Each plot and channel map
can also enable its own low-pass filter. All filters are optional, zero-phase,
non-causal, and operate in time before distance resampling. Discrete channels
are not smoothed. GPS course geometry uses original positions rather than
source-smoothed coordinates. **Range start / Range finish** define a common
reference range for the statistics panel; unavailable matched ranges are shown
as unavailable, not replaced silently by whole-run statistics.

CSV imports default to a header row and first-column seconds. For typed channels,
use **CSV columns / units (advanced)** in setup. For example:

```json
{
  "header_row": 0,
  "time_column": "time",
  "time_unit": "seconds",
  "columns": [
    {"column": "speed_kph", "name": "gps_speed", "quantity": "speed", "unit": "kilometer_per_hour"},
    {"column": "lat", "name": "gps_latitude", "unit": "degree"},
    {"column": "lon", "name": "gps_longitude", "unit": "degree"}
  ]
}
```

### Channel maps and imagery

**Channel course map** colors every run along the same reference geometry.
**Split ribbon** divides one course line lengthwise in comparison order: with
two runs, run 1 is the travel-relative left half and run 2 is the right half.
Additional runs become adjacent left-to-right strips. A map can use either a
shared channel color scale, making values directly comparable at each course
position, or a solid color for each run with a run legend. Each
course or GPS map panel has its own **Color range** control. Leave **Auto** on
to use the displayed runs' minimum and maximum, or turn it off and enter labeled
minimum and maximum values. Turn the split off to overlay traces instead. Choose
any numeric channel, an optional display filter, unit, and manual color range.
**GPS imagery** instead draws actual recorded positions. A single **Map controls**
row expands the data, imagery, view, and scale settings; when collapsed, the
scale or run legend is drawn over the map instead of reserving header space.
Click a trace to scrub, left-drag to pan, and use the wheel or pinch gesture to
zoom about the pointer. Right-click a map for the most common view/color
controls, and use **Fit** to reset.

For a continental-US course, use the imagery panel's USGS download action to
save an aerial image, geographic bounds, and attribution in a local assets
folder. An internet connection is required only for the download; saved images
work offline. Before a workspace is saved, downloads use Race Overlay's writable
per-user application-data folder; saved workspaces use an adjacent `.assets`
folder. Existing map images can be imported as PNG/JPEG and registered
using geographic bounds or three non-collinear image/GPS control points. Bounds
use a north-up Web Mercator image; control points support rotated images.
Geographic registration is independent of the simplified course map. Downloads
are limited to course-sized areas (30 km across), not whole regions. Local
imagery and telemetry are not embedded in the workspace JSON.

Video viewers use persistent independent decoders and bounded frame caches,
targeting up to 30 fps (or the source rate if lower). Hidden viewers are released.
Actual playback depends on video resolution, codec, hardware, and viewer count;
four high-resolution streams may not sustain 30 fps. Comparison playback is
muted. Original source audio is still preserved in Overlay exports.

## Overlay workflow

Select **Overlay** in the mode bar for the original editor, or choose
**Edit overlay / advanced sync** on an analysis recording to edit that recording's project.

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
   source and click **Estimate correlation**. The estimate reports the signed
   Pearson `r`, overlap,
   sample count, raw lag, adjustment, and proposed target offset without
   changing the project. Inspect the result, then use **Apply candidate
   offset**. The default searches every lag with sufficient real overlap;
   disable **Search all feasible overlap** to enter a narrower signed adjustment
   window. Enable absolute-correlation matching when equivalent sensors have
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
unlike camera telemetry with embedded audio, an XRK has no audio track to
synchronize automatically. The optional channel-correlation tool can instead
estimate its offset from a matching camera, CSV, synthetic, or other logger
trace. It searches the complete feasible overlap by default; turn that option
off to use a narrower adjustment window around the current offset. Every XRK
channel can be inspected under **Plot channels**.

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
