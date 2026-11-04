# Ubermensch

A keyboard-first Gmail client for macOS, built from [SPEC.md](SPEC.md).
Tauri 2 + React 19 + Rust/SQLite. Choices the spec left open are in
[DEVIATIONS.md](DEVIATIONS.md).

## Run

```sh
vp install
vp run tauri dev       # real Gmail; needs google-oauth-client.json (git-ignored)
vp run demo            # no Google: in-process fake Gmail with demo mail and two accounts
```

Sign in from the first-run screen or `⌘K → Add account`. Google shows an
"unverified app" warning; continue through it. While the consent screen is in
Testing, Google asks you to sign in again every 7 days.

## Build

```sh
vp run tauri build     # src-tauri/target/release/bundle/macos/Ubermensch.app
```

## Checks

```sh
vp run gate             # every check: Rust tests, lints, type checks, bindings, then the E2E suite
vp run lint:rust        # one step (tasks are in vite.config.ts): test:rust, lint:rust, lint:ts, typecheck, bindings, e2e
vp exec playwright test # E2E suite alone (Playwright + WebKit; `vp exec playwright install webkit` once)
vp config --hooks       # once per clone: pre-commit runs `vp staged` (format and lint staged files)
```

Lint setup:

- TypeScript: Oxlint through Vite+ with type-aware rules and type checking
  (`vite.config.ts`). Correctness, suspicious, and perf rules are errors, and
  warnings fail the check. `tsconfig` is `strict` with
  `noUncheckedIndexedAccess` and `exactOptionalPropertyTypes`. The React
  Compiler (the Rust port, `oxc-transform-react`) compiles every component
  except the list, which TanStack Virtual keeps out (marked in `ListView.tsx`);
  lint fails on any other component the compiler would skip.
- Rust: workspace lints in `src-tauri/Cargo.toml`. Clippy `all` is denied and
  `pedantic` is on, `unsafe_code` is forbidden, and `unwrap` is flagged
  outside tests. `rustfmt.toml` and `clippy.toml` sit next to it.

Perf fixture (two accounts × 20,000 threads):

```sh
cargo run --manifest-path src-tauri/Cargo.toml --release --features fake --bin perf-fixture -- /tmp/perf.db perf
UBERMENSCH_DB=/tmp/perf.db vp run tauri dev
UBERMENSCH_PERF_DB=/tmp/perf.db cargo test --manifest-path src-tauri/Cargo.toml --release --test perf_search -- --ignored --nocapture
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
