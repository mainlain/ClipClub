#!/usr/bin/env bash
set -euo pipefail

# Package an already-built Linux desktop binary. This script never builds code
# and never overwrites the macOS executable, app bundle, or portable profile.
project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"

if ! command -v python3 >/dev/null 2>&1; then
  printf 'Linux packaging needs Python 3.\n' >&2
  exit 1
fi

binary_path="${CLIPCROP_LINUX_BINARY:-$project_root/target/x86_64-unknown-linux-gnu/release/clipcrop}"
probe_path="${CLIPCROP_LINUX_FFPROBE:-}"
probe_version_file="${CLIPCROP_LINUX_PROBE_VERSION_FILE:-}"
probe_license_report="${CLIPCROP_LINUX_PROBE_LICENSE_REPORT:-}"
probe_license_file="${CLIPCROP_LINUX_PROBE_LICENSE_FILE:-$project_root/vendor/ffprobe-LICENSE.txt}"
for required in "$binary_path" "$probe_path" "$probe_version_file" "$probe_license_report" "$probe_license_file"; do
  if [[ -z "$required" || ! -f "$required" ]]; then
    printf 'Missing Linux package input: %s\n' "${required:-unset}" >&2
    printf 'Set CLIPCROP_LINUX_BINARY, CLIPCROP_LINUX_FFPROBE, CLIPCROP_LINUX_PROBE_VERSION_FILE and CLIPCROP_LINUX_PROBE_LICENSE_REPORT.\n' >&2
    exit 1
  fi
done

export CLIPCROP_LINUX_BINARY="$binary_path"
export CLIPCROP_LINUX_FFPROBE="$probe_path"
export CLIPCROP_LINUX_PROBE_VERSION_FILE="$probe_version_file"
export CLIPCROP_LINUX_PROBE_LICENSE_REPORT="$probe_license_report"
export CLIPCROP_LINUX_PROBE_LICENSE_FILE="$probe_license_file"
export CLIPCROP_PACKAGE_ROOT="$project_root"
python3 <<'PY'
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import struct
import tarfile
import tempfile
import time

root = Path(os.environ['CLIPCROP_PACKAGE_ROOT'])
source = Path(os.environ['CLIPCROP_LINUX_BINARY']).resolve(strict=True)
probe = Path(os.environ['CLIPCROP_LINUX_FFPROBE']).resolve(strict=True)
version_report = Path(os.environ['CLIPCROP_LINUX_PROBE_VERSION_FILE']).read_text()
license_report = Path(os.environ['CLIPCROP_LINUX_PROBE_LICENSE_REPORT']).read_text()
license_source = Path(os.environ['CLIPCROP_LINUX_PROBE_LICENSE_FILE'])
version = os.environ.get('CLIPCROP_VERSION') or re.search(
    r'^version\s*=\s*"([^"]+)"', (root / 'Cargo.toml').read_text(), re.M)[1]
if not re.fullmatch(r'[0-9][0-9A-Za-z.+~:-]*', version):
    raise SystemExit('Invalid package version')

# Reject a mistaken macOS binary or wrong CPU before touching package outputs.
for path, label in [(source, 'application'), (probe, 'validator')]:
    header = path.open('rb').read(20)
    if (len(header) < 20 or header[:6] != b'\x7fELF\x02\x01'
            or struct.unpack('<H', header[16:18])[0] not in (2, 3)
            or struct.unpack('<H', header[18:20])[0] != 62):
        raise SystemExit(f'The {label} must be a 64-bit Linux x86-64 ELF file: {path}')
if probe.read_bytes() not in source.read_bytes():
    raise SystemExit('The selected Linux validator is not embedded in the application binary')
if not version_report.startswith('ffprobe version ') or 'Apple clang' in version_report:
    raise SystemExit('Provide the version report from the selected Linux validator')
license_summary = ' '.join(license_report.split())
if 'General Public License' not in license_summary:
    raise SystemExit('Provide the selected validator\'s actual LGPL/GPL license report')
full_license = license_source.read_text()
required_license = 'GNU LESSER GENERAL PUBLIC LICENSE' if 'Lesser General Public License' in license_summary else 'GNU GENERAL PUBLIC LICENSE'
if required_license not in full_license:
    raise SystemExit('The full validator license text does not match its actual license report')

stamp = int(os.environ.get('SOURCE_DATE_EPOCH', str(int(time.time()))))
baseline = os.environ.get('CLIPCROP_LINUX_BASELINE', 'Ubuntu 22.04 LTS; glibc 2.35; x86-64')
source_url = os.environ.get('CLIPCROP_LINUX_PROBE_SOURCE_URL', 'https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz')
probe_url = os.environ.get('CLIPCROP_LINUX_PROBE_DOWNLOAD_URL', source_url)

# Stage in a temporary directory so an invalid/failed package does not replace
# a previously completed Linux folder. Existing user profiles remain untouched.
output_parent = root / 'dist'
output_parent.mkdir(exist_ok=True)
(root / '.build').mkdir(exist_ok=True)
work = Path(tempfile.mkdtemp(prefix='clipcrop-linux-package-', dir=root / '.build'))
folder = work / 'linux-x86_64'
folder.mkdir()
try:
    shutil.copyfile(source, folder / 'clipcrop')
    (folder / 'clipcrop').chmod(0o755)
    for source_name, target_name in [('LICENSE', 'LICENSE'), ('README.md', 'README.md'),
                                    ('docs/LINUX.md', 'LINUX.md'),
                                    ('assets/icon.svg', 'clipcrop.svg'),
                                    ('scripts/install-linux-desktop.sh', 'install-desktop.sh')]:
        shutil.copyfile(root / source_name, folder / target_name)
    (folder / 'install-desktop.sh').chmod(0o755)
    (folder / 'launch-clipcrop').write_text(
        '#!/bin/sh\nset -eu\n'
        'application_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\n'
        'exec "$application_directory/clipcrop" "$@"\n')
    (folder / 'launch-clipcrop').chmod(0o755)

    third_party = folder / 'third-party'
    third_party.mkdir()
    rust_notices = output_parent / 'third-party' / 'rust'
    if not rust_notices.is_dir():
        raise SystemExit('Rust dependency notices are missing; run scripts/package.sh first or provide dist/third-party/rust')
    shutil.copytree(rust_notices, third_party / 'rust')
    shutil.copyfile(output_parent / 'third-party/rust-dependencies.json', third_party / 'rust-dependencies.json')
    shutil.copyfile(license_source, third_party / 'ffprobe-LICENSE.txt')
    (third_party / 'ffprobe-version.txt').write_text(version_report)
    (third_party / 'ffprobe-license-report.txt').write_text(license_report)


    # Keep corresponding source/build inputs with this exact validator. Each
    # optional input is a real file; no network download happens while packaging.
    source_records = []
    source_directory = third_party / 'source'
    def copy_source(environment_name, filename=None):
        value = os.environ.get(environment_name)
        if not value:
            return
        path = Path(value).resolve(strict=True)
        if not path.is_file():
            raise SystemExit(f'{environment_name} must point to a file')
        source_directory.mkdir(exist_ok=True)
        destination = source_directory / (filename or path.name)
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if destination.exists() and hashlib.sha256(destination.read_bytes()).hexdigest() != digest:
            raise SystemExit(f'Conflicting source archive filename: {destination.name}')
        shutil.copyfile(path, destination)
        if not any(entry['filename'] == destination.name for entry in source_records):
            source_records.append({'filename': destination.name, 'sha256': digest})

    copy_source('CLIPCROP_LINUX_PROBE_SOURCE_ARCHIVE')
    copy_source('CLIPCROP_LINUX_PROBE_BUILD_SCRIPT', 'build-linux-ffprobe.sh')
    copy_source('CLIPCROP_LINUX_PROBE_DAV1D_SOURCE_ARCHIVE')
    copy_source('CLIPCROP_LINUX_PROBE_DAV1D_DEBIAN_PATCHES')
    configure_args = os.environ.get('CLIPCROP_LINUX_PROBE_CONFIGURE_ARGS')
    if configure_args:
        source_directory.mkdir(exist_ok=True)
        try:
            arguments_file = Path(configure_args)
            is_arguments_file = arguments_file.is_file()
        except OSError:
            is_arguments_file = False
        if is_arguments_file:
            configure_text = arguments_file.read_text()
        else:
            configure_text = configure_args + '\n'
        configured = source_directory / 'ffprobe-configure-args.txt'
        configured.write_text(configure_text)
        source_records.append({'filename': configured.name, 'sha256': hashlib.sha256(configured.read_bytes()).hexdigest()})
    dav1d_notice = os.environ.get('CLIPCROP_LINUX_PROBE_DAV1D_COPYRIGHT')
    if dav1d_notice:
        shutil.copyfile(dav1d_notice, third_party / 'dav1d-COPYRIGHT.txt')
    for value in os.environ.get('CLIPCROP_LINUX_PROBE_EXTRA_SOURCE_FILES', '').splitlines():
        if value.strip():
            # Reuse the same collision detection without changing global env.
            extra_key = 'CLIPCROP_PACKAGE_EXTRA_SOURCE_FILE'
            os.environ[extra_key] = value
            copy_source(extra_key)
    if source_records:
        (source_directory / 'SHA256SUMS.txt').write_text(''.join(
            f"{entry['sha256']}  {entry['filename']}\n" for entry in source_records))

    # Deliberately do not copy the macOS ffprobe notice into this Linux package.
    existing_notice = (root / 'THIRD_PARTY.md').read_text()
    rust_section = existing_notice[existing_notice.index('## Rust dependencies'):]
    custom_notice = os.environ.get('CLIPCROP_LINUX_THIRD_PARTY_NOTICE')
    if custom_notice:
        notice = Path(custom_notice).read_text()
        if '## Rust dependencies' not in notice:
            notice += '\n\n' + rust_section
    else:
        notice = (
            '# Third-party software\n\n'
            'ClipCrop\'s original source is MIT licensed; see LICENSE. Other programs retain their own licenses.\n\n'
            '## Linux FFmpeg / ffprobe\n\n'
            f'This Linux package embeds the separate-process validator identified in `third-party/ffprobe-version.txt`. '
            f'The selected x86-64 executable has SHA-256 `{hashlib.sha256(probe.read_bytes()).hexdigest()}`. '
            'It is extracted to the private profile folder when needed and is not linked as an FFmpeg library.\n\n'
            'The validator\'s applicable LGPL/GPL license text, actual `-L` report, and exact '
            'version/configuration report are included in `third-party/`. The selected build\'s report determines '
            'its license; the macOS validator notice does not apply to this Linux package.\n\n'
            f'- Build input / download: {probe_url}\n'
            f'- Source archive: {source_url}\n'
            '- FFmpeg license information: https://ffmpeg.org/legal.html\n\n' + rust_section)
    (folder / 'THIRD_PARTY.md').write_text(notice)
    build_info = {
        'application': 'clipcrop-points', 'version': version, 'target': 'x86_64-unknown-linux-gnu',
        'baseline': baseline, 'application_sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
        'ffprobe_sha256': hashlib.sha256(probe.read_bytes()).hexdigest(),
        'ffprobe_version': version_report.splitlines()[0],
        'ffprobe_source': source_url,
        'ffprobe_corresponding_source': source_records,
        'embedded': ['interface assets', 'ffprobe video validator'],
        'system_runtime': ['GTK 3', 'WebKitGTK 4.1', 'GStreamer with playback codecs'],
        'profile': 'All launch methods: XDG_DATA_HOME/clipcrop-points, default HOME/.local/share/clipcrop-points; --data-dir overrides',
    }
    ldd_report = os.environ.get('CLIPCROP_LINUX_LDD_REPORT')
    if ldd_report:
        shutil.copyfile(ldd_report, folder / 'linked-libraries.txt')
    (folder / 'BUILD-INFO.json').write_text(json.dumps(build_info, indent=2) + '\n')

    # Build a portable tar archive with a versioned top-level folder.
    archive_name = f'clipcrop-points-{version}-linux-x86_64'
    portable = work / f'{archive_name}.tar.gz'
    def filter_tar(info):
        info.uid = info.gid = 0
        info.uname = info.gname = 'root'
        info.mtime = stamp
        return info
    with portable.open('wb') as output:
        with gzip.GzipFile(fileobj=output, mode='wb', mtime=stamp, filename='') as compressed:
            with tarfile.open(fileobj=compressed, mode='w', format=tarfile.GNU_FORMAT) as archive:
                archive.add(folder, arcname=archive_name, filter=filter_tar)

    # Debian package: binary under /opt, launcher on PATH, normal desktop entry.
    # User state is always outside the read-only installed application directory.
    deb_root = work / 'debian'
    opt = deb_root / 'opt/clipcrop-points'
    opt.mkdir(parents=True)
    shutil.copytree(folder, opt, dirs_exist_ok=True)
    bin_dir = deb_root / 'usr/bin'; bin_dir.mkdir(parents=True)
    launcher = bin_dir / 'clipcrop-points'
    launcher.write_text('#!/bin/sh\nset -eu\nexec /opt/clipcrop-points/clipcrop "$@"\n')
    launcher.chmod(0o755)
    applications = deb_root / 'usr/share/applications'; applications.mkdir(parents=True)
    (applications / 'local.clipcrop.points.desktop').write_text(
        '[Desktop Entry]\nType=Application\nVersion=1.0\nName=ClipCrop Points\n'
        'Comment=A small peer-to-peer video commons\nExec=clipcrop-points\nIcon=clipcrop-points\n'
        'Terminal=false\nCategories=AudioVideo;Video;Network;\nStartupNotify=true\n')
    icons = deb_root / 'usr/share/icons/hicolor/scalable/apps'; icons.mkdir(parents=True)
    shutil.copyfile(root / 'assets/icon.svg', icons / 'clipcrop-points.svg')
    doc_dir = deb_root / 'usr/share/doc/clipcrop-points'; doc_dir.mkdir(parents=True)
    shutil.copyfile(root / 'LICENSE', doc_dir / 'copyright')
    shutil.copyfile(folder / 'LINUX.md', doc_dir / 'README.md')

    dependencies = os.environ.get('CLIPCROP_LINUX_DEB_DEPENDS',
        'libc6 (>= 2.35), libgcc-s1, libgtk-3-0 (>= 3.24), libwebkit2gtk-4.1-0, '
        'gstreamer1.0-plugins-base, gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-libav')
    installed_size = sum(path.stat().st_size for path in deb_root.rglob('*') if path.is_file()) // 1024
    control = (
        f'Package: clipcrop-points\nVersion: {version}\nSection: video\nPriority: optional\n'
        f'Architecture: amd64\nInstalled-Size: {installed_size}\n'
        f'Depends: {dependencies}\nMaintainer: ClipCrop contributors <clipcrop@localhost>\n'
        'Description: A small peer-to-peer video commons\n'
        ' Share short MP4/WebM clips through BitTorrent, watch and rate videos,\n'
        ' and exchange experimental points using signed peer-to-peer receipts.\n'
        ' The UI and validator are embedded; GTK/WebKitGTK and codecs use system libraries.\n')
    md5sums = ''.join(f'{hashlib.md5(path.read_bytes()).hexdigest()}  {path.relative_to(deb_root)}\n'
                      for path in sorted(deb_root.rglob('*')) if path.is_file())
    control_buffer = io.BytesIO()
    with gzip.GzipFile(fileobj=control_buffer, mode='wb', mtime=stamp, filename='') as compressed:
        with tarfile.open(fileobj=compressed, mode='w', format=tarfile.GNU_FORMAT) as archive:
            for name, text in [('control', control), ('md5sums', md5sums)]:
                data = text.encode()
                info = tarfile.TarInfo('./' + name); info.mode = 0o644; info.size = len(data)
                archive.addfile(filter_tar(info), io.BytesIO(data))
    data_path = work / 'data.tar.gz'
    with data_path.open('wb') as output:
        with gzip.GzipFile(fileobj=output, mode='wb', mtime=stamp, filename='') as compressed:
            with tarfile.open(fileobj=compressed, mode='w', format=tarfile.GNU_FORMAT) as archive:
                for path in sorted(deb_root.iterdir()):
                    archive.add(path, arcname='./' + path.name, filter=filter_tar)
    deb_path = work / f'clipcrop-points_{version}_amd64.deb'
    with deb_path.open('wb') as deb:
        deb.write(b'!<arch>\n')
        for name, value in [('debian-binary', b'2.0\n'), ('control.tar.gz', control_buffer.getvalue()), ('data.tar.gz', data_path)]:
            size = len(value) if isinstance(value, bytes) else value.stat().st_size
            header = f'{name + "/":<16}{stamp:<12}{0:<6}{0:<6}{"100644":<8}{size:<10}`\n'.encode('ascii')
            if len(header) != 60:
                raise SystemExit('Invalid Debian ar header')
            deb.write(header)
            if isinstance(value, bytes):
                deb.write(value)
            else:
                with value.open('rb') as stream:
                    shutil.copyfileobj(stream, deb)
            if size % 2:
                deb.write(b'\n')

    final_folder = output_parent / 'linux-x86_64'
    if final_folder.exists():
        # Refuse to replace a folder in which someone already ran a portable profile.
        if any((final_folder / name).exists() for name in ('clipcrop-data', 'clipcrop-points-data')):
            raise SystemExit('dist/linux-x86_64 contains a profile; move it out before replacing the package')
        shutil.rmtree(final_folder)
    shutil.move(str(folder), final_folder)
    artifacts = []
    for artifact in [portable, deb_path]:
        target = output_parent / artifact.name
        os.replace(artifact, target)
        artifacts.append(target)
    checksums = output_parent / 'clipcrop-linux-x86_64-SHA256SUMS.txt'
    checksums.write_text(''.join(f'{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n' for path in artifacts))
    print('Linux folder:', final_folder)
    for artifact in artifacts:
        print('Linux artifact:', artifact)
    print('Checksums:', checksums)
finally:
    shutil.rmtree(work)
PY
