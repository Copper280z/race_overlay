# FFmpeg bundle input

Race Overlay release packages can contain portable FFmpeg command-line tools.
Set `RACE_OVERLAY_FFMPEG_BUNDLE` to a prepared directory before running a
platform packaging script. The directory must have this shape:

```text
ffmpeg-bundle/
├── bin/
│   ├── ffmpeg[.exe]
│   └── ffprobe[.exe]
├── BUILD_INFO.txt
├── LICENSE.txt
├── SOURCE.txt
├── licenses/          # licenses for compiled-in dependencies, when applicable
└── source/            # exact source archives, patches, and build scripts
```

The tools must be portable to the target machines; copying a dynamically linked
package-manager installation is generally not sufficient. Race Overlay's
release policy uses an LGPL-only FFmpeg configuration. `BUILD_INFO.txt` records
the complete `ffmpeg -version` output and how the tools were built, including
explicit `--disable-gpl` and `--disable-nonfree` flags. `LICENSE.txt` contains
the LGPL text that applies to that exact build.
`SOURCE.txt` inventories the complete corresponding source and build materials
included in `source/`. Do not package an FFmpeg build configured with
`--enable-nonfree`.

The packaging scripts reject incomplete bundle metadata, `--enable-gpl`, and
`--enable-nonfree`, then place the entire directory under
`third-party/ffmpeg`. Race Overlay checks that location before checking the
user's `PATH`.
