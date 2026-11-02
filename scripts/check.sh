#!/usr/bin/env bash
# Full local verification: formatting, protocol bindings, Rust tests, desktop typecheck and build.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="${HOME}/.cargo/bin:${PATH}"

./scripts/gen-protocol.sh

echo "== cargo fmt =="
cargo fmt --all -- --check

echo "== cargo test =="
cargo test --workspace

echo "== desktop typecheck =="
pnpm --dir apps/desktop typecheck

echo "== desktop build =="
pnpm --dir apps/desktop build
