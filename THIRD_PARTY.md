# Third-party software

ClipCrop's original Rust and interface source is covered by [LICENSE](LICENSE). Dependencies and bundled programs retain their original licenses.

## FFmpeg / ffprobe

The workspace's optional `vendor/ffprobe` is a macOS Apple Silicon static build of **FFmpeg/ffprobe 9.0.2**, supplied by [Martin Riedl's FFmpeg build service](https://ffmpeg.martin-riedl.de/). It identifies itself as:

```text
ffprobe version 9.0.2-https://www.martin-riedl.de
Copyright (c) 2007-2026 the FFmpeg developers
SHA-256: 2738aa46a7f9acbc8ab09a6715554c90f7de603171ac403726765685df6a0059
```

Its `ffprobe -L` output states **GNU GPL version 3 or later**, without warranty. It was built with `--enable-gpl` and `--enable-version3`; the GPL text is included at `vendor/ffprobe-LICENSE.txt` and copied into generated packages. ClipCrop executes this probe as a separate process and does not link against FFmpeg libraries.

Source and build information:

- [FFmpeg 9.0.2 source archive](https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz)
- [FFmpeg source mirror, tag n9.0.2](https://github.com/FFmpeg/FFmpeg/tree/n9.0.2)
- [Martin Riedl's build-script repository](https://git.martin-riedl.de/ffmpeg/build-script)
- [Build service and dependency list](https://ffmpeg.martin-riedl.de/)
- [FFmpeg license and legal information](https://www.ffmpeg.org/legal.html)

The upstream archive and current build-script repository are source references, not a complete verified corresponding-source bundle for this prebuilt binary and every statically included dependency. The exact build-script revision and all dependency versions have not been independently reconstructed here. Before publicly distributing a package containing this binary, obtain and provide the complete matching corresponding source and applicable dependency notices, or build a suitable probe from auditable source and retain its full build/source package. The packaging script records the actual selected probe's version/configuration and license report, but it does not resolve that remaining release obligation.

The selected default build reports this configuration:

```text
--prefix=/Volumes/ffmpeg_arm64/out --pkg-config-flags=--static
--extra-version=https://www.martin-riedl.de --enable-gray --enable-libxml2
--enable-version3 --enable-gpl --enable-openssl --enable-libfreetype
--enable-fontconfig --enable-libharfbuzz --enable-libbluray --enable-libsnappy
--enable-libsrt --enable-libvmaf --enable-libass --enable-libklvanc
--enable-libzimg --enable-libzvbi --enable-libaom --enable-libdav1d
--enable-libopenh264 --enable-libopenjpeg --enable-librav1e --enable-libsvtav1
--enable-libvpx --enable-libvvenc --enable-libwebp --enable-libx264
--enable-libx265 --enable-libmp3lame --enable-libopus --enable-libvorbis
--enable-libtheora
```

If `CLIPCROP_FFPROBE` selects a different binary, its license, source, CPU architecture, and build configuration must be checked independently. Do not assume this default-binary notice describes that replacement.

## Rust dependencies

`Cargo.lock` records the exact dependency graph. Principal runtime components include:

| Component | Purpose | Declared license |
| --- | --- | --- |
| [librqbit](https://docs.rs/librqbit/9.0.1/librqbit/) | BitTorrent transfers | Apache-2.0 |
| [libp2p](https://docs.rs/libp2p/0.57.1/libp2p/) | Peer discovery and gossip | MIT |
| [Wry](https://github.com/tauri-apps/wry) | Native WebView | Apache-2.0 OR MIT |
| [Tao](https://github.com/tauri-apps/tao) | Native windows and event loop | Apache-2.0 |
| [Axum](https://github.com/tokio-rs/axum) | Loopback HTTP interface | MIT |
| [Tokio](https://github.com/tokio-rs/tokio) | Async runtime | MIT |
| [Serde](https://github.com/serde-rs/serde) | State and protocol serialization | MIT OR Apache-2.0 |

The release packaging script generates `third-party/rust-dependencies.json` from Cargo metadata and copies top-level license/notice files exposed by the installed crate sources into `third-party/rust/`. This inventory includes transitive Cargo packages; it is informational and should be reviewed for the final target and enabled features. Some packages declare a license expression without shipping a top-level license file. Their declared terms still apply.

OS-provided frameworks, WebView runtimes, system fonts, and Linux system libraries are supplied by the operating system or its package manager and retain their own terms. They are not embedded by this project.
