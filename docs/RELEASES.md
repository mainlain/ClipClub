# Releases and app updates

The source repository and release downloads are public so installed apps can check GitHub and download packages without user accounts, embedded tokens, or a separate update service. The app reads `https://api.github.com/repos/mainlain/ClipClub/releases/latest`, selects a package for its OS/CPU, and verifies the SHA-256 asset digest supplied by GitHub before preparing an installation. The separately published manifest/checksum files provide an audit record; the app does not trust a user-provided update URL.

Checks run at startup and every six hours, with a manual check in Settings. Choosing **Install and restart** updates a writable macOS app bundle or portable Linux installation. A staged helper replaces the application after it exits and restores the previous app if replacement fails; the existing profile is retained. A direct macOS executable and system-managed Linux installation such as `.deb` use the browser download/manual installation path. No release is required during setup, and publishing a new stable version advertises it to the next successful check.

## Publish a version

1. Change `[package].version` in `Cargo.toml` and update the matching root package version in `Cargo.lock` with Cargo. Commit and push the completed change.
2. Create a tag matching that version, for example `git tag v0.2.0`, then push that tag with `git push origin v0.2.0`.
3. The **Build and publish release** workflow checks the tag against `Cargo.toml`, builds native Apple Silicon macOS and Linux x86-64 packages, runs the application checks and packaged-app smoke test, and collects all artifacts.
4. After all builds pass, the workflow stages every asset in a GitHub draft release and publishes it. GitHub selects the newest stable release using its creation-date/semantic-version ordering, so an older tag build finishing later does not force itself to latest. Tags with a prerelease suffix, such as `v0.2.0-rc.1`, are published as prereleases and are excluded from stable app updates.

No release signing secret or GitHub personal token is needed. The publishing job uses the repository-scoped Actions token with `contents: write`; build jobs have read-only repository access. The workflow refuses to replace an already published release. Use a new version/tag for a changed binary. A failed publish can be retried while its release is still a draft.

The macOS bundles are ad hoc signed and verified, without Developer ID notarization. First-time downloads remain subject to macOS's normal launch controls.

## Verify builds without publishing

Run the workflow manually from the desired branch with `tag` set to the version in that branch's `Cargo.toml` (for example `v0.1.0`) and leave `publish` false. A build-only run does not require that tag to exist. The two native build artifacts and complete release manifest are available on the Actions run. To publish from a manual run, select the exact existing version tag as the workflow revision and set `publish` true.

Local release-tool checks are:

```sh
python3 scripts/release.test.py
bash -n scripts/build-release-ffprobe.sh scripts/package.sh scripts/package-linux.sh
```

`scripts/check.sh` covers Rust and interface checks. Every release package also runs `scripts/package-smoke.py` against its real bundled binary using disposable profiles, checking startup, embedded validator extraction, and profile persistence.

## Published assets

For version `0.2.0`, the assets are:

| Target | Asset | Archive layout |
| --- | --- | --- |
| macOS Apple Silicon | `clipclub-0.2.0-macos-aarch64.zip` | `clipclub.app/` at the archive root |
| Linux x86-64 portable | `clipclub-0.2.0-linux-x86_64.tar.gz` | `clipclub-0.2.0-linux-x86_64/` at the archive root |
| Debian/Ubuntu x86-64 | `clipclub_0.2.0_amd64.deb` | System package under `/opt/clipclub` with desktop entry |

`update-manifest.json` records the version, tag, target, filename, size, SHA-256, and GitHub download URL of every package. `SHA256SUMS.txt` records package and manifest digests. Asset names and archive roots are part of the app updater contract and must remain synchronized with `src/update.rs` and `src/update_install.rs`.

Mac ZIP files preserve Unix executable modes and contain only the app tree. They exclude resource-fork metadata and symbolic links. Linux packages record the version and target in `BUILD-INFO.json`. The installer checks these structures, package limits, version, and native architecture before replacing application files. On macOS it also verifies the app's code signature.

## Validator source and notices

Release CI builds the native validator with `scripts/build-release-ffprobe.sh`, using:

| Source | Pinned SHA-256 |
| --- | --- |
| [FFmpeg 9.0.2](https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz) | `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e` |
| [dav1d 1.5.3](https://download.videolan.org/pub/videolan/dav1d/1.5.3/dav1d-1.5.3.tar.xz) | `732010aa5ef461fa93355ed2c6c5fedb48ddc4b74e697eaabe8907eaeb943011` |

The builder enables only local-file MP4/WebM demuxing and the supported decoders, disables automatic dependency detection, and statically includes FFmpeg and the BSD-licensed dav1d decoder in the separate-process validator. It checks that no dynamic FFmpeg/dav1d libraries were selected. The operating system supplies its normal runtime libraries.

Every release contains both complete source archives, the native build script, exact configure arguments, and source checksums under `third-party/source/`. The package also includes the LGPLv2.1-or-later text, dav1d notice, actual validator version/license reports, and Rust dependency notices. In a macOS app these files are under `clipclub.app/Contents/Resources/`.

To reproduce the validator on the same native OS/CPU, install `cc`, `make`, `pkg-config`, `meson`, `ninja`, `tar`, and `xz` (Xcode Command Line Tools on macOS), then run the included `third-party/source/build-release-ffprobe.sh`. It locates the source archives beside itself. Its build/output directories can be selected with `CLIPCROP_FFMPEG_BUILD_DIR` and `CLIPCROP_FFPROBE_OUTPUT`. The exact upstream archive checksums and configure arguments remain pinned in the recipe.

The optional workspace `vendor/ffprobe` is used only for local development packaging. Release CI uses the audited source builder and does not ship that prebuilt vendor executable.
