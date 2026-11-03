#!/usr/bin/env bash
# Gates before a commit (SPEC Testing):
# cargo test first (it regenerates the bindings the TypeScript checks read),
# then vp check (format, lint, type check), tsc on every project,
# cargo fmt, cargo clippy with and without the demo feature, rustdoc warnings,
# the committed bindings must match the generated ones, and last the
# end-to-end suite (Playwright drives the built UI against the real core).
set -euo pipefail
cd "$(dirname "$0")/.."
(cd src-tauri && cargo test --workspace -q)
vp check
node_modules/.bin/tsc -p tsconfig.app.json --noEmit
node_modules/.bin/tsc -p tsconfig.node.json --noEmit
node_modules/.bin/tsc -p e2e --noEmit
(
  cd src-tauri
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -q -- -D warnings
  cargo clippy --workspace --all-targets --features fake -q -- -D warnings
  RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps -q
)
if [ -n "$(git status --porcelain src/bindings)" ] && git rev-parse HEAD >/dev/null 2>&1 && git ls-files --error-unmatch src/bindings >/dev/null 2>&1; then
  git diff --stat -- src/bindings
  echo "Generated bindings differ from the committed files. Commit src/bindings." >&2
  exit 1
fi
vp exec playwright test
echo "gate: all green"
