#!/usr/bin/env bash
set -euo pipefail

# [Review Fix #81] SCM artifacts must be built on Linux; a local Mach-O is not a TCE release.
if [[ "$(uname -s)" != Linux ]]; then
  printf '%s\n' 'ask-auth deployment build requires Linux' >&2
  exit 1
fi

ASK_AUTH_SERVICE_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ASK_AUTH_BUILD_ROOT="$(cd -- "$ASK_AUTH_SERVICE_ROOT/../.." && pwd)"
# [Review Fix #105] Resolve all builds and the authoritative lockfile from the root workspace.
cd -- "$ASK_AUTH_BUILD_ROOT"
ASK_AUTH_RUST_HOST="$(rustc -vV | awk '/^host: / { print $2 }')"
if [[ "$ASK_AUTH_RUST_HOST" != *-linux-* ]]; then
  printf '%s\n' 'ask-auth deployment build requires a Linux Rust host' >&2
  exit 1
fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ASK_AUTH_BUILD_ROOT/target}"
# [Review Fix #87] Cargo environment overrides must not remove the core's software-POLYVAL wiping configuration.
if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
  export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS}"$'\x1f--cfg\x1fpolyval_force_soft'
elif [[ "${CARGO_ENCODED_RUSTFLAGS+present}" == present ]]; then
  export CARGO_ENCODED_RUSTFLAGS=$'--cfg\x1fpolyval_force_soft'
else
  export RUSTFLAGS="${RUSTFLAGS:-} --cfg polyval_force_soft"
fi
cargo build -p ask-auth --release --locked --bin ask-auth --target "$ASK_AUTH_RUST_HOST"
mkdir -p "$ASK_AUTH_SERVICE_ROOT/output/bin"
cp -- "$CARGO_TARGET_DIR/$ASK_AUTH_RUST_HOST/release/ask-auth" "$ASK_AUTH_SERVICE_ROOT/output/bin/ask-auth"
cp -- "$ASK_AUTH_SERVICE_ROOT/run.sh" "$ASK_AUTH_SERVICE_ROOT/output/run.sh"
chmod 755 "$ASK_AUTH_SERVICE_ROOT/output/bin/ask-auth" "$ASK_AUTH_SERVICE_ROOT/output/run.sh"
