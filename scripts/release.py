#!/usr/bin/env python3
"""Build native release packages and record the exact published asset digests."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import struct
import subprocess
import tempfile
from urllib.parse import quote
import zipfile


ROOT = Path(__file__).resolve().parent.parent
TARGETS = {
    'aarch64-apple-darwin': ('Darwin', 'arm64', 'macos-aarch64', '.zip'),
    'x86_64-apple-darwin': ('Darwin', 'x86_64', 'macos-x86_64', '.zip'),
    'x86_64-unknown-linux-gnu': ('Linux', 'x86_64', 'linux-x86_64', '.tar.gz'),
}
SEMVER_CORE = r'(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)'
PRERELEASE_IDENTIFIER = r'(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)'
BUILD_IDENTIFIER = r'[0-9A-Za-z-]+'
SEMVER = (
    SEMVER_CORE
    + rf'(?:-{PRERELEASE_IDENTIFIER}(?:\.{PRERELEASE_IDENTIFIER})*)?'
    + rf'(?:\+{BUILD_IDENTIFIER}(?:\.{BUILD_IDENTIFIER})*)?'
)


def release_version(tag=None, root=ROOT):
    text = (root / 'Cargo.toml').read_text()
    package = re.search(r'^\[package\]\s*\n(.*?)(?=^\[|\Z)', text, re.M | re.S)
    match = re.search(r'^version\s*=\s*"([^"]+)"', package[1], re.M) if package else None
    if not match or not re.fullmatch(SEMVER, match[1]):
        raise ValueError('Cargo.toml must contain a valid package version')
    version = match[1]
    if tag is not None and tag != 'v' + version:
        raise ValueError(f'Release tag {tag!r} must match Cargo.toml version v{version}')
    return version


def asset_name(version, target):
    _, _, suffix, extension = TARGETS[target]
    return f'clipclub-{version}-{suffix}{extension}'


def digest(path):
    checksum = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            checksum.update(chunk)
    return checksum.hexdigest()


def check_binary(path, target):
    with path.open('rb') as binary:
        header = binary.read(20)
    if target.endswith('apple-darwin'):
        cpu = 0x0100000C if target.startswith('aarch64') else 0x01000007
        valid = len(header) >= 8 and header[:4] == b'\xcf\xfa\xed\xfe' and struct.unpack('<I', header[4:8])[0] == cpu
    else:
        valid = len(header) >= 20 and header[:6] == b'\x7fELF\x02\x01' and struct.unpack('<H', header[18:20])[0] == 62
    if not valid:
        raise ValueError(f'{path} is not a native {target} executable')


def archive_bundle(bundle, destination):
    # Preserve Unix executable bits without __MACOSX/resource-fork entries.
    # The installer deliberately refuses links and unexpected archive roots.
    with zipfile.ZipFile(destination, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
        for path in [bundle, *sorted(bundle.rglob('*'))]:
            if path.is_symlink():
                raise ValueError(f'Release bundles may not contain symlinks: {path}')
            archive.write(path, path.relative_to(bundle.parent))


def release_notice(probe, target):
    rust_section = (ROOT / 'THIRD_PARTY.md').read_text().split('## Rust dependencies', 1)[1]
    return (
        '# Third-party software\n\n'
        'clipclub source is MIT licensed; see LICENSE. Other programs retain their own licenses.\n\n'
        '## Release FFmpeg / ffprobe\n\n'
        f'This {target} package embeds a separate-process FFmpeg 9.0.2 validator with dav1d 1.5.3. '
        f'The validator SHA-256 is `{digest(probe / "ffprobe")}`. '
        'It is built from the included pinned upstream source, with local-file MP4/WebM validation '
        'and the supported decoders. Automatically detected dependencies and network protocols are disabled. '
        'FFmpeg and the BSD-licensed dav1d decoder are linked into the validator; operating-system libraries remain system supplied. '
        'clipclub executes the validator as a separate process.\n\n'
        'The complete FFmpeg and dav1d source archives, build recipe, exact configure arguments, '
        'and checksums are retained under `third-party/source/`. '
        'The applicable LGPLv2.1-or-later license is in `third-party/ffprobe-LICENSE.txt`, '
        'dav1d copyright/license is in `third-party/dav1d-COPYRIGHT.txt`, and the validator reports '
        'its actual version/configuration and license in `third-party/`.\n\n'
        '- FFmpeg source: https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz\n'
        '- dav1d source: https://download.videolan.org/pub/videolan/dav1d/1.5.3/dav1d-1.5.3.tar.xz\n'
        '- Build recipe: `third-party/source/build-release-ffprobe.sh`\n\n'
        '## Rust dependencies' + rust_section
    )


def package_release(args):
    version = release_version(args.tag)
    system, machine, _, _ = TARGETS[args.target]
    if platform.system() != system or platform.machine() != machine:
        raise ValueError(f'Build {args.target} on a native {system} {machine} runner')
    probe = args.probe_dir.resolve(strict=True)
    for name in ('ffprobe', 'LICENSE.LGPLv2.1', 'dav1d-copyright.txt',
                 'version.txt', 'license-report.txt', 'configure-arguments.txt',
                 'source/ffmpeg-9.0.2.tar.xz', 'source/dav1d-1.5.3.tar.xz',
                 'source/build-release-ffprobe.sh'):
        if not (probe / name).is_file():
            raise ValueError(f'Missing audited validator input: {probe / name}')
    check_binary(probe / 'ffprobe', args.target)
    if 'Lesser General Public License' not in ' '.join((probe / 'license-report.txt').read_text().split()):
        raise ValueError('The release validator must report the LGPL license matching its included source')
    args.output.mkdir(parents=True, exist_ok=True)
    (ROOT / '.build').mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='release-package-', dir=ROOT / '.build') as temporary:
        work = Path(temporary)
        stage = work / 'package'
        notice = work / 'THIRD_PARTY.md'
        notice.write_text(release_notice(probe, args.target))
        environment = dict(os.environ)
        environment.update({
            'CLIPCROP_TARGET': args.target, 'CLIPCROP_FFPROBE': str(probe / 'ffprobe'),
            'CLIPCROP_DIST_DIR': str(stage), 'CLIPCROP_THIRD_PARTY_NOTICE': str(notice),
            'CLIPCROP_PROBE_LICENSE_FILE': str(probe / 'LICENSE.LGPLv2.1'),
            'CLIPCROP_PROBE_SOURCE_DIR': str(probe / 'source'),
            'CLIPCROP_PROBE_DAV1D_NOTICE': str(probe / 'dav1d-copyright.txt'),
        })
        subprocess.run(['bash', str(ROOT / 'scripts/package.sh')], cwd=ROOT, env=environment, check=True)
        check_binary(stage / 'clipclub', args.target)
        # Exercise the real embedded validator and persistence before publishing.
        subprocess.run(['python3', str(ROOT / 'scripts/package-smoke.py'), str(stage / 'clipclub')],
                       cwd=ROOT, env=environment, check=True)
        name = asset_name(version, args.target)
        if system == 'Darwin':
            bundle = stage / 'clipclub.app'
            subprocess.run(['codesign', '--verify', '--deep', '--strict', str(bundle)], check=True)
            archive_bundle(bundle, work / name)
            shutil.copyfile(work / name, args.output / name)
        else:
            target_directory = Path(environment.get('CARGO_TARGET_DIR', str(ROOT / 'target')))
            if not target_directory.is_absolute():
                target_directory = ROOT / target_directory
            environment.update({
                'CLIPCROP_LINUX_BINARY': str(target_directory / args.target / 'release/clipclub'),
                'CLIPCROP_LINUX_FFPROBE': str(probe / 'ffprobe'),
                'CLIPCROP_LINUX_PROBE_VERSION_FILE': str(probe / 'version.txt'),
                'CLIPCROP_LINUX_PROBE_LICENSE_REPORT': str(probe / 'license-report.txt'),
                'CLIPCROP_LINUX_PROBE_LICENSE_FILE': str(probe / 'LICENSE.LGPLv2.1'),
                'CLIPCROP_LINUX_PROBE_SOURCE_ARCHIVE': str(probe / 'source/ffmpeg-9.0.2.tar.xz'),
                'CLIPCROP_LINUX_PROBE_DAV1D_SOURCE_ARCHIVE': str(probe / 'source/dav1d-1.5.3.tar.xz'),
                'CLIPCROP_LINUX_PROBE_BUILD_SCRIPT': str(probe / 'source/build-release-ffprobe.sh'),
                'CLIPCROP_LINUX_PROBE_CONFIGURE_ARGS': str(probe / 'configure-arguments.txt'),
                'CLIPCROP_LINUX_PROBE_DAV1D_COPYRIGHT': str(probe / 'dav1d-copyright.txt'),
                'CLIPCROP_LINUX_THIRD_PARTY_NOTICE': str(notice),
            })
            subprocess.run(['bash', str(ROOT / 'scripts/package-linux.sh')], cwd=ROOT, env=environment, check=True)
            for filename in (name, f'clipclub_{version}_amd64.deb'):
                shutil.copyfile(stage / filename, args.output / filename)
        print(args.output / name)


def write_manifest(args):
    version = release_version(args.tag)
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', args.repository):
        raise ValueError('Use a GitHub owner/repository name')
    names = [(target, asset_name(version, target), 'portable') for target in TARGETS]
    names.append(('x86_64-unknown-linux-gnu', f'clipclub_{version}_amd64.deb', 'deb'))
    assets = []
    for target, name, kind in names:
        path = args.directory / name
        if not path.is_file() or path.is_symlink() or path.stat().st_size == 0:
            raise ValueError(f'Missing complete release asset: {path}')
        assets.append({
            'target': target, 'kind': kind, 'name': name,
            'sha256': digest(path), 'size': path.stat().st_size,
            'url': f'https://github.com/{args.repository}/releases/download/{quote(args.tag, safe="")}/{quote(name, safe="")}',
        })
    manifest = {'version': version, 'tag': args.tag, 'assets': assets}
    (args.directory / 'update-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    checksum_paths = [args.directory / entry['name'] for entry in assets]
    checksum_paths.append(args.directory / 'update-manifest.json')
    (args.directory / 'SHA256SUMS.txt').write_text(''.join(
        f'{digest(path)}  {path.name}\n' for path in checksum_paths))
    print(args.directory / 'update-manifest.json')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    version_parser = commands.add_parser('version')
    version_parser.add_argument('--tag', required=True)
    package = commands.add_parser('package')
    package.add_argument('--tag', required=True)
    package.add_argument('--target', choices=TARGETS, required=True)
    package.add_argument('--probe-dir', type=Path, required=True)
    package.add_argument('--output', type=Path, default=ROOT / 'dist/releases')
    manifest = commands.add_parser('manifest')
    manifest.add_argument('--tag', required=True)
    manifest.add_argument('--repository', required=True)
    manifest.add_argument('--directory', type=Path, default=ROOT / 'dist/releases')
    args = parser.parse_args()
    try:
        if args.command == 'version':
            print(release_version(args.tag))
        elif args.command == 'package':
            package_release(args)
        else:
            write_manifest(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'release: {error}\n')


if __name__ == '__main__':
    main()
