#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: stage-ffmpeg.sh BUNDLE_DIR DESTINATION_DIR" >&2
    exit 2
fi

bundle_dir=$1
destination=$2
tool_suffix=
if [ -f "$bundle_dir/bin/ffmpeg.exe" ]; then
    tool_suffix=.exe
fi

for tool in ffmpeg ffprobe; do
    if [ ! -f "$bundle_dir/bin/$tool$tool_suffix" ]; then
        echo "FFmpeg bundle is missing bin/$tool$tool_suffix" >&2
        exit 1
    fi
done
for document in LICENSE.txt BUILD_INFO.txt SOURCE.txt; do
    if [ ! -f "$bundle_dir/$document" ]; then
        echo "FFmpeg bundle is missing $document" >&2
        exit 1
    fi
done
if [ ! -d "$bundle_dir/source" ] ||
    ! find "$bundle_dir/source" -type f -print -quit | grep -q .; then
    echo "FFmpeg bundle must include its corresponding source in source/" >&2
    exit 1
fi
if ! grep -q -- '--disable-gpl' "$bundle_dir/BUILD_INFO.txt" ||
    ! grep -q -- '--disable-nonfree' "$bundle_dir/BUILD_INFO.txt"; then
    echo "FFmpeg bundle must explicitly disable GPL and nonfree components" >&2
    exit 1
fi
if grep -Eq -- '--enable-(gpl|nonfree)' "$bundle_dir/BUILD_INFO.txt"; then
    echo "FFmpeg bundle enables GPL or nonfree components and cannot be packaged" >&2
    exit 1
fi

mkdir -p "$destination"
cp -R "$bundle_dir/." "$destination/"
install -m 755 "$bundle_dir/bin/ffmpeg$tool_suffix" "$destination/bin/ffmpeg$tool_suffix"
install -m 755 "$bundle_dir/bin/ffprobe$tool_suffix" "$destination/bin/ffprobe$tool_suffix"
