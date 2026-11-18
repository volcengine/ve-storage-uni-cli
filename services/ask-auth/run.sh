#!/usr/bin/env bash
set -euo pipefail
ASK_AUTH_RUN_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec "$ASK_AUTH_RUN_ROOT/bin/ask-auth" "$@"
