# Third-party notices

Race Overlay is licensed under the MIT License. Official release packages may
also contain a separately built, LGPL-only copy of the `ffmpeg` and `ffprobe`
command-line programs from the [FFmpeg project](https://ffmpeg.org/).

FFmpeg is normally licensed under LGPL-2.1-or-later. System builds that enable
GPL components, including libx264 or libx265, are instead licensed under
GPL-2.0-or-later. Race Overlay release packages reject those GPL and nonfree
configurations. FFmpeg and its bundled dependencies are not relicensed under
Race Overlay's MIT License.

Every Race Overlay package containing FFmpeg must include, alongside the tools:

- FFmpeg's applicable license text and copyright notices;
- the exact FFmpeg version and configure/build information;
- the licenses for other libraries compiled into that FFmpeg build; and
- the complete corresponding source and build materials used to produce the
  distributed binaries.

The package-specific files are stored in its `third-party/ffmpeg` directory.
Source-only builds of Race Overlay do not contain FFmpeg and use a separately
installed copy instead.
