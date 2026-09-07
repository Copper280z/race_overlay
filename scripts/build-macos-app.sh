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

mkdir -p "$macos"
install -m 755 "$target_dir/release/race-overlay" "$macos/race-overlay"
install -m 644 "$repo_root/packaging/macos/Info.plist" "$contents/Info.plist"
touch "$app_bundle"

echo "Built $app_bundle"
echo "Launch it with: open \"$app_bundle\""
