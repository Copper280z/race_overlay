# Race Overlay

A cross-platform Rust desktop editor for burning synchronized motorsport
telemetry into video. It reads the IMU trailer in Insta360 INSV/LRV files,
AiM MyChron XRK logs, generic timestamped CSV, or deterministic synthetic race
data.

The editor provides numeric, bar, speed gauge, GPS track map, G-meter, tachometer, temperature,
lap timer, delta, shift-light, steering/input, and gear widgets. An additive
MyChron dashboard preset lays out and binds a complete race display in one
click. Widgets can be bound to channels from any loaded source, dragged and
resized over a muted video preview, styled with independent foreground and
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
