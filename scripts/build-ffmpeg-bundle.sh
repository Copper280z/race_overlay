#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: build-ffmpeg-bundle.sh OUTPUT_DIR" >&2
    exit 2
fi

output_dir=$1
case "$output_dir" in
    ""|/|.|..|*/.|*/..) echo "refusing unsafe output directory: $output_dir" >&2; exit 2 ;;
esac
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
case "$output_dir" in
    /*) ;;
    *) output_dir="$(pwd)/$output_dir" ;;
esac
if [ -e "$output_dir" ]; then
    echo "output directory already exists; remove it explicitly before rebuilding: $output_dir" >&2
    exit 2
fi

ffmpeg_version=8.1.2
ffmpeg_sha256=464beb5e7bf0c311e68b45ae2f04e9cc2af88851abb4082231742a74d97b524c
ffmpeg_url="https://ffmpeg.org/releases/ffmpeg-$ffmpeg_version.tar.xz"
x264_commit=b35605ace3ddf7c1a5d67a2eb553f034aef41d55
x264_sha256=cd71a7515b0e9a012e1ac9b1f8415bebcaf6fc97d4db32286642ac4c0fbe24f9
x264_url="https://code.videolan.org/videolan/x264/-/archive/$x264_commit/x264-$x264_commit.tar.gz"
x265_version=4.1
x265_sha256=a31699c6a89806b74b0151e5e6a7df65de4b49050482fe5ebf8a4379d7af8f29
x265_url="https://bitbucket.org/multicoreware/x265_git/downloads/x265_$x265_version.tar.gz"

build_root=${RACE_OVERLAY_FFMPEG_BUILD_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/race-overlay-ffmpeg-build}
downloads="$build_root/downloads"
sources="$build_root/sources"
prefix="$build_root/prefix"
mkdir -p "$downloads" "$sources" "$prefix"

hash_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

download() {
    url=$1
    expected=$2
    destination=$3
    if [ ! -f "$destination" ]; then
        curl --fail --location --retry 3 --output "$destination.partial" "$url"
        mv "$destination.partial" "$destination"
    fi
    actual=$(hash_file "$destination")
    if [ "$actual" != "$expected" ]; then
        echo "checksum mismatch for $destination" >&2
        echo "expected $expected" >&2
        echo "actual   $actual" >&2
        exit 1
    fi
}

ffmpeg_archive="$downloads/ffmpeg-$ffmpeg_version.tar.xz"
download "$ffmpeg_url" "$ffmpeg_sha256" "$ffmpeg_archive"
ffmpeg_source="$sources/ffmpeg-$ffmpeg_version"
if [ ! -d "$ffmpeg_source" ]; then
    tar -xf "$ffmpeg_archive" -C "$sources"
fi

host=$(uname -s)
tool_suffix=
license_name=COPYING.LGPLv2.1
configure_platform=
case "$host" in
    Darwin)
        configure_platform="--disable-gpl --enable-videotoolbox --enable-audiotoolbox"
        ;;
    Linux)
        license_name=COPYING.GPLv2
        x264_archive="$downloads/x264-$x264_commit.tar.gz"
        x265_archive="$downloads/x265-$x265_version.tar.gz"
        download "$x264_url" "$x264_sha256" "$x264_archive"
        download "$x265_url" "$x265_sha256" "$x265_archive"

        x264_source="$sources/x264-$x264_commit"
        if [ ! -d "$x264_source" ]; then
            tar -xzf "$x264_archive" -C "$sources"
        fi
        if [ ! -f "$prefix/lib/pkgconfig/x264.pc" ]; then
            (cd "$x264_source" && ./configure \
                --prefix="$prefix" --enable-static --disable-cli --enable-pic && \
                make -j"$(getconf _NPROCESSORS_ONLN)" && make install)
        fi

        x265_source="$sources/x265_$x265_version"
        if [ ! -d "$x265_source" ]; then
            tar -xzf "$x265_archive" -C "$sources"
        fi
        if [ ! -f "$prefix/lib/pkgconfig/x265.pc" ]; then
            cmake -S "$x265_source/source" -B "$x265_source/build-race-overlay" \
                -G Ninja \
                -DCMAKE_BUILD_TYPE=Release \
                -DCMAKE_INSTALL_PREFIX="$prefix" \
                -DENABLE_SHARED=OFF \
                -DENABLE_CLI=OFF \
                -DENABLE_LIBNUMA=OFF
            cmake --build "$x265_source/build-race-overlay"
            cmake --install "$x265_source/build-race-overlay"
        fi
        PKG_CONFIG_PATH="$prefix/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
        export PKG_CONFIG_PATH
        configure_platform="--enable-gpl --enable-libx264 --enable-libx265 --pkg-config-flags=--static --extra-cflags=-I$prefix/include --extra-ldflags=-L$prefix/lib --extra-ldflags=-static-libgcc --extra-ldflags=-static-libstdc++"
        ;;
    MINGW*|MSYS*)
        tool_suffix=.exe
        configure_platform="--disable-gpl --arch=x86_64 --target-os=mingw32 --enable-d3d11va --enable-mediafoundation --extra-ldflags=-static-libgcc"
        ;;
    *)
        echo "unsupported FFmpeg build host: $host" >&2
        exit 1
        ;;
esac

ffmpeg_prefix="$build_root/ffmpeg-prefix"
configuration_stamp="$ffmpeg_source/.race-overlay-configured-$host"
configuration_signature="$host $configure_platform"
recorded_configuration=$(cat "$configuration_stamp" 2>/dev/null || true)
if [ "$recorded_configuration" != "$configuration_signature" ]; then
    if [ -f "$ffmpeg_source/ffbuild/config.mak" ]; then
        (cd "$ffmpeg_source" && make distclean)
    fi
    (cd "$ffmpeg_source" && ./configure \
        --prefix="$ffmpeg_prefix" \
        --disable-autodetect \
        --disable-debug \
        --disable-doc \
        --disable-nonfree \
        --enable-static \
        --disable-shared \
        $configure_platform)
    printf '%s\n' "$configuration_signature" > "$configuration_stamp"
fi
(cd "$ffmpeg_source" && make -j"$(getconf _NPROCESSORS_ONLN)" && make install)

ffmpeg="$ffmpeg_prefix/bin/ffmpeg$tool_suffix"
ffprobe="$ffmpeg_prefix/bin/ffprobe$tool_suffix"
for tool in "$ffmpeg" "$ffprobe"; do
    if [ ! -f "$tool" ]; then
        echo "FFmpeg build did not produce $tool" >&2
        exit 1
    fi
done

filters=$($ffmpeg -hide_banner -filters 2>/dev/null)
for required_filter in overlay pad scale fps format; do
    if ! printf '%s\n' "$filters" | grep -Eq "[[:space:]]$required_filter[[:space:]]"; then
        echo "FFmpeg build is missing required filter: $required_filter" >&2
        exit 1
    fi
done
encoders=$($ffmpeg -hide_banner -encoders 2>/dev/null)
case "$host" in
    Darwin)
        printf '%s\n' "$encoders" | grep -q 'h264_videotoolbox'
        printf '%s\n' "$encoders" | grep -q 'hevc_videotoolbox'
        ;;
    Linux)
        printf '%s\n' "$encoders" | grep -q 'libx264'
        printf '%s\n' "$encoders" | grep -q 'libx265'
        ;;
    MINGW*|MSYS*)
        printf '%s\n' "$encoders" | grep -q 'h264_mf'
        printf '%s\n' "$encoders" | grep -q 'hevc_mf'
        ;;
esac

mkdir -p "$output_dir/bin" "$output_dir/licenses" "$output_dir/source"
install -m 755 "$ffmpeg" "$output_dir/bin/ffmpeg$tool_suffix"
install -m 755 "$ffprobe" "$output_dir/bin/ffprobe$tool_suffix"
install -m 644 "$ffmpeg_source/$license_name" "$output_dir/LICENSE.txt"
install -m 644 "$ffmpeg_archive" "$output_dir/source/ffmpeg-$ffmpeg_version.tar.xz"
install -m 644 "$script_dir/build-ffmpeg-bundle.sh" "$output_dir/source/build-ffmpeg-bundle.sh"

if [ "$host" = Linux ]; then
    install -m 644 "$x264_archive" "$output_dir/source/x264-$x264_commit.tar.gz"
    install -m 644 "$x265_archive" "$output_dir/source/x265-$x265_version.tar.gz"
    install -m 644 "$x264_source/COPYING" "$output_dir/licenses/X264_LICENSE.txt"
    install -m 644 "$x265_source/COPYING" "$output_dir/licenses/X265_LICENSE.txt"
fi

$ffmpeg -version > "$output_dir/BUILD_INFO.txt"
{
    echo "Race Overlay FFmpeg bundle"
    echo
    echo "FFmpeg source: $ffmpeg_url"
    echo "FFmpeg SHA-256: $ffmpeg_sha256"
    if [ "$host" = Linux ]; then
        echo "x264 source: $x264_url"
        echo "x264 SHA-256: $x264_sha256"
        echo "x265 source: $x265_url"
        echo "x265 SHA-256: $x265_sha256"
    fi
    echo
    echo "The exact source archives are included in source/."
} > "$output_dir/SOURCE.txt"

echo "Built FFmpeg bundle at $output_dir"
