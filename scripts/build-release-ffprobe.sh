#!/usr/bin/env bash
set -euo pipefail
# Build the release validator from pinned, retained source on its native host.
# Linux: cc, make, pkg-config, meson, ninja, tar, xz. macOS: Xcode tools + those tools.
script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(cd "$script_directory/.." && pwd)"
source_directory="$project_root/.build/release-tools"
if [[ -f "$script_directory/ffmpeg-9.0.2.tar.xz" ]]; then
  source_directory="$script_directory"
fi
source_archive="${CLIPCROP_FFMPEG_SOURCE:-$source_directory/ffmpeg-9.0.2.tar.xz}"
dav1d_archive="${CLIPCROP_DAV1D_SOURCE:-$source_directory/dav1d-1.5.3.tar.xz}"
build_directory="${CLIPCROP_FFMPEG_BUILD_DIR:-$project_root/.build/release-ffmpeg-build}"
dav1d_build_directory="${CLIPCROP_DAV1D_BUILD_DIR:-$build_directory/dav1d-build}"
codec_prefix="${CLIPCROP_CODEC_PREFIX:-$build_directory/codecs}"
output_directory="${CLIPCROP_FFPROBE_OUTPUT:-$project_root/.build/release-ffprobe}"
host_system="$(uname -s)"
[[ "$host_system" == Linux || "$host_system" == Darwin ]] || { printf 'Use a native Linux or macOS host.\n' >&2; exit 1; }
if [[ "$host_system" == Darwin && "$(uname -m)" != arm64 ]]; then
  printf 'Build the macOS validator on Apple Silicon.\n' >&2
  exit 1
fi
[[ -f "$source_archive" ]] || { printf 'Provide ffmpeg-9.0.2.tar.xz with CLIPCROP_FFMPEG_SOURCE.\n' >&2; exit 1; }
[[ -f "$dav1d_archive" ]] || { printf 'Provide dav1d-1.5.3.tar.xz with CLIPCROP_DAV1D_SOURCE.\n' >&2; exit 1; }
python3 - "$source_archive" "$dav1d_archive" <<'PY'
import hashlib
from pathlib import Path
import sys
for filename, expected in zip(sys.argv[1:], (
    '8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e',
    '732010aa5ef461fa93355ed2c6c5fedb48ddc4b74e697eaabe8907eaeb943011',
)):
    if hashlib.sha256(Path(filename).read_bytes()).hexdigest() != expected:
        raise SystemExit(f'Pinned source checksum does not match: {filename}')
PY
mkdir -p "$build_directory" "$output_directory"
if [[ ! -f "$build_directory/ffmpeg-9.0.2/configure" ]]; then
  tar -xJf "$source_archive" -C "$build_directory"
fi
if [[ ! -f "$build_directory/dav1d-1.5.3/meson.build" ]]; then
  tar -xJf "$dav1d_archive" -C "$build_directory"
fi
export PKG_CONFIG_PATH="$codec_prefix/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
if [[ ! -f "$codec_prefix/lib/libdav1d.a" ]]; then
  meson setup "$dav1d_build_directory" "$build_directory/dav1d-1.5.3" \
    --prefix="$codec_prefix" --libdir=lib --buildtype=release --default-library=static \
    -Denable_tools=false -Denable_tests=false -Denable_asm=false \
    > "$output_directory/dav1d-configure.log" 2>&1 \
    || { tail -60 "$output_directory/dav1d-configure.log"; exit 1; }
  ninja -C "$dav1d_build_directory" -j"${CLIPCROP_BUILD_JOBS:-2}" install \
    > "$output_directory/dav1d-build.log" 2>&1 \
    || { tail -60 "$output_directory/dav1d-build.log"; exit 1; }
fi
cd "$build_directory/ffmpeg-9.0.2"
configure_args=(
  --disable-everything --disable-autodetect --disable-network --disable-doc
  --disable-debug --disable-programs --disable-avdevice --disable-avfilter
  --disable-x86asm --enable-static --disable-shared --enable-small
  --enable-ffprobe --enable-protocol=file --enable-demuxer=mov,matroska
  --enable-decoder=aac,aac_fixed,ac3,ac3_fixed,eac3,h264,hevc,mpeg4,vp8,vp9,alac,mp3,mp3float,opus,vorbis,libdav1d
  --enable-parser=aac,aac_latm,ac3,h264,hevc,mpegaudio,mpeg4video,opus,vorbis,vp8,vp9,av1
  --enable-libdav1d --pkg-config-flags=--static
)
printf '%s\n' "${configure_args[@]}" > "$output_directory/configure-arguments.txt"
./configure "${configure_args[@]}" > "$output_directory/configure.log" 2>&1 \
  || { tail -50 "$output_directory/configure.log"; exit 1; }
make -j"${CLIPCROP_BUILD_JOBS:-2}" ffprobe > "$output_directory/build.log" 2>&1 \
  || { tail -70 "$output_directory/build.log"; exit 1; }
if [[ "$host_system" == Linux ]]; then
  ldd ffprobe > "$output_directory/linked-libraries.txt"
else
  otool -L ffprobe > "$output_directory/linked-libraries.txt"
fi
if grep -Eq 'lib(dav1d|avcodec|avformat|avutil|swscale|swresample)[.].*(so|dylib)' "$output_directory/linked-libraries.txt"; then
  printf 'Unexpected dynamic codec dependency; preserving existing validator.\n' >&2
  cat "$output_directory/linked-libraries.txt" >&2
  exit 1
fi
cp ffprobe "$output_directory/ffprobe.new"
chmod +x "$output_directory/ffprobe.new"
"$output_directory/ffprobe.new" -version > /dev/null
mv "$output_directory/ffprobe.new" "$output_directory/ffprobe"
cp COPYING.LGPLv2.1 "$output_directory/LICENSE.LGPLv2.1"
cp "$build_directory/dav1d-1.5.3/COPYING" "$output_directory/dav1d-copyright.txt"
"$output_directory/ffprobe" -version > "$output_directory/version.txt"
"$output_directory/ffprobe" -L > "$output_directory/license-report.txt" 2>&1
# Distribute all corresponding source/build inputs with the exact validator.
mkdir -p "$output_directory/source"
cp "$source_archive" "$dav1d_archive" "$output_directory/source/"
cp "$script_directory/build-release-ffprobe.sh" "$output_directory/source/"
cp "$output_directory/configure-arguments.txt" "$output_directory/source/"
python3 - "$output_directory" <<'PY'
import hashlib
from pathlib import Path
import sys
root = Path(sys.argv[1])
paths = [root / 'ffprobe', *sorted((root / 'source').iterdir())]
(root / 'SHA256SUMS.txt').write_text(''.join(
    f'{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(root)}\n'
    for path in paths))
PY
printf 'Built release ffprobe: %s\n' "$output_directory/ffprobe"
