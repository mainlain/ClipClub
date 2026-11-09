# Linux

clipclub uses a native GTK 3 / WebKitGTK 4.1 desktop window on Linux. Its interface, video validator, BitTorrent client, and peer mesh run in the same application process. No browser, Node.js, installed Rust toolchain, or separate FFmpeg installation is needed to run a bundled release.

## Packages

The x86-64 release targets Ubuntu 22.04 or a newer compatible desktop with GTK 3 and WebKitGTK 4.1. It contains an embedded Linux video validator; the OS WebView and GStreamer remain system libraries.

- `clipclub-0.1.0-linux-x86_64.tar.gz`: portable archive.
- `clipclub_0.1.0_amd64.deb`: Debian/Ubuntu package with a desktop-menu entry.
- `clipclub-linux-x86_64-SHA256SUMS.txt`: artifact checksums.

On Debian/Ubuntu, install the `.deb` and its runtime dependencies with:

```sh
sudo apt install ./clipclub_0.1.0_amd64.deb
clipclub
```

All launch methods store your profile in `$XDG_DATA_HOME/clipcrop-points`, defaulting to `~/.local/share/clipcrop-points` when `XDG_DATA_HOME` is unset, empty, or relative. Removing the package does not remove that profile.

## Run a portable package

Extract the archive and run:

```sh
tar -xzf clipclub-0.1.0-linux-x86_64.tar.gz
cd clipclub-0.1.0-linux-x86_64
./clipclub
```

Your identity, points history, videos, and extracted validator live in the user data folder described above. Opening the app does not search for or create an adjacent profile in Downloads, and moving the executable preserves the same profile. Never share your profile's `identity.key`.

The packaged wrapper uses the same profile location:

```sh
./launch-clipclub
```

Use `--data-dir PATH` with either executable or launcher to explicitly choose a profile folder, including a portable folder beside the executable.

The executable is architecture-specific. Use the package matching your Linux machine; an x86-64 package does not run natively on ARM64.

## System libraries and video codecs

The desktop uses the distribution's GTK 3, WebKitGTK 4.1, and GStreamer libraries. These operating-system components are not statically embedded. On Debian/Ubuntu, install the runtime and common video codecs with:

```sh
sudo apt install libgtk-3-0 libwebkit2gtk-4.1-0 gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
```

The supported distro/glibc baseline is recorded in the package's build information. Distribution package names can differ. MP4/WebM are validated before playback, but the actual video/audio codec must also be available to WebKitGTK/GStreamer.

GTK selects X11 or Wayland according to your desktop session. clipclub uses Wry's GTK integration rather than its X11-only raw-window integration.

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
export CLIPCROP_LINUX_BINARY="$PWD/target/release/clipclub"
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

The default peer mesh and BitTorrent ports are TCP 42931 and 42932. Peers connect automatically through mDNS on the local network and the public BitTorrent Mainline DHT across the internet; there is no peer-address entry. Allow local multicast for mDNS and outbound UDP for DHT discovery. The app attempts UPnP forwarding for both TCP ports on supported routers. Restrictive NAT, carrier-grade NAT, firewalls, or disabled UPnP can still require router configuration to make those ports reachable. There is no hosted relay or guaranteed NAT traversal.

### Checking local discovery

The portable application and Debian installation use the same networking code. Keep ClipClub open on both devices, check that Settings shows different peer IDs, and compare their versions. Local discovery uses IPv4 mDNS on UDP 5353, followed by a connection to the other device's peer TCP port. The BitTorrent TCP port is needed for video transfer. A device can receive a discovery announcement while a firewall still blocks the subsequent TCP connection.

Run the diagnostic script on each Linux device, replacing the example address with the other device's Wi-Fi IPv4 address:

```sh
python3 scripts/diagnose-discovery.py --peer 192.168.1.20
```

The script reports local interfaces and listeners, probes mDNS for libp2p announcements, and tests the explicitly supplied peer's TCP ports. It uses Python's standard library, requires no root privileges, and makes no firewall or router changes. Use `--help` for custom ports. A successful multicast send alone does not establish that the other device received it, and an empty discovery result does not identify which device or network component blocked it. Compare both devices' results before changing firewall rules.

Local discovery needs multicast delivery between the devices. Guest Wi-Fi, access-point client isolation, separate subnets, VPN routing, an exclusive mDNS listener, or firewall rules can prevent it even when both devices show the same Wi-Fi name. Avahi is not required: ClipClub implements its own mDNS discovery. Its [configuration](https://github.com/avahi/avahi/blob/master/man/avahi-daemon.conf.5.xml.in) can, however, reserve UDP 5353 exclusively and prevent another mDNS implementation from binding that port.

Official platform references: [Wry's Linux requirements](https://github.com/tauri-apps/wry#linux-dependencies) and [GTK integration example](https://docs.rs/wry/0.57.1/wry/#examples).
