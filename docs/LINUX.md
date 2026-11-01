# Recovered points fork on Linux

This is the v0.1.0 points recovery. Its normal profile is `$XDG_DATA_HOME/clipcrop-points` (default `~/.local/share/clipcrop-points`). The system package and launcher are `clipcrop-points`; the portable executable filename remains `clipcrop`. The historical baseline is preserved in Git. Linux fork binaries have not been rebuilt in this recovery; the instructions below describe how to build/package them. See [RECOVERY.md](../RECOVERY.md).

# Linux

ClipCrop uses a native GTK 3 / WebKitGTK 4.1 desktop window on Linux. Its interface, video validator, BitTorrent client, and peer mesh run in the same application process. No browser, Node.js, installed Rust toolchain, or separate FFmpeg installation is needed to run a bundled release.

## Packages

The x86-64 release targets Ubuntu 22.04 or a newer compatible desktop with GTK 3 and WebKitGTK 4.1. It contains an embedded Linux video validator; the OS WebView and GStreamer remain system libraries.

- `clipcrop-points-0.1.0-linux-x86_64.tar.gz`: portable archive.
- `clipcrop-points_0.1.0_amd64.deb`: Debian/Ubuntu package with a desktop-menu entry.
- `clipcrop-linux-x86_64-SHA256SUMS.txt`: artifact checksums.

On Debian/Ubuntu, install the `.deb` and its runtime dependencies with:

```sh
sudo apt install ./clipcrop-points_0.1.0_amd64.deb
clipcrop-points
```

All launch methods store your profile in `$XDG_DATA_HOME/clipcrop-points`, defaulting to `~/.local/share/clipcrop-points` when `XDG_DATA_HOME` is unset, empty, or relative. Removing the package does not remove that profile.

## Run a portable package

Extract the archive and run:

```sh
tar -xzf clipcrop-points-0.1.0-linux-x86_64.tar.gz
cd clipcrop-points-0.1.0-linux-x86_64
./clipcrop
```

Your identity, settings, points history, videos, and extracted validator live in the user data folder described above. Opening the app does not search for or create an adjacent profile in Downloads, and moving the executable preserves the same profile. Never share your profile's `identity.key`.

The packaged wrapper uses the same profile location:

```sh
./launch-clipcrop
```

Use `--data-dir PATH` with either executable or launcher to explicitly choose a profile folder, including a portable folder beside the executable.

This recovered fork starts a separate points profile; it does not import a profile from the original points app or the cryptocurrency app. To reuse old points data, make a separate copy of a version-1 points profile and open that copy with `--data-dir /path/to/points-profile-copy`. Cryptocurrency profiles are incompatible. Keep the original profile unchanged.

The executable is architecture-specific. Use the package matching your Linux machine; an x86-64 package does not run natively on ARM64.

## System libraries and video codecs

The desktop uses the distribution's GTK 3, WebKitGTK 4.1, and GStreamer libraries. These operating-system components are not statically embedded. On Debian/Ubuntu, install the runtime and common video codecs with:

```sh
sudo apt install libgtk-3-0 libwebkit2gtk-4.1-0 gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
```

The supported distro/glibc baseline is recorded in the package's build information. Distribution package names can differ. MP4/WebM are validated before playback, but the actual video/audio codec must also be available to WebKitGTK/GStreamer.

GTK selects X11 or Wayland according to your desktop session. ClipCrop uses Wry's GTK integration rather than its X11-only raw-window integration.

## Optional application-menu entry

A portable package can include `install-desktop.sh`. Run it from the package directory:

```sh
./install-desktop.sh
```

This creates a launcher for the current user only, under `$XDG_DATA_HOME/applications`, defaulting to `~/.local/share/applications`. It points to the extracted application, so keep that folder in place. The application resolves the same user data profile folder on every launch to avoid writing into the application folder. It needs Python 3 only while creating the launcher.

## Build on Linux

Install Rust and the native development libraries. On Debian/Ubuntu:

```sh
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev python3 meson ninja-build xz-utils
```

The release validator is a small, separate FFmpeg 9.0.2 `ffprobe` built from upstream source, with the BSD-licensed dav1d 1.5.3 AV1 decoder built from its upstream source. Its build enables only local files, MP4/WebM demuxing, and the accepted video/audio decoders. FFmpeg and dav1d are linked into the validator; libc and libm use the operating system so they receive normal system updates. Networking and automatically detected optional dependencies are disabled. The package includes its source archive, build recipe, exact configure arguments, applicable license texts, and static decoder notices under `third-party/`.

Build the validator on Linux using `scripts/build-linux-ffprobe.sh`, then embed the resulting executable in the Rust release:

```sh
CLIPCROP_FFMPEG_SOURCE=/absolute/path/to/ffmpeg-9.0.2.tar.xz \
CLIPCROP_DAV1D_SOURCE=/absolute/path/to/dav1d-1.5.3.tar.xz \
CLIPCROP_FFMPEG_BUILD_DIR="$PWD/.build/linux-ffmpeg-build" \
CLIPCROP_BUILD_JOBS=2 ./scripts/build-linux-ffprobe.sh
CLIPCROP_FFPROBE="$PWD/.build/linux-ffprobe/ffprobe" cargo build --locked --release --features bundle-ffprobe
```

The builder verifies the pinned source archive checksums and writes the validator, license reports, and build records under `.build/linux-ffprobe`. `CLIPCROP_FFPROBE_OUTPUT` can override that output location. Use a trusted source archive matching the recorded build information.

`scripts/package-linux.sh` packages an already-built Linux x86-64 executable without overwriting the macOS release. Its primary inputs are:

```sh
export CLIPCROP_LINUX_BINARY="$PWD/target/release/clipcrop"
export CLIPCROP_LINUX_FFPROBE="$PWD/.build/linux-ffprobe/ffprobe"
export CLIPCROP_LINUX_PROBE_VERSION_FILE="$PWD/.build/linux-ffprobe/version.txt"
export CLIPCROP_LINUX_PROBE_LICENSE_REPORT="$PWD/.build/linux-ffprobe/license-report.txt"
export CLIPCROP_LINUX_PROBE_LICENSE_FILE="$PWD/.build/linux-ffprobe/LICENSE.LGPLv2.1"
./scripts/package-linux.sh
```

It verifies both ELF architectures and that the selected validator bytes are embedded in the application. Release packaging also supplies `CLIPCROP_LINUX_PROBE_SOURCE_ARCHIVE`, `CLIPCROP_LINUX_PROBE_BUILD_SCRIPT`, `CLIPCROP_LINUX_PROBE_CONFIGURE_ARGS` (a file path or literal arguments), and `CLIPCROP_LINUX_PROBE_DAV1D_COPYRIGHT`. Optional decoder source/debian-patch archives and other license/source files can be included through the corresponding `DAV1D_SOURCE_ARCHIVE`, `DAV1D_DEBIAN_PATCHES`, and newline-separated `EXTRA_SOURCE_FILES` variables with the same `CLIPCROP_LINUX_PROBE_` prefix. `CLIPCROP_LINUX_THIRD_PARTY_NOTICE` supplies the matching Linux notice; `CLIPCROP_LINUX_LDD_REPORT` records linked system libraries.

For a development build using an installed validator:

```sh
CLIPCROP_FFPROBE=/usr/bin/ffprobe cargo run
```

The default peer mesh and BitTorrent ports are TCP 42931 and 42932. Local-network discovery uses mDNS. For peers on other networks, enter a reachable peer address in Settings and arrange any necessary port forwarding. There is no hosted discovery or relay service.

Official platform references: [Wry's Linux requirements](https://github.com/tauri-apps/wry#linux-dependencies) and [GTK integration example](https://docs.rs/wry/0.57.1/wry/#examples).
