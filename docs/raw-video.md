# Raw X4 Air implementation and validation

## Status

The shared stage 1 and stage 2 paths are implemented in Analysis, Overlay, and
export. They are **not yet release-qualified**: live desktop interaction checks
and Windows/Linux validation remain outstanding. On the validation attempt the
Mac was locked, so computer-use automation could not inspect the live application.
Headless gesture, linked-clock regression, persistence, media, and export tests
are separate evidence and do not replace these checks.

Stage 3 has an interchangeable backend interface and a reproducible OpenCV
prototype. **No advanced backend is enabled in the application.** Temporal seam
consistency, deterministic bounded preparation across seeks, visual quality over
moving footage, native packaging, and the full live-preview gate are not complete.
The application explicitly reports an unavailable backend for an `adaptive` seam
setting. It does not silently change that setting to feathering.

## Implementation

- `overlay-core/src/video/`: bounded INSV trailer parsing, two Mei lens models,
  decoded-track orientation/crop mapping, recording/project settings, and an
  independent optical-rig gyro trajectory. Normalized video and IMU clocks are
  applied once. Motion integration retains every gyro sample; smoothing uses a
  100 Hz trajectory, and sampling composes it with the full-rate instantaneous
  orientation. Motion gaps split preparation and are never extrapolated.
- `overlay-render/src/video/`: shared CPU reference and offscreen wgpu projection,
  cached pixel rays and source buffers, valid-coverage hard/feather seams, and an
  advanced backend contract separating frame preparation from view rendering.
- `overlay-media/src/processed/`: persistent paired FFmpeg decoders, integer PTS
  pairing, bounded/coalescing preview requests, processing-revision invalidation,
  and direct transformed-frame export with original audio. Source BT.709 range
  conversion is explicit; export converts full-range RGBA to BT.709 while retaining the source range.
- `app/src/video_processing.rs`: shared settings and worker wiring, view controls,
  direct Analysis gestures, and Overlay's explicit Reframe interaction.

The source used for validation is an X4 Air v1.2.7 standard SDR recording with two
3840 × 3840 HEVC tracks, full-range BT.709, 30000/1001 fps, and AAC audio. The lens
metadata contains independent intrinsics/distortion, orientations, crop and
translation. Projection uses calibrated rays at infinity; the inter-lens baseline
is retained but does not provide scene depth or remove near-field parallax.

The raw sensor axis permutation was checked against spherical feature rotations
in the supplied footage. A frame-interval gyro fit found agreement near the
embedded time mapping (best residual lag approximately 2 ms). This check must use
passthrough frame output: FFmpeg's default frame duplication near a seek otherwise
introduces a misleading one-frame timing error. No fitted time offset is applied
in production. Rolling-shutter distortion and translation remain uncorrected.

## Measured evidence (2026-09-09)

Hardware: Apple M2, 16 GiB RAM; macOS 26.5; FFmpeg 9.0.1 with VideoToolbox; release
Rust build; wgpu compute projection. The 60-second interval starts at 120 s.

- 1,800 decoded/projected frames, 960 × 540, stabilization enabled, fixed seam
  crossing the lenses: 1/1,799 late frames (0.056%) after warm-up in a paced pipeline run.
- Processing p95: 5.02 ms. Cached-frame aim/FOV changes: p95 1.23 ms, max 1.42 ms.
- Wall time including first-frame startup, diagnostic images, and cached-view
  probes: 60.601 s. This is a processing harness, not a measurement of live egui
  presentation, and therefore does not certify dropped-frame rates in Analysis.
- A real one-second cross-lens 1920 × 1080 feathered, stabilized export passed;
  3840 × 2160 projection and full-range color signaling were also checked.
- Generated dual-track export preserved 30000/1001 frame timing and every AAC
  packet hash. Cancellation preserved the previous destination file.
- The GPU and CPU reference projections agreed within two byte values on the
  deterministic GPU regression fixture.

OpenCV 5.0.0 prototype: exposure compensation, graph-cut seam finding and four-band
multiband blending; optional DIS flow with forward/backward consistency and a
six-pixel displacement limit. Measurements below cover **both** fixed overlap
regions and exclude decoding, mapping into those regions, final projection, and UI.

| Each region | Baseline median / p95 | With DIS median / p95 |
| --- | --- | --- |
| 96 × 256 | 50.25 / 50.60 ms | 46.12 / 47.69 ms |
| 64 × 192 | 16.19 / 16.66 ms | 17.95 / 18.10 ms |

The larger prototype exceeds the 33.37 ms frame budget by itself. The smaller
prototype warrants further evaluation, but has not passed quality or temporal
checks. Only approximately 10–31% of pixels in its two regions passed the flow
checks on this frame. Those fractions include invalid lens coverage and are not a
quality score. Adding a native OpenCV dependency is deferred until a candidate
passes the complete gate. Python/OpenCV are development-only tools here.

The required formatting, workspace tests (167 passed), Clippy with warnings denied,
and release build passed on this Mac.

## Reproduce explicit checks

Routine tests need no video fixture or FFmpeg. Local/GPU/media checks are ignored
by default:

```sh
cargo test -p overlay-core video:: -- --nocapture
cargo test -p overlay-media processed::tests -- --ignored --nocapture
cargo test -p overlay-render video::tests::gpu_matches_cpu -- --ignored --nocapture
cargo test -p race-overlay supplied_raw_video_exports -- --ignored --nocapture
cargo run --release -p race-overlay --example inspect_raw_video -- \
  VID_20260830_124108_00_017.insv /tmp/raw-validation 120 1800 paced
```

The harness writes decoded lens images, projected views and optical diagnostics
outside the repository. For the stitching prototype, install `numpy` and
`opencv-contrib-python` in an isolated Python environment, then run:

```sh
python scripts/prototype_raw_stitching.py /tmp/raw-validation /tmp/stitch-results \
  --width 96 --height 256 --iterations 10
```

Before release, complete live Analysis drag/scroll while playing and paused,
linked comparison scrubbing, lap switching, pane resizing, multiple recordings,
shared views, save/reopen, Overlay Reframe/widget interaction, stabilized gap
presentation, and preview/export comparison. Repeat live performance checks on
supported distribution platforms. Stage 3 additionally requires the full moving
scene, temporal consistency, seeking, packaging and 60-second live gate in the
implementation plan. Run the contributor guide's four workspace checks before
publication.
