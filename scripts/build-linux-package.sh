#!/bin/sh
set -eu

if [ "$(uname -s)" != "Linux" ]; then
    echo "build-linux-package.sh requires Linux" >&2
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

package_name="race-overlay-linux-$(uname -m)"
package_dir="$target_dir/release/$package_name"
rm -rf "$package_dir"
mkdir -p "$package_dir/licenses"
install -m 755 "$target_dir/release/race-overlay" "$package_dir/race-overlay"
install -m 644 "$repo_root/LICENSE" "$package_dir/licenses/RACE_OVERLAY_LICENSE.txt"
install -m 644 "$repo_root/THIRD_PARTY_NOTICES.md" "$package_dir/THIRD_PARTY_NOTICES.md"
if [ -n "${RACE_OVERLAY_FFMPEG_BUNDLE:-}" ]; then
    sh "$repo_root/scripts/stage-ffmpeg.sh" \
        "$RACE_OVERLAY_FFMPEG_BUNDLE" \
        "$package_dir/third-party/ffmpeg"
    echo "Bundled FFmpeg from $RACE_OVERLAY_FFMPEG_BUNDLE"
else
    echo "FFmpeg was not bundled; the app will use an installed copy"
fi

archive="$target_dir/release/$package_name.tar.gz"
tar -C "$target_dir/release" -czf "$archive" "$package_name"
echo "Built $archive"
