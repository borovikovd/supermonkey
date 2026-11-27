# AGENTS.md

Principles for building and changing a Tauri app (Rust core + web UI). Everything
specific to this repository is in [Project specifics](#project-specifics).

## How to work

- **Spec first.** The spec is the source of truth for behavior. Update it first,
  then tests and code, in the same commit. Where it is silent, decide and write
  the decision in; ask only when a choice is hard to undo.
- **Done means verified live.** Passing the gate is necessary, not sufficient:
  exercise the change in the running app.
- **Report honestly.** State failures, flakes, skipped steps, and what you did
  not check. Fix unrelated bugs you spot with a test, or write them down — never
  leave them unmentioned.

## Git and releases

- **No AI attribution** anywhere (`Co-Authored-By`, "generated with", etc.).
- **One logical change per commit**, Conventional Commits (`type(scope): summary`,
  `!` for breaking), with a body explaining why. `feat`, `fix`, and `perf`
  subjects become user-facing release notes.
- **Automated releases.** Never bump versions or edit the changelog by hand; use
  the release tool's override (`Release-As: x.y.z`) when releasing other changes.
- **Branch discipline.** Commit only on a green gate, never skip hooks, push
  after each commit, and never force-push, rewrite pushed history, or delete
  branches without asking. Squash-merge long-lived branches into one commit so
  internal fixes stay out of release notes, keeping the branch for history.
- **Parallel work.** Separate agents use separate git worktrees; merge shared
  lists (test plans, id tables) by union and renumber; delete finished worktrees
  and their build folders.

## Architecture

- **Layered modules, strict DAG.** Prefer layered modules inside one app crate
  over premature multi-crate splits; reserve separate crates for standalone
  binaries or test doubles. Enforce module edges and external-crate boundaries
  in an architecture test.
- **Cohesive modules, narrow facades.** Keep internal submodules `pub(crate)`
  behind a small facade. Split by responsibility (no catch-all "features" or
  "utils"), keep pure state machines in `domain`, and centralize repeated guards
  and worker loops.
- **Pure IPC, shared session.** Define wire types once in Rust, generate UI
  bindings (committed; gate fails if stale), and keep `ipc` depending only on
  `domain`. Put session/host glue above `core` so the native shell and test
  harness share one entry path.
- **Single ownership of concerns.** Rust owns truth (UI mirrors state + pending
  actions); one writer actor owns SQLite writes (reads use a pool); one client
  per external service owns auth, rate limits, retries, and error mapping;
  quitting (including macOS menu Quit, which by default bypasses Tauri exit
  hooks), untrusted HTML rendering, and keybindings each have exactly one path.
- **User-visible priority.** Schedule on-screen work ahead of backfills and
  secondary sync.
- **Reproducible modes and data.** Every mode (`dev`, `demo`, `test`, `release`)
  runs via one command; non-release modes never touch real user data, and `demo`
  rebuilds fresh each launch. The bundle ID names the data folder — plan
  migration before renaming it.
- **Contract-first rewrites.** Freeze generated types, the E2E suite, and the
  spec first; rewrite slice by slice on a branch, every slice green.

## Build and the gate

- **One runner, one gate.** Named tasks only (no build shell scripts). One `gate`
  task runs all checks (Rust tests/lints, UI format/lint/typecheck, binding
  freshness, E2E) and fails on warnings; each check also runs standalone.
- **Check every shipped target.** Cargo workspace builds unify features and hide
  warnings; lint and build default, debug-feature, and release configs.
- **Hermetic and clean.** Checks need no secrets (use committed placeholders),
  avoid untracked task caches, keep zero unused dependencies, and delete temp
  test data on every exit including `SIGTERM`.

## Security and secrets

- **Least privilege and strict webview boundary.** Grant minimal window
  capabilities; justify plugins and broad permissions (`shell`, `fs`, `http`).
  Validate every command input in Rust (the webview is untrusted). Enforce strict
  CSP (no `unsafe-eval`, no remote scripts); sanitize untrusted HTML in Rust,
  render only in a script-blocked sandboxed iframe, and open links in the system
  browser.
- **Untrusted URLs and files.** Untrusted fetches require `https`, a short
  timeout, and no redirects to private/loopback IPs. Quarantine saved untrusted
  files (`com.apple.quarantine` on macOS) before opening.
- **Release isolation.** Compile debug servers and test hooks out of release
  builds; localhost debug servers must check `Origin` and `Sec-Fetch-Site`. Test
  release builds explicitly (`tauri://localhost` differs from dev
  `http://localhost`).
- **Secrets.** Keep credential files git-ignored. Store tokens in the OS keychain
  (or a `0600` file when unsigned macOS prompts block workflow) — never in
  SQLite, git, tests, chat, or logs. Never log tokens, auth codes, or user
  content.

## Concurrency

Assume every `.await` lets the world change.

- **Decide every interruption.** Specify how every long operation handles
  mid-flight close, repeat, undo, quit, remote mutation/deletion, owner removal,
  or offline.
- **Bind targets; stale loses, local wins.** Capture the user's target (account,
  items) at action time and re-verify before late work applies. Guard remote
  replies with versions/generations/sequences, and re-apply pending local writes
  over remote reads.
- **Monotonic IDs and idempotent retries.** Never reuse IDs (`AUTOINCREMENT` or
  stored sequence, never `max(id) + 1`). Number requests so retries with backoff
  deduplicate safely.
- **Drop guards for in-progress state.** Clear spinners, busy flags, and sending
  states on every path (success, error, cancellation, panic).
- **Brief locks.** Never hold a lock across `.await` or external calls/I/O; keep
  coupled invariants under one lock.
- **Isolation and bounded shutdown.** Isolate queues, slots, and limits per
  resource; on quit, wait up to a deadline for user writes to commit, then exit.
- **Force the race in tests.** Every concurrency fix includes a test that holds
  and releases the interleaving (or explains why it cannot be forced).

## Rust

- **Strict lints, zero panics/unsafe.** Enforce `clippy::pedantic` + strict
  extras and deny rustdoc lints. Never loosen them; use
  `#[expect(lint, reason = "...")]` only, and record false-positive evidence
  (count + sample) beside any disabled lint. Forbid `unsafe`,
  `unwrap`/`expect`/`panic` outside tests, and silent `as` casts (`try_from`).
- **Own or borrow deliberately.** Borrow to read (`&T`, `&str`, `&[T]`, single
  field over whole context), `&mut T` to mutate, take by value to store, send,
  or consume (`Copy` by value). Never take `&T` and clone inside; never clone to
  appease the borrow checker — reshape ownership. Before writing
  `param.clone()`, check whether callers still need it and flip to `T` if not.
  Move data forward through multi-stage pipelines (`.take()`, `.remove()`,
  `into_*`) instead of holding earlier stages by `&`, and use borrowing
  `#[derive(Serialize)]` views (`*Ref<'a>`) for values built solely to serialize
  and drop.
- **Minimal pointers and locks.** Structs own their fields (references only in
  single-call or single-tx scopes). Add `Box`, `Arc`, or `Cow` only for a named
  need; prefer one owner + IDs over `Arc<Mutex<T>>` / `Rc<RefCell<T>>`. Never
  hide `Arc` or `Box` behind a type alias; take `&Arc<T>` only to clone into a
  task. Use `parking_lot` locks; justify any async mutex held across `.await`.
- **Clear types.** Wrap IDs and units in newtypes (convert only at SQL/serde/IPC
  edges). Avoid single-implementation traits. Return slices or `Cow` from
  parsers. Read helpers take a DB handle, not app context.
- **Explicit errors and clean logs.** Return errors or log with context (`let _ =`
  only for closed channels). Emit one-line event logs without user content or
  secrets; filter noisy third-party targets at the logger.

## UI

- **Strict types and rules.** Type-aware lint, no `any`, no non-null assertions;
  every suppression names its rule and reason; never suppress framework
  correctness rules (e.g., React hooks).
- **Design tokens.** Define every color, type scale, radius, shadow, font, and
  fixed width by role in one file, disabling default framework palettes/scales.
  The gate rejects color literals, arbitrary values (`x-[...]`), and colliding
  token classes.
- **Honest, resilient UI.** Hold up under long text, small windows, and empty
  states (min heights, not fixed). Never flash blank screens or instant spinners
  (show cached state immediately; delay loading indicators). Local stand-ins for
  remote results must be exact or absent, never supersets. Show only working
  actions/hints per screen, and keep heavy work off keydown-to-paint.

## Testing

- **Real app, controllable fake.** E2E tests drive the built UI + real Rust core
  against a fake server supporting errors, timeouts, holds, external edits, and
  realistic rate limits/latency — never the network or real accounts.
- **Test first, external oracles.** Expected values come from the spec, fixtures,
  or hand calculations — never code under test. Write the test first and observe
  it fail for the right reason before fixing; mark unfixed bugs expected-fail
  with `BUG:`.
- **Deterministic, zero retries.** Wait on app idle signals and advance a fake
  clock — never sleep. Select by role, label, or `data-*`. Never retry flaky
  tests — fix the race.
- **Systematic E2E and focused unit tests.** Cover every effect × trigger,
  perception (size, contrast, overlap, small window, axe),
  startup/modes/migrations, failure paths, and long-op interruptions. Use
  unit/property tests only for complex pure logic, security invariants, and
  races E2E cannot reach.

## Review and QA

- **Use the app.** Inspect every UI change in the running app; test against a
  real account of realistic size before release. Have a fresh eye check screens,
  shortcuts, modes, small windows, long text, console, and logs.
- **Focused flow-based passes.** After large changes, run separate passes for
  architecture, ownership, concurrency, security, and test strength, sliced end
  to end by data flow rather than directory, repeating until a pass finds
  nothing. For ownership passes, enforce zero-waste strictly on hot paths
  (loops, large buffers/bodies, SQL/MIME/sync, `Copy`/API boundaries) and leave
  one-shot startup, error, and migration paths alone unless egregious — stop
  once a pass finds nothing above that bar.
- **Converging bug review.** Put batch fixes on a branch, never dueling commits
  on `main`. State the two-sided invariant before editing (both safety/no-waste
  and completeness/soundness) and lock both sides with an invariant or
  state-machine test rather than a single example. A reviewer challenges the
  branch without rewriting it and blocks only on a runnable counterexample or a
  checked baseline/external fact (`git show`, primary spec, live probe) — never
  memory or unprobed hunches. Resolve only verified blockers, freeze
  unverifiable external questions for a live check, and stop at zero blockers on
  a green gate.
- **Learn from escapes.** When a bug passes the gate, add the test or rule that
  would have caught it.

## Project specifics

- **Product:** Supermonkey, a keyboard-first Gmail client for macOS (Tauri 2,
  React 19 + React Compiler, TypeScript 7, Tailwind 4, Vite+, Rust + SQLite).
- **Spec:** Behavior lives in the E2E suite (`e2e/*.spec.ts`) and Rust tests
  (no separate spec doc). Behavior changes start by updating or adding a test.
- **Commands:** Use `vp` (Vite+) for JS tooling — never `npm`, `pnpm`, or `npx`.
  Run `cargo` from `src-tauri/`.

  ```sh
  vp install            # dependencies
  vp run demo           # app against in-process fake Gmail, fresh each launch
  vp run tauri dev      # app against real Gmail (needs google-oauth-client.json)
  vp run tauri build    # release .app
  vp check --fix        # UI format, lint, type check
  vp run gate           # full gate; run before each commit
  vp run <task>         # single check: test:rust, lint:rust, lint:ts, typecheck, bindings, check:static, e2e
  ```

- **Layout:**
  - `src-tauri/src` (edges checked by `src-tauri/tests/architecture.rs`):
    `domain/` (newtypes, mail/draft models, views, splits, snooze, contacts, query language),
    `mime/` (parsing, sanitizer, unsubscribe, outgoing MIME),
    `gmail/` (client, quota scheduler, OAuth, token file),
    `store/` (schema, queries, search, contacts, writer actor, read pool),
    `ipc/` (pure wire types generated into `src/bindings`),
    `core/` (engine, writer state, actions, subscriptions, drafts, outbox, listings,
    snooze, network, notifications, unwanted, attachments, sync workers),
    `session/` (`Session`, `Host`, optional `web` bridge),
    `shell/` (Tauri commands, standard menu with app-owned Quit, platform adapters, logging),
    `testkit/` (`feature = "fake"`: E2E server, demo seed, perf fixture),
    `bin/` (`e2e-server`, `perf-fixture`).
  - `src-tauri/crates/sm-fake-gmail`: standalone in-process fake Gmail server.
  - `src/`: UI (`store/`, `keyboard/`, `ui/`, `compose/`, `ipc/`); design tokens
    in `src/theme.css`. `src/ui/UntrustedFrame.tsx` is the single path for mail
    HTML (sandbox keeps `allow-scripts` only because WebKit otherwise drops
    parent listeners; its CSP executes nothing).
- **Tests:** E2E tests live in `e2e/*.spec.ts` using `e2e/app.ts`,
  `e2e/control.ts`, and `e2e/seeds.ts` (`app.advance()`, `app.settle()`).
  Rust invariant/chaos tests live in `src-tauri/tests/invariants.rs` and
  `src-tauri/tests/engine.rs`; static and mirror checks (`checks/*.test.ts`,
  `src/store/mirror-core.test.ts`) run fast via `vp run check:static`.
- **Releases:** `.github/workflows/release.yml` manages a "release X.Y.Z" PR on
  `main` (merged only by the user) using `GOOGLE_OAUTH_CLIENT_JSON`. Versions
  live in `package.json` and `src-tauri/Cargo.toml`.
- **No comments:** Code and tests carry no comments, doc comments included
  (`missing_docs` is off). Keep lint directives with inline reasons (`--` or
  `reason = "..."`).
- **Secrets:** `google-oauth-client.json` is git-ignored;
  `src-tauri/lint/oauth-client.json` is the gate placeholder; refresh tokens live
  in `tokens.json` (`0600`) in the app's data folder.
