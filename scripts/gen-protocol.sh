#!/usr/bin/env bash
# Regenerate TypeScript bindings from crates/protocol.
# Generated files live in apps/desktop/src/shared/ and must not be edited by hand.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="${HOME}/.cargo/bin:${PATH}"
export TS_RS_EXPORT_DIR="${PWD}/apps/desktop/src/shared"
rm -f apps/desktop/src/shared/*.ts
cargo test -p protocol --quiet
echo "generated: $(ls apps/desktop/src/shared/*.ts | tr '\n' ' ')"
