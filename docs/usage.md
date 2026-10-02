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

## The interface

One header row holds the **Analysis / Overlay** switch, the commands for the
current workspace, the name of the document being edited (when there is room;
a dot marks unsaved changes), and a ⚙ menu with the **Metric / Imperial**
default, the color **Theme**, keyboard shortcuts, and About. On macOS the header
is also the window's title bar; Windows and Linux keep their native title bar.
macOS also gets the system menu bar at the top of the screen (**File**, **View**,
**Panels**, **Window**, **Help**) with the same commands and shortcuts:
⌘O add files, ⇧⌘O open workspace, ⌥⌘O open video, ⌘S / ⇧⌘S save, ⌘1 / ⌘2 switch
workspace, plus theme and unit choices under **View**. Windows and Linux use the
in-window header for these commands. One status bar at
the bottom reports messages, background work, export progress, and any issues
(click the issue count to read and dismiss them).

Keyboard shortcuts: **Space** plays or pauses, **◀ ▶** steps one video frame or
telemetry sample, and **⌘/Ctrl S** saves the current workspace or project.
Dropping files anywhere in the window imports them.

Choose a color scheme (**Race Dark**, **Graphite**, or **Daylight**) in the ⚙
menu; the choice is remembered. Menus that hold several settings (such as a plot's
**Options**) stay open while you adjust them and close when you click outside.
Daylight uses deeper run colors to keep traces readable on its light surfaces.

## Analysis: a quick comparison

The app starts in **Analysis**. Nothing requires a saved track or video.

1. Drop one or several XRK files, or use **Add files…**. Each file becomes a
   recording. Full circuit laps are read from logger metadata; out/in segments
   remain available but are not competitive. When an autocross log contains a
   logger finish boundary, the estimated run begins 3 s before the sustained
   launch, so the start line is inside it, and ends at that boundary; otherwise it receives a best-effort moving interval.
   Nearly stationary logs are marked noncompetitive. These estimates are not
   official timing-system results. When several telemetry files are imported
   together, every interval from those new recordings is selected initially.
   The first import uses **Data Focus** when it has no video, leaving the space
   for telemetry. An import with video uses **Quick Compare**. Later imports
   and saved workspaces preserve your layout; add a video panel through
   **Panels** or choose a layout preset when you want one.
2. Check the runs/laps to compare in **Recordings & laps**. Each recording is a
   card; a colored dot beside a checked lap is that run's color in plots and
   maps. The star pins the comparison reference. **Select all** and **Clear**
   update every visible interval at once. **est.** marks an estimated interval,
   and **check** flags a lap whose course match or time alignment needs
   attention; hover a checked lap for the full details. Match and alignment details are shown only beneath selected
   intervals. New imports do not replace a pinned reference.
   Lap rows stay on one line; hover a shortened name to read it in full.
   Runs that cross the start gate are aligned at it. Otherwise, when selected
   runs come from multiple files, the app automatically attempts to align each
   run's start to the reference.
   Standing starts are aligned where each run has traveled 3 m from rest;
   traveled distance comes from speed and, unlike GPS position, does not drift
   between runs. Otherwise lateral acceleration is correlated in the
   traveled-distance domain;
   yaw rate and GPS speed are guarded fallbacks. A source without speed or GPS,
   such as a camera IMU, can instead align on sustained motion onset. Successful
   matches open in **Aligned time**. If a spatial match is not trustworthy, the
   app uses **Traveled distance** when every run supports it, or segment-relative
   time otherwise.
3. A video is optional. Drop one log and one exported video together to pair
   them, or use **Attach video…** on the recording. Multiple imported videos
   appear in a pairing selector; filenames are not used to guess synchronization.
   A camera recording (INSV) can also come first. Its entry shows the video and
   the camera's own telemetry, with the camera assumed to face the direction of
   travel and levelled from its quietest stationary moment; if that is wrong,
   open **Camera orientation** under **Video alignment**, change the forward
   axis, trims, or interval, and apply it again. The vehicle-frame
   lateral/longitudinal acceleration and rotation channels this creates are
   what a logger is correlated against.
   To compare that video with a logger, add the log to the same entry in any of
   these ways: drop or **Add files…** a log recorded during the video (a log
   whose session date and time overlap the camera file's, read from the INSV
   file name and the logger's metadata, joins the video's entry on its own,
   whichever was added first); **More ▸ Add data log…** on the entry; or
   **Add data log…** in its **Video alignment**. The log becomes the entry's
   clock, so its laps and gates stay valid, and the camera is synchronized to it
   automatically once the camera audio match and orientation are done. A match
   with Pearson r of at least 0.5 over at least 30 s of overlap is applied for
   you; a weaker one is left as a candidate to review. Logs the date and time
   cannot place, or that overlap more than one video, stay separate entries.
   Device clocks can disagree by a minute or more, so the date and time only
   decide pairing; the offset itself always comes from the signals. In
   **Video alignment** you can pick other channels and **Estimate logger ↔
   camera alignment** again, apply the candidate, or **Set the offset by hand**
   (video time = logger time + offset; the camera's telemetry moves with it).
   For a separately logged run, select the recording and use the **Video
   alignment** section directly inside that recording's expanded row: add the
   matching camera telemetry and let audio alignment finish. Expand **Camera
   orientation**, choose a stationary interval and the sensor
   axis pointing toward the vehicle's nose, then apply calibration. Estimate the
   match over the complete feasible overlap, inspect the signed Pearson result,
   and apply it. This keeps the logger clock stable, so existing intervals and
   gates remain valid. Before the first video frame (telemetry often starts a
   fraction of a second earlier) the video pane holds the first frame and says
   how long before the video starts that position is.
4. Click or drag across a plot to scrub all linked panels. the play button in the transport bar at the bottom (or **Space**)
   advances the reference clock, with each video following its own run's
   corresponding point. Video selectors can follow the comparison selection or
   remain pinned to a particular lap. Uncheck **Linked** to inspect a video's
   independent exported-video timestamp. Left/right arrow keys step one video
   frame when frame-rate metadata is available, or one adjacent reference
   telemetry sample in a telemetry-only recording.
5. The **Panels** menu adds channel plots, X/Y scatter plots, delta plots,
   videos, maps, statistics, and setup panels. Drag tab headers to split, tab,
   reorder, or float panels. Choose
   **Quick Compare**, **Data Focus**, or **Video Compare** under **Layout
   presets** for a fresh layout: recordings on the left, plots in the middle, and
   maps and values on the right.
6. Save a `.race-analysis.json` workspace to retain recordings, video offsets,
   intervals, selections, reference, gates, anchors, and panel settings/layout.
   Media remains external. Paths beneath the workspace's folder are saved
   relatively; keep those files together when moving the workspace. Missing
   sources can be repaired with **Locate source file…** in setup. Removing a
   recording never deletes its files.

### Clocks, distance, and synchronization

- **Elapsed time** starts at zero for each selected interval. With multiple
  files and no start-gate crossing, **Aligned time** times standing starts 3 m
  from rest. Other runs compare lateral acceleration
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
  at matched course positions: negative is ahead, positive is behind. When both
  runs cross the start gate, elapsed time is measured from it and the curve is
  zero at the gate.
  `delta_time` is also available in an ordinary channel plot, so it can be
  stacked with other selected series.

The transport bar always displays the reference elapsed time, even when plots use meters.
Video labels show exported-video seconds. In **Timing & course setup**, the convention
is `video time = recording time + video offset`; source offsets are
`source time − recording time`. These controls accept signed values. Use
**Match one visible event** for a direct video/log offset. Automatic audio and
channel-correlation synchronization, including signed correlation coefficients,
remain available through **Edit overlay** on the selected recording. Returning to Analysis updates
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

In **Timing & course setup**, whose **Intervals**, **Sources & sync**, and
**Course gates** tabs group these controls, refine each interval with signed numeric start/finish
controls or drag its timeline endpoints.

On **Course gates**, scrub the reference to a timing line and capture it as the
start or finish gate; each gate has a position, travel direction, and width.
Gates never trim intervals. Every compared run that crosses the start gate is
time-aligned at that crossing, including runs added later, and charts mark the
reference's start and finish crossings with dashed lines. Each trace is dimmed
before its own start-gate crossing and after its finish-gate crossing, but all
of its data stays visible. A run that does not cross the start gate inside its
interval is flagged and falls back to best-effort alignment. With a single
circuit gate, **Split laps at gate** replaces every recording's intervals with
laps between consecutive crossings; recordings without two crossings keep
their intervals. **Restore detected intervals** re-runs logger-lap or motion
detection. **Playhead + n s** captures before or after the playhead (up to
120 s, only where the reference has GPS), for a gate just outside the interval.
**Correct GPS drift** (off by default) shifts each compared run's GPS onto the
reference, because consumer GPS positions drift by metres between runs while
staying steady within one. Standing starts use their shared staging spot;
other runs fit their whole path onto the reference's, which needs a path that
turns, not a straight line. The correction applies to maps, course matching,
and gate crossings; raw data and lap splitting are unchanged. Each run's notes
show the shift and method, or say when no estimate was found.
Splitting and re-detection preserve segment identity by matching time overlap
and keep the existing selection whenever those intervals still exist, so the
pinned reference does not silently jump to another file.

Plots can contain any selected channels; each channel has its own vertical
scale, with shared horizontal zoom across plots. Same-name channels from
different sources remain distinct by recording. Advanced plot bindings resolve
different channel names across files. Units are converted, never just relabeled;
plot and map units can override the app's Metric/Imperial default. Pinch and
wheel zoom affect only the shared horizontal comparison axis; dragging either
axis scales that axis directly, with vertical scale remaining independent per
plot. Double-click resets a plot. Long recordings are drawn at screen resolution
(each pixel column keeps its minimum and maximum, so peaks stay visible) and zooming in
shows the underlying samples. A plot can list a channel no loaded source has (the default
plot lists GPS speed and RPM); **Channels** still shows it so it can be unticked, and the
empty row has **Remove from plot**. Each plot's header has **Channels** and
**Options** menus (smoothing, units and channel matching, legend); stacked plots
show one shared legend. A legend that would cover too much of the plot folds
into **Legend (N)** in its corner; open it to scroll the runs and toggle traces.
Legends can be hidden,
and each run's legend text can be edited from the controls or the plot's
right-click menu. X/Y scatter panels select independent X and Y channels and can
optionally color samples by a third channel using a shared Z color scale.

Source filtering is staged until **Reload & apply**. Each plot and channel map
can also enable its own low-pass filter. All filters are optional, zero-phase,
non-causal, and operate in time before distance resampling. Discrete channels
are not smoothed. GPS course geometry uses original positions rather than
source-smoothed coordinates. The **Stats range** menu in the transport bar (**Start here**, **Finish here**,
**Clear**) defines a common
reference range for the statistics panel; unavailable matched ranges are shown
as unavailable, not replaced silently by whole-run statistics.
Statistics columns are resizable: drag a header divider, or double-click it to
reset that column. Widths are shared across the run cards in that panel and saved
with the workspace. Narrow cells stay on one line; hover to read a full value.

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
**GPS imagery** instead draws actual recorded positions. The header switches
between **Channel** and **Runs** coloring and picks the channel; its **Options**
menu holds smoothing, units, layout, rotation, color range, and imagery. The
color scale is drawn top-right and the run legend bottom-left, over the map.
Click a trace to scrub, left-drag to pan, and use the wheel or pinch gesture to
zoom about the pointer. Right-click a map for the most common view/color
controls, and use **Fit** to reset.

On a **GPS imagery** map, **Get aerial image** supplies a course background
from the first imagery source that covers the course. USGS National Map
imagery (continental US) is built in as the fallback. Many state and county GIS
offices publish sharper, more recent orthoimagery as ArcGIS services.
**Find imagery…** searches Esri's public ArcGIS Online catalog for them near the
course and lists, in Settings, only services that actually return imagery of the
course: each is asked to describe itself and to draw a small sample, so dead
links, sign-in-only services, infrared layers, and layers without imagery
there are hidden. Publishers rarely register every year they fly, so the other
services in the same server folder as a match (for example New York's other
flight years) are checked too. Results are grouped as local, county,
statewide, or national, local first, and within a group the most recent year
in the name comes first ("latest" counts as newest); **Add** keeps one. The search sends the catalog the course area rounded outward to
0.1°, and only when you click the button. A service the catalog lacks can be
added by pasting its link under **Settings ▸ Your imagery sources** (the ⚙
menu, or **Race Overlay ▸ Settings…** on macOS): the service page, its export
address, a layer, or the WMS link ArcGIS publishes beside it, for example New
York's `https://orthos.its.ny.gov/arcgis/rest/services/wms/Latest/MapServer`.
Both drawn-on-request services and hosted tile layers (stitched from Web
Mercator tiles) work. Race Overlay remembers each source's name, coverage,
attribution, and size limit for every workspace. Sources are tried in list
order and can be renamed, switched off, reordered, re-linked, or removed. A
source whose coverage cannot be read (an unusual projection) is tried for every
course. An image covers the course at up to 4096 pixels across.

Race Overlay first looks in its per-user imagery cache and reuses any saved
image from the chosen source that covers the course, so the same venue is
downloaded only once, even across workspaces. Only a miss contacts the network.
The image (with its geographic bounds and attribution) is also copied beside the
workspace, in an adjacent `.assets` folder once saved, so saved workspaces do
not depend on the cache. **Re-download** ignores the cache and fetches a fresh
copy. The cache lives in the application-data folder
(`~/Library/Application Support/Race Overlay/Imagery Cache` on macOS,
`%LOCALAPPDATA%\Race Overlay\Imagery Cache` on Windows, and
`$XDG_DATA_HOME/race-overlay/Imagery Cache` or `~/.local/share/race-overlay/Imagery Cache`
on Linux); delete it at any time to reclaim space. Existing map images can be
imported as PNG/JPEG and registered using geographic bounds or three
non-collinear image/GPS control points. Bounds use a north-up Web Mercator image;
control points support rotated images. Geographic registration is independent of
the simplified course map. Downloads are limited to course-sized areas (30 km
across), not whole regions. A GPS imagery map overlays all selected runs by
default and fits itself to the reference run's venue; selected runs recorded
elsewhere (more than about 5 km away) are hidden with a note. The background
image loads automatically whenever the map is visible.
Local
imagery and telemetry are not embedded in the workspace JSON.

Video viewers use persistent independent decoders and bounded frame caches,
targeting up to 30 fps (or the source rate if lower). Hidden viewers are released.
Actual playback depends on video resolution, codec, hardware, and viewer count;
four high-resolution streams may not sustain 30 fps. Comparison playback is
muted. Original source audio is still preserved in Overlay exports.

## Overlay workflow

Select **Overlay** in the mode bar for the original editor, or choose
**Edit overlay / advanced sync** on an analysis recording to edit that recording's project.

1. Create a project and select a stitched/reframed video, or a standard SDR
   X4 Air dual-track INSV for direct processing (see below).
2. Add one or more telemetry sources. For an Insta360 recording, select the
   matching LRV when available; it is much smaller than the INSV and contains
   the same sensor trailer. For an AiM logger, choose **Add ▸ MyChron XRK** in the Data sources panel and select
   the original uncompressed `.xrk` file.
   Select a source and use **Remove source…** at the bottom of its settings to remove it. Removal clears that
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
5. Add individual widgets, or use **Add ▸ MyChron dashboard** in the Widgets panel to append a complete
   pre-arranged set without removing anything already on the canvas. The preset
   binds RPM/shift lights, speed, gear, G, water temperature, EGT, lap time,
   delta, and steering when those channels are available. Widgets can be bound
   to any source channel, dragged, and resized directly over the preview.
   Foreground opacity controls the data, labels, ticks, and lamps; background
   opacity independently controls the rounded panel behind the widget.
   Choose the default **Metric** or **Imperial** unit system in the ⚙ menu.
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

The XRK importer validates message framing and retains diagnostic counts for
packets emitted by the logger without a matching channel definition. These
hidden/internal packets are not presented as named telemetry because the file
supplies neither their labels nor calibration.

## Project files

Projects use the `.race-overlay.json` suffix. They store media paths, layout,
bindings, source calibration and synchronization, but do not copy the source
media.

## Direct X4 Air video

Standard SDR X4 Air dual-track INSV recordings can be imported directly. Both
fisheye video tracks are projected into one 16:9 view. Other camera models,
paired-file layouts, HDR/log video, speed ramps, automatic segment joining, and
automatic LRV video proxies are not supported by this path. Unsupported or
incomplete optical metadata produces an error.

Importing an INSV adds its video and camera telemetry as one recording. Attaching
one to an existing recording preserves its other sources and selected primary
source. Matching camera telemetry is reused rather than added again.

In an Analysis video pane, drag the image with the primary mouse button to aim and
scroll over the image to change horizontal FOV. These controls work while paused
or playing and do not change the linked clock, selected lap, or alignment mode.
FOV ranges from 30° to 150°, starting at 90°. The controls above the image also
provide roll, reset, seam mode, stabilization, and export resolution. Panes showing
the same recording share the settings; different recordings remain independent.
Settings are saved in the workspace and carried into that recording's Overlay
project. In Overlay, enable **Reframe video** to aim/zoom; leave it disabled to
position widgets.

**Hard cut** is the default seam. **Feather** blends across a narrow transition,
initially 2°, within the valid lens overlap. Nearby objects can jump at a hard cut
or appear doubled with feathering because the two lenses see them from different
positions. Uncovered pixels remain black. Advanced stitching is currently
unavailable; its prototype and acceptance status are documented in
[raw-video validation](raw-video.md).

**Stabilize** is off by default. It smooths camera vibration while following slower
turns and banking. The smoothing control is Gaussian standard deviation in seconds
(default 0.25 s), using a symmetric ±3σ window. This is camera-motion smoothing;
it does not lock the horizon or world direction. It uses the original optical-rig
gyro data separately from vehicle calibration and telemetry filters, and does not
alter plotted or exported telemetry. Preparation happens in a background worker.
Missing gyro coverage is shown as unavailable. Stabilized export requires valid
motion coverage over the video; disable stabilization to export without it.

Raw export defaults to **1080p (1920 × 1080)**, with **2160p (3840 × 2160)** available.
It uses the same view, seam, and stabilization policy as preview, draws overlays
after projection, and copies the original AAC audio. An intermediate stitched
video is not needed. Preview decoding uses reduced-resolution lens frames;
export decodes the original lens resolution. Video time starts at the first frame.
