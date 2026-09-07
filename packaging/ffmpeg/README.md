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
package-manager installation is generally not sufficient. `BUILD_INFO.txt`
records the complete `ffmpeg -version` output and how the tools were built,
including an explicit `--disable-nonfree` flag. `LICENSE.txt` contains the LGPL
or GPL text that applies to that exact build.
`SOURCE.txt` inventories the complete corresponding source and build materials
included in `source/`. Do not package an FFmpeg build configured with
`--enable-nonfree`.

The packaging scripts reject incomplete bundle metadata and
`--enable-nonfree`, then place the entire directory under `third-party/ffmpeg`.
Race Overlay checks that location before checking the user's `PATH`.

The release workflow creates these inputs with
`scripts/build-ffmpeg-bundle.sh`: LGPL-only VideoToolbox and MediaFoundation
builds on macOS and Windows, and a GPL build with statically linked x264/x265 on
Linux. All are built from the source archives copied into `source/`.
