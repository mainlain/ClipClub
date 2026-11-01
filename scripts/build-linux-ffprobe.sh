#!/usr/bin/env bash
set -euo pipefail
# Build an LGPL FFmpeg validator with static FFmpeg/dav1d and system glibc.
# Native Linux tools: cc, make, pkg-config, meson, ninja, tar, xz, sha256sum.
project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_archive="${CLIPCROP_FFMPEG_SOURCE:-$project_root/.build/linux-tools/ffmpeg-9.0.2.tar.xz}"
dav1d_archive="${CLIPCROP_DAV1D_SOURCE:-$project_root/.build/linux-tools/dav1d-1.5.3.tar.xz}"
build_directory="${CLIPCROP_FFMPEG_BUILD_DIR:-$project_root/.build/linux-ffmpeg-build}"
dav1d_build_directory="${CLIPCROP_DAV1D_BUILD_DIR:-$build_directory/dav1d-build}"
codec_prefix="${CLIPCROP_CODEC_PREFIX:-$build_directory/codecs}"
output_directory="${CLIPCROP_FFPROBE_OUTPUT:-$project_root/.build/linux-ffprobe}"
[[ "$(uname -s)" == Linux ]] || { printf 'Run this builder on Linux.\n' >&2; exit 1; }
[[ -f "$source_archive" ]] || { printf 'Provide official ffmpeg-9.0.2.tar.xz with CLIPCROP_FFMPEG_SOURCE.\n' >&2; exit 1; }
[[ -f "$dav1d_archive" ]] || { printf 'Provide official dav1d-1.5.3.tar.xz with CLIPCROP_DAV1D_SOURCE.\n' >&2; exit 1; }
# Pin the upstream source archives used for this build; do not silently accept another release.
printf '%s  %s\n' \
  8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e "$source_archive" \
  732010aa5ef461fa93355ed2c6c5fedb48ddc4b74e697eaabe8907eaeb943011 "$dav1d_archive" \
  | sha256sum --check --status
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
# Keep third-party codecs embedded while allowing the OS to update libc.
# Refuse to replace a working validator if the linker picked a system dav1d/FFmpeg.
ldd ffprobe > "$output_directory/ldd.txt"
if grep -Eq 'lib(dav1d|avcodec|avformat|avutil|swscale|swresample)\.so' "$output_directory/ldd.txt"; then
  printf 'Unexpected dynamic codec dependency; preserving existing validator.\n' >&2
  cat "$output_directory/ldd.txt" >&2
  exit 1
fi
cp ffprobe "$output_directory/ffprobe.new"
chmod +x "$output_directory/ffprobe.new"
"$output_directory/ffprobe.new" -version > /dev/null
mv "$output_directory/ffprobe.new" "$output_directory/ffprobe"
cp COPYING.LGPLv2.1 "$output_directory/LICENSE.LGPLv2.1"
cp "$build_directory/dav1d-1.5.3/COPYING" "$output_directory/dav1d-copyright.txt"
cp "$project_root/scripts/build-linux-ffprobe.sh" "$output_directory/build-linux-ffprobe.sh"
"$output_directory/ffprobe" -version > "$output_directory/version.txt"
"$output_directory/ffprobe" -L > "$output_directory/license-report.txt" 2>&1
sha256sum "$source_archive" "$dav1d_archive" "$output_directory/ffprobe" > "$output_directory/SHA256SUMS.txt"
printf 'Built Linux ffprobe: %s\n' "$output_directory/ffprobe"
