# Race Overlay

A cross-platform Rust desktop application for comparing motorsport telemetry
and burning synchronized data overlays into video. Standard SDR Insta360 X4 Air
dual-track INSV can be reframed directly in Analysis, Overlay, and export, with
hard-cut/feather seams and optional camera-motion smoothing. It reads the IMU
trailer in Insta360 INSV/LRV files,
AiM MyChron XRK logs, generic timestamped CSV, or deterministic synthetic race
data. Recordings can be downloaded from a MyChron6 over its Wi-Fi, by hand or
automatically in Track mode whenever the logger comes into range.

## Install and run

### Packaged releases

The easiest installation is a package for your operating system from the
[GitHub Releases page](https://github.com/Copper280z/race_overlay/releases),
when one is available. A release labeled as including FFmpeg contains everything
needed for video preview, synchronization, and export; extract it and run Race
Overlay. It does not need Rust, Homebrew, or a separate FFmpeg installation.

Race Overlay does not yet have Apple Developer ID signing or notarization. The
macOS app is ad-hoc signed to protect its bundle integrity, but its first launch
still requires you to Control-click **Race Overlay**, choose **Open**, then
confirm **Open** once. Windows may show a SmartScreen notice for an unfamiliar
application. Only run packages downloaded from this project's release page.

### Building from source

Until a package is available for your computer, build from source using the
steps below. No programming experience is required, but you will enter a few
commands in Terminal or PowerShell. The first build downloads Rust libraries and
can take several minutes.

You need:

- the current stable Rust toolchain (Rust 1.95 or newer for this version); and
- both `ffmpeg` and `ffprobe` for video features. They come together in the
  normal FFmpeg package. Telemetry-only analysis works without them.

Download the source without Git:

1. On the [project page](https://github.com/Copper280z/race_overlay), choose
   **Code**, then **Download ZIP**.
2. Open the downloaded ZIP and extract the `race_overlay-main` folder somewhere
   convenient, such as Documents.
3. Follow the section for your operating system below.

#### macOS

1. Open **Terminal** using Spotlight Search.
2. Install Apple's command-line build tools:

   ```sh
   xcode-select --install
   ```

   If macOS says they are already installed, continue.
3. Install [Homebrew](https://brew.sh/) if the command `brew --version` says
   `command not found`. Use the installer shown on the Homebrew website and
   follow any **Next steps** it prints at the end. Close and reopen Terminal
   afterward.
4. Install FFmpeg:

   ```sh
   brew install ffmpeg
   ```

5. Install Rust using the command on the
   [official Rust installation page](https://www.rust-lang.org/tools/install/).
   Choose the default installation when asked, then close and reopen Terminal.
6. In Finder, open the extracted `race_overlay-main` folder. Control-click the
   folder background and choose **Services → New Terminal at Folder**. If that
   item is unavailable, type `cd ` in Terminal, drag the folder onto the Terminal
   window, and press Return.
7. Build the normal macOS application:

   ```sh
   ./scripts/build-macos-app.sh
   ```

8. In Finder, open `target`, then `release`. Move **Race Overlay.app** to your
   Applications folder or open it where it is.

The app bundle can find Homebrew FFmpeg on both Apple Silicon and Intel Macs,
even when launched from Finder.

#### Windows 10 or 11

1. Open **PowerShell** from the Start menu.
2. Install FFmpeg with Windows Package Manager:

   ```powershell
   winget install --id Gyan.FFmpeg -e --source winget
   ```

   Accept any source or package agreement prompts. When installation finishes,
   close every PowerShell and Race Overlay window, then open PowerShell again.
   If `winget` is not recognized, install or update **App Installer** from the
   Microsoft Store and retry.
3. Download and run `rustup-init.exe` from the
   [official Rust installation page](https://www.rust-lang.org/tools/install/).
   Use the default installation. If it offers to install the Visual Studio C++
   build tools, allow it—Windows needs those tools to build Rust applications.
   Restart PowerShell afterward.
4. Open the extracted `race_overlay-main` folder in File Explorer. Click the
   address bar, type `powershell`, and press Enter. A PowerShell window will open
   in the correct folder.
5. Build Race Overlay:

   ```powershell
   cargo build --release -p race-overlay
   ```

6. Open `target\release` and double-click `race-overlay.exe`. You can right-drag
   it to the desktop and choose **Create shortcuts here** if desired.

#### Linux

These commands cover Ubuntu and Debian. Open Terminal and install the compiler
tools and FFmpeg:

```sh
sudo apt update
sudo apt install build-essential ffmpeg
```

On Arch Linux, use `sudo pacman -S --needed base-devel ffmpeg`. On another
distribution, install its C/C++ build-tools package and its package named
`ffmpeg`; both `ffmpeg` and `ffprobe` must be included.

Install Rust using the command on the
[official Rust installation page](https://www.rust-lang.org/tools/install/),
choose the default installation, and close and reopen Terminal. Then open the
extracted source folder in your file manager, right-click its background, and
choose **Open in Terminal**. Build and start Race Overlay with:

```sh
cargo build --release -p race-overlay
./target/release/race-overlay
```

### Check or troubleshoot FFmpeg

Run both commands in a newly opened Terminal or PowerShell window:

```text
ffmpeg -version
ffprobe -version
```

Each should print version and build information. If either command is not found:

- close and reopen Race Overlay after installing FFmpeg;
- also close and reopen Terminal or PowerShell so it receives the updated
  program search path;
- on Windows, repeat the exact `winget` command above rather than downloading an
  archive and editing `PATH` manually; and
- on macOS, make sure `brew install ffmpeg` completed without an error and that
  you followed Homebrew's post-install **Next steps**.

If both version commands work but Race Overlay still reports that FFmpeg is
missing, start Race Overlay from that same Terminal or PowerShell window and
open an issue with the displayed error. Include the output of both version
commands. Do not download a file claiming to be FFmpeg from an unrelated
download site; the [official FFmpeg download page](https://ffmpeg.org/download.html)
lists the package and executable providers recognized by the FFmpeg project.

The new **Analysis** workspace opens without a video or saved track. Drop XRK
files to compare circuit laps or separate autocross runs, pin a reference, and
inspect linked plots, independent video viewers, channel-colored course maps,
and actual GPS over locally saved imagery. Plot against elapsed time, matched
course position, or each run's traveled distance. Panels can be dragged into
tabs, split docks, or floating windows; three layout presets provide quick
starting points. Analysis workspaces use `.race-analysis.json` files.

**Overlay** retains the original editor and export workflow. Use a recording's
**Edit overlay / advanced sync** action to calibrate sources, inspect detailed
timing, customize its dashboard, or export. Switching back carries the changes
into that recording without replacing the other comparisons.

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
Supported camera telemetry is audio-aligned automatically, so every normal control uses
the exported video's timeline.

See [docs/usage.md](docs/usage.md) for the end-to-end workflow and detailed
behavior reference.

## Packaging releases

Release packages can include portable `ffmpeg` and `ffprobe` executables, so end
users do not need to install FFmpeg. Prepare the licensed bundle described in
[packaging/ffmpeg/README.md](packaging/ffmpeg/README.md), set
`RACE_OVERLAY_FFMPEG_BUNDLE` to that directory, and run the platform script:

- macOS: `./scripts/build-macos-app.sh`
- Linux: `./scripts/build-linux-package.sh`
- Windows PowerShell: `.\scripts\build-windows-package.ps1`

The scripts reject a bundled FFmpeg directory that lacks its license, build
information, or corresponding-source location. Without the environment variable,
they produce an unbundled package that uses an installed FFmpeg copy.

The macOS script signs nested tools and seals the completed app bundle. It uses
an ad-hoc identity by default; release maintainers with a Developer ID
certificate can set `RACE_OVERLAY_MACOS_SIGN_IDENTITY` to that certificate's
full identity. Apple notarization is still required for ordinary double-click
launches without the first-run Control-click confirmation.

GitHub Actions runs these packaging scripts on native macOS, Windows, and Linux
runners. A manual **Build release packages** run stores all three packages as
workflow artifacts. Pushing a tag such as `v0.1.0` also creates a GitHub Release
and attaches them with a SHA-256 checksum file. Prerelease tags such as
`v0.1.0-alpha.1` are marked as prereleases.

Each native job builds FFmpeg from checksum-pinned upstream source before
building Race Overlay. macOS uses VideoToolbox, Windows uses MediaFoundation,
and Linux statically includes x264/x265 software encoders. Required filters and
encoders are verified before packaging. The complete source archives, license
texts, and actual FFmpeg build configuration are included in every package.

## License

Race Overlay is available under the [MIT License](LICENSE). FFmpeg is a separate
component under its own license. The macOS and Windows bundles use an
LGPL-2.1-or-later FFmpeg configuration. The FFmpeg executable in the Linux
bundle enables x264 and x265 and is therefore distributed under
GPL-2.0-or-later. See [third-party notices](THIRD_PARTY_NOTICES.md).

### Direct X4 Air video

Open a standard SDR dual-track `.insv` to import video and camera telemetry together.
Drag directly on an Analysis video image to aim; scroll to change horizontal FOV.
In Overlay, enable **Reframe video** to use these gestures. View settings belong to
recordings and survive saving and Analysis–Overlay handoffs. Output is 1080p or
2160p with original AAC audio. Ordinary video workflows remain available.

GPU projection uses the existing wgpu stack. macOS decoding uses FFmpeg's
VideoToolbox support; other platforms currently use software decoding. CPU
projection is available when no GPU adapter is available, with reduced performance.
No OpenCV or Python installation is needed to run these features. See
[usage](docs/usage.md#direct-x4-air-video) and [validation/status](docs/raw-video.md)
for supported inputs, performance evidence, and outstanding release gates.
