#!/usr/bin/env bash
set -euo pipefail

# The packaged copy lives alongside the executable and original SVG icon.
installer_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
program_path="${1:-$installer_directory/clipcrop}"
if [[ "$(uname -s)" != Linux ]]; then
  printf 'This launcher installer is for Linux.\n' >&2
  exit 1
fi
if [[ ! -x "$program_path" ]]; then
  printf 'Pass the path to the extracted ClipCrop executable.\n' >&2
  exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
  printf 'Creating the launcher needs Python 3. The application itself does not.\n' >&2
  exit 1
fi
case "${XDG_DATA_HOME:-}" in
  /*) application_data="$XDG_DATA_HOME" ;;
  *) application_data="$HOME/.local/share" ;;
esac
python3 - "$program_path" "$application_data" <<'PY'
from pathlib import Path
import sys

program = Path(sys.argv[1]).resolve(strict=True)
data_root = Path(sys.argv[2]).expanduser().resolve()
if any(c in str(program) + str(data_root) for c in '\n\r\0'):
    raise SystemExit('Application and data paths cannot contain newlines or NUL characters.')

def exec_argument(value):
    # Desktop Entry string escaping precedes Exec's double-quote parsing.
    value = str(value).replace('\\', '\\\\\\\\')
    for character in ('"', '$', '`'):
        value = value.replace(character, '\\\\' + character)
    return '"' + value.replace('%', '%%') + '"'

def desktop_string(value):
    return str(value).replace('\\', '\\\\').replace('\t', '\\t')

applications = data_root / 'applications'
applications.mkdir(parents=True, exist_ok=True)
icon = program.parent / 'clipcrop.svg'
if not icon.is_file():
    # Source-tree invocation can use the original asset.
    icon = program.parent.parent.parent / 'assets/icon.svg'
icon_value = desktop_string(icon) if icon.is_file() else 'video-x-generic'
entry = applications / 'local.clipcrop.points.desktop'
entry.write_text(
    '[Desktop Entry]\n'
    'Type=Application\n'
    'Version=1.0\n'
    'Name=ClipCrop Points\n'
    'Comment=A small peer-to-peer video commons\n'
    f'Exec={exec_argument(program)}\n'
    f'Icon={icon_value}\n'
    'Terminal=false\n'
    'Categories=AudioVideo;Video;Network;\n'
    'StartupNotify=true\n', encoding='utf-8')
entry.chmod(0o644)
print('Created', entry)
print('Keep the application at', program)
PY
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$application_data/applications"
fi
