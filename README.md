# Ubermensch

A keyboard-first Gmail client for macOS, built from [SPEC.md](SPEC.md).
Tauri 2 + React 19 + Rust/SQLite. Choices the spec left open are in
[DEVIATIONS.md](DEVIATIONS.md).

## Run

```sh
pnpm install
pnpm tauri dev          # real Gmail; needs google-oauth-client.json (git-ignored)
pnpm demo               # no Google: in-process fake Gmail with demo mail and two accounts
```

Sign in from the first-run screen or `⌘K → Add account`. Google shows an
"unverified app" warning; continue through it. While the consent screen is in
Testing, Google asks you to sign in again every 7 days.

## Build

```sh
pnpm tauri build        # src-tauri/target/release/bundle/macos/Ubermensch.app
```

## Checks

```sh
./scripts/gate.sh       # vp check, vp test, cargo test, cargo clippy, bindings
```

Perf fixture (two accounts × 20,000 threads):

```sh
cd src-tauri
cargo run --release --bin perf-fixture -- /tmp/perf.db perf
UBERMENSCH_DB=/tmp/perf.db pnpm tauri dev
UBERMENSCH_PERF_DB=/tmp/perf.db cargo test --release --test perf_search -- --ignored --nocapture
```

Debug builds log keydown-to-paint for each shortcut and warn when one is over
budget. `UBERMENSCH_WEB_BRIDGE=1` serves the command surface on
`127.0.0.1:1430`, so the UI can run in a browser at `http://localhost:5173`
against the real core.

## Layout

- `src-tauri/src` — writer thread, subscriptions, mail actions, drafts,
  outbox, splits, and the async engine (sync, op queue, outbox runner).
- `src-tauri/crates` — `ub-mime`, `ub-gmail`, `ub-fake-gmail`, `ub-rules`.
- `src` — the React UI: mirror and chrome stores, keyboard, palette, list,
  conversation, composer. `src/bindings` is generated from Rust by ts-rs.
