#!/usr/bin/env bash
# Gates before a build step is called done (SPEC Testing):
# vp check, cargo test (which regenerates the bindings), cargo clippy,
# and the committed bindings must match the generated ones.
set -euo pipefail
cd "$(dirname "$0")/.."
vp check
vp test
(cd src-tauri && cargo test --workspace -q && cargo clippy --workspace --all-targets -q -- -D warnings)
if [ -n "$(git status --porcelain src/bindings)" ] && git rev-parse HEAD >/dev/null 2>&1 && git ls-files --error-unmatch src/bindings >/dev/null 2>&1; then
  git diff --stat -- src/bindings
  echo "Generated bindings differ from the committed files. Commit src/bindings." >&2
  exit 1
fi
echo "gate: all green"
