#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"

if [[ -n "${CLIPCROP_CARGO:-}" ]]; then
  cargo_bin="$CLIPCROP_CARGO"
elif command -v cargo >/dev/null 2>&1; then
  cargo_bin="$(command -v cargo)"
elif [[ -x "$project_root/.build/cargo/bin/cargo" ]]; then
  cargo_bin="$project_root/.build/cargo/bin/cargo"
else
  printf 'Install Rust/Cargo or set CLIPCROP_CARGO.\n' >&2
  exit 1
fi
if [[ "$cargo_bin" == "$project_root/.build/cargo/bin/cargo" ]]; then
  export CARGO_HOME="$project_root/.build/cargo"
  export RUSTUP_HOME="$project_root/.build/rustup"
  export PATH="$project_root/.build/cargo/bin:$PATH"
fi

if ! command -v python3 >/dev/null 2>&1; then
  printf 'Packaging needs Python 3 to collect dependency license notices.\n' >&2
  exit 1
fi

probe_path="${CLIPCROP_FFPROBE:-$project_root/vendor/ffprobe}"
if [[ ! -f "$probe_path" || ! -x "$probe_path" ]]; then
  printf 'Set CLIPCROP_FFPROBE to a trusted ffprobe executable for the target platform.\n' >&2
  exit 1
fi
probe_path="$(cd "$(dirname "$probe_path")" && pwd)/$(basename "$probe_path")"
export CLIPCROP_FFPROBE="$probe_path"

build_args=(build --locked --release --features bundle-ffprobe)
target_directory="${CARGO_TARGET_DIR:-$project_root/target}"
target_triple="${CLIPCROP_TARGET:-}"
if [[ -n "$target_triple" ]]; then
  build_args+=(--target "$target_triple")
  target_directory="$target_directory/$target_triple"
fi
"$cargo_bin" "${build_args[@]}"

binary_name=clipcrop
if [[ "$target_triple" == *windows* || "${OS:-}" == Windows_NT ]]; then
  binary_name=clipcrop.exe
fi
dist_directory="${CLIPCROP_DIST_DIR:-$project_root/dist}"
mkdir -p "$dist_directory/third-party"
cp "$target_directory/release/$binary_name" "$dist_directory/$binary_name"
cp README.md LICENSE THIRD_PARTY.md "$dist_directory/"
if [[ -f vendor/ffprobe-LICENSE.txt ]]; then
  cp vendor/ffprobe-LICENSE.txt "$dist_directory/third-party/ffprobe-GPLv3.txt"
fi

# Version reports only work when the probe can run on the build host. A
# cross-build still embeds the selected target binary without executing it.
if "$probe_path" -version > "$dist_directory/third-party/ffprobe-version.txt" 2>&1; then
  "$probe_path" -L > "$dist_directory/third-party/ffprobe-license-report.txt" 2>&1
else
  printf 'Selected ffprobe could not run on the build host; verify its target and license separately.\n' \
    > "$dist_directory/third-party/ffprobe-license-report.txt"
fi

metadata_file="$(mktemp "${TMPDIR:-/tmp}/clipcrop-metadata.XXXXXX")"
trap 'rm -f "$metadata_file"' EXIT
"$cargo_bin" metadata --locked --format-version 1 > "$metadata_file"
python3 - "$metadata_file" "$dist_directory/third-party" <<'PY'
import json
from pathlib import Path
import shutil
import sys

metadata = json.loads(Path(sys.argv[1]).read_text())
destination = Path(sys.argv[2])
inventory = []
for package in metadata["packages"]:
    if package["name"] == "clipcrop":
        continue
    entry = {key: package.get(key) for key in ("name", "version", "license", "repository", "homepage")}
    inventory.append(entry)
    source = Path(package["manifest_path"]).parent
    notices = []
    for pattern in ("LICENSE*", "LICENCE*", "COPYING*", "NOTICE*", "COPYRIGHT*"):
        notices.extend(source.glob(pattern))
    if package.get("license_file"):
        notices.append(source / package["license_file"])
    for notice in notices:
        if notice.is_file() and not notice.is_symlink():
            folder = destination / "rust" / (package["name"] + "-" + package["version"])
            folder.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(notice, folder / notice.name)
inventory.sort(key=lambda entry: (entry["name"], entry["version"]))
(destination / "rust-dependencies.json").write_text(json.dumps(inventory, indent=2) + "\n")
PY

if [[ "$(uname -s)" == Darwin && ( -z "$target_triple" || "$target_triple" == *apple-darwin ) ]]; then
  app_directory="$dist_directory/ClipCrop.app"
  mkdir -p "$app_directory/Contents/MacOS" "$app_directory/Contents/Resources"
  cp "$dist_directory/clipcrop" "$app_directory/Contents/MacOS/clipcrop"
  cp LICENSE THIRD_PARTY.md "$app_directory/Contents/Resources/"
  cp -R "$dist_directory/third-party" "$app_directory/Contents/Resources/"
  icon_set="$project_root/.build/ClipCrop.iconset"
  mkdir -p "$project_root/.build/swift-module-cache"
  swift -module-cache-path "$project_root/.build/swift-module-cache" \
    "$project_root/scripts/make-icon.swift" "$icon_set"
  python3 "$project_root/scripts/make-icns.py" "$icon_set" \
    "$app_directory/Contents/Resources/ClipCrop.icns"
  cat > "$app_directory/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>ClipCrop</string>
  <key>CFBundleDisplayName</key><string>ClipCrop</string>
  <key>CFBundleIdentifier</key><string>local.clipcrop</string>
  <key>CFBundleExecutable</key><string>clipcrop</string>
  <key>CFBundleIconFile</key><string>ClipCrop.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
  if command -v codesign >/dev/null 2>&1; then
    codesign --force --sign - "$app_directory"
  fi
fi

printf 'Portable package created in %s\n' "$dist_directory"
printf 'See THIRD_PARTY.md before redistributing the bundled third-party probe.\n'
