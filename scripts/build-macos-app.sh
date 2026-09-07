#!/bin/sh
set -eu

if [ "$(uname -s)" != "Darwin" ]; then
    echo "build-macos-app.sh requires macOS" >&2
    exit 1
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
cd "$repo_root"

cargo build --release -p race-overlay

target_dir=${CARGO_TARGET_DIR:-target}
case "$target_dir" in
    /*) ;;
    *) target_dir="$repo_root/$target_dir" ;;
esac

app_bundle="$target_dir/release/Race Overlay.app"
contents="$app_bundle/Contents"
macos="$contents/MacOS"
resources="$contents/Resources"

rm -rf "$app_bundle"
mkdir -p "$macos" "$resources/licenses"
install -m 755 "$target_dir/release/race-overlay" "$macos/race-overlay"
install -m 644 "$repo_root/packaging/macos/Info.plist" "$contents/Info.plist"
install -m 644 "$repo_root/LICENSE" "$resources/licenses/RACE_OVERLAY_LICENSE.txt"
install -m 644 "$repo_root/THIRD_PARTY_NOTICES.md" "$resources/THIRD_PARTY_NOTICES.md"
if [ -n "${RACE_OVERLAY_FFMPEG_BUNDLE:-}" ]; then
    sh "$repo_root/scripts/stage-ffmpeg.sh" \
        "$RACE_OVERLAY_FFMPEG_BUNDLE" \
        "$resources/third-party/ffmpeg"
    echo "Bundled FFmpeg from $RACE_OVERLAY_FFMPEG_BUNDLE"
else
    echo "FFmpeg was not bundled; the app will use an installed copy"
fi
touch "$app_bundle"

archive="$target_dir/release/race-overlay-macos-$(uname -m).zip"
rm -f "$archive"
ditto -c -k --sequesterRsrc --keepParent "$app_bundle" "$archive"

echo "Built $app_bundle"
echo "Built $archive"
echo "Launch it with: open \"$app_bundle\""
