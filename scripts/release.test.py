#!/usr/bin/env python3
"""Regression checks for updater archive layout and fail-closed release metadata."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import struct
import subprocess
import tarfile
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('release', ROOT / 'scripts/release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def test_tag_must_match_cargo_version(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'Cargo.toml').write_text('[package]\nname = "clipclub"\nversion = "1.2.3"\n')
            self.assertEqual(release.release_version('v1.2.3', root), '1.2.3')
            for tag in ('v1.2.4', '1.2.3', 'v1.2.3\nother=value'):
                with self.assertRaises(ValueError):
                    release.release_version(tag, root)

    def test_semver_rejects_empty_identifiers_and_leading_zero_prereleases(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for version in ('0.2.0+build-1', '0.2.0-rc.1+build.01', '0.2.0--'):
                (root / 'Cargo.toml').write_text(f'[package]\nversion = "{version}"\n')
                self.assertEqual(release.release_version('v' + version, root), version)
            for version in ('0.2.0-', '0.2.0+', '0.2.0-alpha..1', '0.2.0+build..1',
                            '0.2.0-01', '0.2.0-rc.01', '01.2.0'):
                (root / 'Cargo.toml').write_text(f'[package]\nversion = "{version}"\n')
                with self.assertRaises(ValueError):
                    release.release_version('v' + version, root)

    def test_native_architecture_is_enforced(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'probe'
            binary.write_bytes(b'\xcf\xfa\xed\xfe' + struct.pack('<I', 0x0100000C) + b'\0' * 12)
            release.check_binary(binary, 'aarch64-apple-darwin')
            for target in ('unsupported-target', 'x86_64-unknown-linux-gnu'):
                with self.assertRaises(ValueError):
                    release.check_binary(binary, target)
            binary.write_bytes(b'\x7fELF\x02\x01' + b'\0' * 10 + struct.pack('<HH', 2, 62))
            release.check_binary(binary, 'x86_64-unknown-linux-gnu')
            with self.assertRaises(ValueError):
                release.check_binary(binary, 'aarch64-apple-darwin')

    def test_manifest_requires_every_platform_and_records_real_digests(self):
        version = release.release_version()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args = argparse.Namespace(tag='v' + version, repository='mainlain/ClipClub', directory=root)
            filenames = [release.asset_name(version, target) for target in release.TARGETS]
            filenames.append(f'clipclub_{version}_amd64.deb')
            for name in filenames[:-1]:
                (root / name).write_bytes(b'release asset: ' + name.encode())
            with self.assertRaises(ValueError):
                release.write_manifest(args)
            self.assertFalse((root / 'update-manifest.json').exists())
            (root / filenames[-1]).write_bytes(b'debian asset')
            release.write_manifest(args)
            data = json.loads((root / 'update-manifest.json').read_text())
            self.assertEqual(data['version'], version)
            self.assertEqual(len(data['assets']), 3)
            for asset in data['assets']:
                contents = (root / asset['name']).read_bytes()
                self.assertEqual(asset['size'], len(contents))
                self.assertEqual(asset['sha256'], hashlib.sha256(contents).hexdigest())
                self.assertEqual(asset['url'], f'https://github.com/mainlain/ClipClub/releases/download/v{version}/{asset["name"]}')
            self.assertIn('  update-manifest.json\n', (root / 'SHA256SUMS.txt').read_text())

    def test_linux_archive_matches_updater_layout_and_keeps_notices(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / 'output'
            notices = output / 'third-party/rust/example-1.0.0'
            notices.mkdir(parents=True)
            (notices / 'LICENSE').write_text('fixture rust license\n')
            (output / 'third-party/rust-dependencies.json').write_text('[]\n')
            elf = b'\x7fELF\x02\x01' + b'\0' * 10 + struct.pack('<HH', 2, 62)
            probe = root / 'ffprobe'
            binary = root / 'clipclub'
            probe.write_bytes(elf + b'fixture validator')
            binary.write_bytes(elf + b'fixture app' + probe.read_bytes())
            version_file = root / 'version.txt'
            version_file.write_text('ffprobe version fixture\n')
            report = root / 'license-report.txt'
            report.write_text('GNU Lesser General Public License version 2.1 or later\n')
            license_file = root / 'LICENSE.LGPLv2.1'
            license_file.write_text('GNU LESSER GENERAL PUBLIC LICENSE\n')
            env = dict(os.environ)
            env.update({
                'CLIPCROP_DIST_DIR': str(output),
                'CLIPCROP_LINUX_BINARY': str(binary), 'CLIPCROP_LINUX_FFPROBE': str(probe),
                'CLIPCROP_LINUX_PROBE_VERSION_FILE': str(version_file),
                'CLIPCROP_LINUX_PROBE_LICENSE_REPORT': str(report),
                'CLIPCROP_LINUX_PROBE_LICENSE_FILE': str(license_file),
                'SOURCE_DATE_EPOCH': '1700000000',
            })
            # Fixtures exercise packaging, never execute these synthetic binaries.
            subprocess.run(['bash', str(ROOT / 'scripts/package-linux.sh')],
                           cwd=ROOT, env=env, check=True, stdout=subprocess.PIPE, text=True)
            version = release.release_version()
            folder = f'clipclub-{version}-linux-x86_64'
            with tarfile.open(output / (folder + '.tar.gz')) as archive:
                member = archive.getmember(folder + '/clipclub')
                self.assertEqual(member.mode, 0o755)
                self.assertEqual(archive.extractfile(member).read(), binary.read_bytes())
                self.assertEqual(archive.extractfile(folder + '/third-party/ffprobe-LICENSE.txt').read(), license_file.read_bytes())
                self.assertIn(folder + '/third-party/rust/example-1.0.0/LICENSE', archive.getnames())
            debian = output / f'clipclub_{version}_amd64.deb'
            self.assertTrue(debian.read_bytes().startswith(b'!<arch>\n'))

    def test_mac_archive_has_one_app_root_and_preserves_executable_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle = root / 'clipclub.app'
            executable = bundle / 'Contents/MacOS/clipclub'
            executable.parent.mkdir(parents=True)
            executable.write_bytes(b'native executable fixture')
            executable.chmod(0o755)
            destination = root / 'app.zip'
            release.archive_bundle(bundle, destination)
            with zipfile.ZipFile(destination) as archive:
                self.assertTrue(all(name.startswith('clipclub.app/') for name in archive.namelist()))
                info = archive.getinfo('clipclub.app/Contents/MacOS/clipclub')
                self.assertEqual((info.external_attr >> 16) & 0o777, 0o755)
                self.assertEqual(archive.read(info), executable.read_bytes())
            (bundle / 'outside').symlink_to('/tmp')
            with self.assertRaises(ValueError):
                release.archive_bundle(bundle, destination)


if __name__ == '__main__':
    unittest.main()
