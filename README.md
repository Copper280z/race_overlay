# Race Overlay

A cross-platform Rust desktop application for comparing motorsport telemetry
and burning synchronized data overlays into video. It reads the IMU trailer in Insta360 INSV/LRV files,
AiM MyChron XRK logs, generic timestamped CSV, or deterministic synthetic race
data.

The new **Analysis** workspace opens without a video or saved track. Drop XRK
files to compare circuit laps or separate autocross runs, pin a reference, and
inspect linked plots, independent video viewers, channel-colored course maps,
and actual GPS over locally saved imagery. Plot against elapsed time, matched
course position, or each run's traveled distance. Panels can be dragged into
tabs, split docks, or floating windows; three layout presets provide quick
starting points. Analysis workspaces use `.race-analysis.json` files.

**Overlay** retains the original editor and export workflow. Use a recording's
**Edit overlay / sync** action to calibrate or synchronize its sources, customize
its dashboard, or export; switching back carries the changes into that recording
without replacing the other comparisons.

The editor provides numeric, bar, speed gauge, GPS track map, G-meter, tachometer, temperature,
lap timer, delta, shift-light, steering/input, and gear widgets. An additive
MyChron dashboard preset lays out and binds a complete race display in one
click. Widgets can be bound to channels from any loaded source, dragged and
resized over a muted video preview, styled with a project-wide appearance
palette plus per-widget color overrides and independent foreground and
background opacity, converted between metric and imperial display units, and
exported to MP4 through FFmpeg while retaining the source audio stream. Optional
zero-phase, non-causal source, derived-G, graph-preview, and widget low-pass
filters complement the multi-channel graph and camera calibration tools.
MyChron, CSV, and synthetic sources can estimate
their time offset by correlating any of their channels with a channel from any
other loaded source, reporting the signed Pearson coefficient before the user
applies the candidate. Sources can be removed without disturbing the
video, layout, or other telemetry. GPS track maps support multi-lap circuits
without drawing duplicate lap paths and point-to-point autocross courses with
user-captured start and finish markers. Together these tools make camera-axis
identification and IMU calibration inspectable in the editor.
Insta360 telemetry is audio-aligned automatically, so every normal control uses
the exported video's timeline.

See [docs/usage.md](docs/usage.md) for setup and the end-to-end workflow.
