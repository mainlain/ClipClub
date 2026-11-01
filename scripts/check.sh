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

if [[ -z "${CLIPCROP_FFPROBE:-}" && -x "$project_root/vendor/ffprobe" ]]; then
  export CLIPCROP_FFPROBE="$project_root/vendor/ffprobe"
fi

if "$cargo_bin" fmt --version >/dev/null 2>&1; then
  "$cargo_bin" fmt --all -- --check
else
  printf 'rustfmt is unavailable; formatting check skipped. Install it with rustup component add rustfmt.\n' >&2
fi

"$cargo_bin" test --locked --no-default-features --all-targets
"$cargo_bin" check --locked

if command -v node >/dev/null 2>&1; then
  while IFS= read -r -d '' script; do
    node --check "$script"
  done < <(find ui -type f -name '*.js' -print0)
  node --test scripts/library.test.cjs
fi

printf 'Checks completed.\n'
