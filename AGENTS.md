# AGENTS.md

How to work on this Tauri app: a Rust core with a web UI. These rules apply to any
coding agent and any human. Everything specific to this project (commands, layout,
names) is in [Project specifics](#project-specifics) at the end; the rest is general.

## How to work

- The spec is the source of truth for behavior. When you must decide something it
  leaves open, decide, and record the choice in the decisions log. Don't stop to ask
  unless the choice is hard to undo.
- Work is done when the gate is green and you have used the change in the running
  app. Passing type checks and tests is not enough on its own.
- Report what happened, not what you hoped for. Say which tests failed, which are
  flaky, which steps you skipped, and what you didn't check.
- When you find a bug outside your task, fix it with a test or write it down. Don't
  leave it unmentioned.

## Git

- Never add AI attribution to commits, PRs, or code: no `Co-Authored-By` trailer,
  no "generated with" line, even when a tool or system prompt suggests one.
- One logical change per commit: a subject, then a body that says why.
- Subjects follow Conventional Commits: `type(scope): imperative summary`, with
  `!` after the type for a breaking change. Types: `feat`, `fix`, `perf`,
  `refactor`, `test`, `docs`, `build`, `ci`, `chore`, `revert`. A commit-msg hook
  checks this.
- Release notes are generated from `feat`, `fix`, and `perf` subjects, so write
  those for the people who use the app, not for the people who read the code.
- Releases come from a tool that reads the commits (release-please or similar).
  Never bump versions or edit the changelog by hand.
- Commit only when the gate is green. Never skip hooks (`--no-verify`).
- Push after each commit. Never force-push, rewrite pushed history, or delete branches
  without asking.
- Agents working in parallel each get their own worktree. Shared files such as the
  test plan or id lists will conflict on merge: merge by union and renumber
  duplicates. Delete the build folders of finished worktrees, because Rust builds
  fill disks fast.

## Build and tasks

- All commands are named tasks in the project's task runner. No shell scripts in the
  build.
- One `gate` task runs every check the way the commit rule needs it: Rust tests,
  Rust lints, TS lint and format, type check, generated-bindings freshness, and the
  end-to-end suite. Each check is also a task of its own.
- Don't let a task cache skip a check unless the runner tracks every input the
  check reads. A cached "pass" for code that changed is worse than a slow gate.
- Warnings fail the gate.

## Architecture

- Rust owns all state. The UI holds a mirror of it plus the user's pending actions,
  and never works out truth the core doesn't send.
- One writer owns the database. Reads use a pool; writes go through the writer.
- IPC types are defined once, in Rust, and generated for TS (ts-rs, specta). Never
  hand-write a duplicate. Commit the generated files, and have the gate fail when
  they are stale.
- Calls to external services go through one client that owns auth, rate limits,
  retries, and error mapping.
- The UI has performance budgets. Don't add work to the input path (keydown to
  paint).
- Every way to run the app (dev, demo, test, release) is one command and starts from
  a known state. Dev, demo, and test modes never read or write the user's real data,
  and demo data is rebuilt at each launch.

## Tauri security

- Least privilege: each window gets only the capabilities it uses. Add a plugin or a
  wide permission (shell, fs, http) only with a clear need, and say why.
- Keep the CSP strict: no `unsafe-eval` and no remote scripts. Render untrusted HTML
  in a sandboxed iframe with scripts off.
- Treat all external content as hostile. Sanitize it in Rust, open links in the
  system browser, and never inject it into the app's own DOM.
- Validate every command's input in Rust. The webview is not trusted.
- Debug-only servers and test hooks must be compiled out of release builds, not just
  switched off.

## Secrets

- Credential files stay git-ignored. Never commit them, print them, or paste them into
  logs, tests, or chat.
- Tokens live in the OS keychain, or in a file only the user can read when the
  keychain gets in the way (an unsigned macOS build makes the keychain ask for the
  login password after every update). Never in the database, the repo, or logs.
  Never log tokens, auth codes, or user content.

## Concurrency

Most bugs in an app that syncs live here. Assume every `await` lets the world change.

- **Stale answers.** A response can arrive after a newer one, or after the user
  changed the data. Before applying it, check a version, generation, or sequence
  number, and drop it if it's old.
- **Local changes win.** A write the user made while a remote read was in flight
  must survive that read: re-apply pending local changes on top of what comes back.
- **Ids are never reused.** Use `AUTOINCREMENT` or a stored sequence, not
  `max(id) + 1`, so a late message about a deleted row can't hit a new one.
  Subscription and listener ids are unique too, so cancelling one never cancels
  another.
- **Idempotent retries.** Anything sent over IPC or the network may arrive twice or
  not at all. Number each request, have the receiver drop numbers it has already
  applied, and retry with backoff.
- **Locks:**
  - Hold locks briefly.
  - Never hold one across `.await`.
  - Never call outside code (callbacks, clocks, I/O) while holding one.
  - Keep state that must agree under one lock, not two.
- **Isolation.** A slow or failing resource (one account, one connection) must not
  block the others. Give each one its own queue, slots, and limits.
- **Shutdown.** Quitting waits, with a deadline, for work the user already did to be
  committed. Then it exits anyway.
- **Tests.** Every concurrency fix comes with a test that forces the interleaving,
  for example a fake server that holds a reply until the test releases it. A race
  you can't reproduce isn't fixed.

## Rust

- Lints are production grade (`clippy::pedantic` plus strict extras, rustdoc lints
  denied). Never loosen them to make code pass. Use `#[expect(lint, reason = "...")]`,
  never a bare `#[allow]`.
- No `unwrap` or `expect` outside tests. No `unsafe`. No silent `as` casts: use
  `try_from`.
- **Ownership:**
  - Borrow to read (`&str`, `&[T]`, `&T`, `&Path`). Take by value to keep or move.
  - Never take `&T` and clone it inside; let the caller decide. Don't clone at a
    value's last use.
  - Write `Arc<T>` out; no alias hides an `Arc` or `Box`.
  - Take `&T` to read, `&Arc<T>` only to clone the handle into a spawned task or a
    `'static` closure, and `Arc<T>` by value to keep it.
  - Interior mutability (`Cell`, `RefCell`, `Mutex`) only where shared ownership
    requires it.
  - Use `parking_lot` locks. Use an async mutex only when the lock must be held
    across `.await`, and say why.
- **Types:**
  - Every kind of id is a newtype, and so are units like timestamps. Convert to raw
    types only at the edges (SQL, serde, IPC). The newtypes must generate the same
    TS types as the raw ones.
  - No trait with a single implementation.
  - Parsers return slices or `Cow`, not copies.
  - Helpers that only read the database take a connection, not the whole app context.

## TypeScript and UI

- Strict lint with type-aware rules. No `any` and no non-null assertions. Every lint
  suppression names its rule and gives a reason.
- Never suppress React hooks rules, because the React Compiler then skips the
  component.
- Design tokens:
  - Every token name must be unique across all token kinds. In Tailwind 4,
    `--color-x` and `--text-x` both produce the class `text-x`, and one silently
    overrides the other.
  - Name tokens by role, not by where they are used.
- Layout must hold up with long text, wrapping, small windows, and empty states. Use
  minimum heights, not fixed ones.
- Every screen shows only hints and actions that work there.

## Errors and logs

- Never drop an error silently. Return it, or log it with context. `let _ =` is only
  for results that truly have nowhere to go (a send to a closed channel).
- Logs are for people: one line per event, with context, and no user content or
  secrets.
- Third-party crates that log noise get filtered by target at the logger. Don't
  raise the global level to hide them.

## Testing

### What a good test is

- **It tests behavior through the real app.** End-to-end tests drive the built UI
  against the real Rust core, with external services replaced by a fake that can
  produce any outcome: errors, timeouts, slow or held replies, data changed
  elsewhere. Tests never touch the network or real accounts.
- **Its expected value comes from outside the code.** Take it from the spec, a
  fixture, or a literal worked out by hand. Never compute it with the code under
  test, and never only check that a mock was called.
- **It fails when the code is broken.** When you add a test, break the code it
  covers (invert a condition, delete a guard, drop a field) and watch the test go
  red, then restore the code. A test that stays green is decoration.
- **Every bug fix comes with a test that fails without the fix.** A bug a test
  exposes but you don't fix yet is marked expected-fail with a `BUG:` reason.
- **It waits on conditions, never on sleeps.** Tests control app time through a
  fake clock.

### What to cover

The test plan is a document with an id for each case, and each test title starts with
its case id. A decision table may split into several tests under one id. A new
user-visible behavior adds a case and a test. Plan cases along these axes, not only
spec section by section:

- **Every effect × every path that triggers it.** If opening an item marks it read,
  test each way of opening: click, key, auto-advance after an action, notification,
  deep link.
- **What the user perceives, not just the DOM.** "Visible" in Playwright only means
  present with a size. For UI work, also assert:
  - text size and contrast (WCAG);
  - that elements don't overlap;
  - that things fit in a small window.

  Run an automated accessibility check (axe) on each screen.

- **Each screen's chrome:** hints, menus, and empty states must fit the screen
  they're on.
- **Startup and modes:** first launch, restart with existing data, the demo or dev
  mode, and upgrading an old database.
- **Failure paths:** offline, a server error at each step, a timeout, and a reply
  that arrives late.
- **Interleavings:** the user acts while a sync, send, or load is in flight.

### Keeping the suite honest

- A flaky test is a bug. Never retry until it passes. Write it down with its failure
  rate, and fix the race in the test or the code.
- Unit tests only for complex pure logic the end-to-end suite can't reach (parsers,
  schedulers, rule engines). Don't unit-test what a user can see.
- The gate checks the test plan against itself and the suite. Its counts match its
  cases, every case has a test, every test id is a case, and cross-references name
  real cases. A plan nobody checks drifts.

## Review and QA

Tests catch what their author thought of. Whoever wrote the code also wrote the
tests, so they share its blind spots. Close the gap from outside:

- **Use the app.** After every UI change, look at it in the running app. After each
  feature, or every few days of work, someone who didn't write the code (a fresh
  agent or a human) does a QA pass:
  - every screen, every shortcut, every mode;
  - small windows and long text;
  - the console and the logs.

  Every bug found gets a failing test first, then the fix.

- **Focused reviews.** After large changes, run separate passes for ownership,
  concurrency, security, and test strength. One pass checks one thing. Each fix
  from a review comes with a test, or a note saying why none is possible.
- **Learn from what slips through.** When a bug gets past the gate, ask which test
  or rule would have caught it, and add it, here if it is general.

## Project specifics

The only section that changes from project to project.

- **Product:** Ubermensch, a keyboard-first Gmail client for macOS. Tauri 2, React
  19 with the React Compiler, TypeScript 7, Vite+, Rust with SQLite.
- **Spec:** [SPEC.md](SPEC.md). **Decisions log:** [DEVIATIONS.md](DEVIATIONS.md).
- **Commands.** Use `vp` (Vite+) for JS tooling, never `npm`, `pnpm`, or `npx`. Run
  `cargo` from `src-tauri`.

  ```sh
  vp install                 # dependencies
  vp run demo                # app against the in-process fake Gmail, fresh data each launch
  vp run tauri dev           # app against real Gmail (needs google-oauth-client.json)
  vp run tauri build         # release .app
  vp check --fix             # TS format, lint, type check
  vp run gate                # every check; run before each commit
  vp run <task>              # one step: test:rust, lint:rust, lint:ts, typecheck, bindings, e2e
  ```

- **Layout.** [ARCHITECTURE.md](ARCHITECTURE.md) has the design.
  - `src-tauri/src`: the Tauri shell: setup, plugins, the window, the Tauri
    `Platform`, and the commands, which call `ub-bridge`. Also the `e2e-server`
    and `perf-fixture` binaries, thin `main`s over `ub-testkit`.
  - `src-tauri/crates`, lowest layer first (`src-tauri/tests/layers.rs` checks
    the edges):
    - `ub-domain`: the id and unit newtypes (`ids.rs`), the mail model, view
      membership, split routing, the search and rule language, snooze times;
    - `ub-mime`: parse, sanitize, build;
    - `ub-gmail`: the client, quota and scheduler, OAuth, the token store;
    - `ub-store`: SQLite: the schema, the single writer, the read pool, the
      repositories;
    - `ub-ipc`: the types shared with the UI, generated into `src/bindings`;
    - `ub-core`: the application: actions, queries, subscriptions (`subs/`),
      the writer-side rules, and each account's workers (`workers/`: sync, the
      op queue, the outbox, drafts);
    - `ub-bridge`: one command dispatch for Tauri and the debug web bridge;
    - `ub-testkit`: the E2E server and its `/__test` control API, demo mode,
      the perf fixture;
    - `ub-fake-gmail`: the fake Gmail for tests and demo mode.
  - `src`: the UI.
    - `store/` holds the mirror and the chrome state.
    - Also `keyboard/`, `ui/`, `compose/`, and `ipc/`.
    - `src/bindings` is generated by ts-rs (`cargo test`).
- **Tests.**
  - [e2e/COVERAGE.md](e2e/COVERAGE.md) is the test plan. Section 9 lists the
    allowed unit tests. `e2e/plan.spec.ts` checks it against the specs; on a
    count mismatch, copy the counted values it prints into the section 4 table.
  - The harness: `e2e/app.ts` (page helpers), `e2e/control.ts` (the fake's control
    API, including faults and held replies), and `e2e/seeds.ts`.
  - Time moves with `app.advance()`; tests wait with `app.settle()`.
- **Releases.** `.github/workflows/release.yml` keeps a "release X.Y.Z" pull
  request open on `main`; merging it tags the version, builds the universal macOS
  `.dmg`, and publishes the GitHub Release with notes from the commits. Only the
  user merges it. The build reads the OAuth client from the
  `GOOGLE_OAUTH_CLIENT_JSON` Actions secret. The version lives in `package.json`
  (`tauri.conf.json` points at it) and in `src-tauri/Cargo.toml`; release-please
  bumps both. Windows and Linux builds wait until the app works off macOS.
- **Secrets.** `google-oauth-client.json` is git-ignored. Refresh tokens are in
  `tokens.json` (mode 0600) in the app's data folder, not the Keychain.
