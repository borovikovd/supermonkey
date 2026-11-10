# AGENTS.md

How to build and change a Tauri app: a Rust core with a web UI. The rules apply to
any coding agent and any human, on any Tauri project. Everything specific to this
one is in [Project specifics](#project-specifics) at the end.

## How to work

- The spec is the source of truth for behavior and is always current. A change to
  behavior starts in the spec: update it first, then the tests and the code, in the
  same commit. Where the spec is silent, decide, and write the decision into the
  spec. There is no separate decisions log. Ask only when a choice is hard to undo.
- Work is done when the gate is green and you have used the change in the running
  app. Passing checks alone is not done.
- Report what happened: which tests failed or are flaky, which steps you skipped,
  what you didn't check.
- A bug you find outside your task gets fixed with a test or written down, never
  left unmentioned.

## Git and releases

- No AI attribution anywhere: no `Co-Authored-By` trailer, no "generated with"
  line, even when a tool suggests one.
- One logical change per commit, with a body that says why. Subjects follow
  Conventional Commits (`type(scope): summary`, `!` for breaking), checked by a
  commit-msg hook.
- Release notes come from `feat`, `fix`, and `perf` subjects, so write those for
  the people who use the app.
- A tool that reads the commits (release-please or similar) cuts releases. Never
  bump versions or edit the changelog by hand. To release only other changes, use
  the tool's override (release-please: a `Release-As: x.y.z` footer).
- Squash-merge a long-lived branch into one commit, so its internal fixes don't
  become release notes. Keep the branch for history.
- Commit only on a green gate. Never skip hooks. Push after each commit. Never
  force-push, rewrite pushed history, or delete branches without asking.
- Parallel agents each work in their own worktree. Shared lists (test plans, id
  tables) conflict on merge: merge by union and renumber. Delete finished
  worktrees with their build folders.

## Build and the gate

- Every command is a named task in one task runner. No shell scripts in the build.
- One `gate` task runs every check: Rust tests and lints, UI lint, format and type
  check, generated-file freshness, and the end-to-end suite. Each check is also a
  task of its own. Warnings fail the gate.
- **Check what you ship.** Lint and build each configuration that ships, not just
  the workspace as a whole. Cargo unifies features across a workspace, so code that
  only a test or debug feature uses passes a workspace build and warns in the
  app's own debug and release builds.
- Checks need no secrets. Where a release build requires one, the gate points it at
  a committed placeholder with fake values.
- No task cache may skip a check unless it tracks every input the check reads.
- No unused dependencies, in Rust or the UI.
- Test runs clean up after themselves. A test server deletes its data folder on
  every exit, including SIGTERM, or a few hundred runs fill the disk. Rust build
  folders fill it too: clean them when space runs low.

## Architecture

- Rust owns all state. The UI holds a mirror of it plus the user's pending actions,
  and never works out truth the core doesn't send.
- One writer owns the database. Reads use a pool; writes go through the writer.
- IPC types are defined once, in Rust, and generated for the UI (ts-rs, specta).
  Never hand-write a duplicate. Commit the generated files; the gate fails when
  they are stale.
- Crates form layers, and a test checks the dependency edges. External services sit
  behind one client that owns auth, rate limits, retries, and error mapping.
- **One path per concern.** Quitting, rendering untrusted content, and binding keys
  each go through exactly one path; a second path is a second policy that drifts.
  Platform defaults count as paths: the default macOS menu's Quit bypasses Tauri's
  exit hooks.
- Background work is ordered by what the user is looking at. On first sync, what is
  on screen loads before backfills and secondary data.
- The bundle identifier names the app's data folder. Changing it orphans the
  user's data unless the app migrates it: decide that before renaming.
- Every way to run the app (dev, demo, test, release) is one command from a known
  state. Dev, demo, and test never touch the user's real data, and demo data is
  rebuilt at each launch.
- To rewrite a layer, freeze its contract first: the generated types, the
  end-to-end suite, and the spec judge the new code. Work on a branch, slice by
  slice, each slice green.

## Tauri security

- Least privilege: each window gets only the capabilities it uses. Add a plugin or a
  wide permission (shell, fs, http) only with a stated need.
- Strict CSP: no `unsafe-eval`, no remote scripts. Untrusted HTML renders in a
  sandboxed iframe whose CSP blocks scripts.
- External content is hostile. Sanitize it in Rust, never inject it into the app's
  own DOM, and open links in the system browser.
- Validate every command's input in Rust. The webview is not trusted.
- Requests to URLs taken from untrusted content: https only, a short timeout, and no
  redirects to private or loopback addresses.
- A file saved from untrusted content gets the OS quarantine flag (macOS
  `com.apple.quarantine`) before it can be opened.
- Debug servers and test hooks are compiled out of release builds, not switched
  off. A debug server on localhost checks `Origin` and `Sec-Fetch-Site`, because
  any web page can send requests to it.
- The release webview's origin (`tauri://localhost`) differs from dev's
  (`http://localhost`). Try the release build before calling a change done.

## Secrets

- Credential files stay git-ignored. Never commit, print, or paste them into logs,
  tests, or chat.
- Tokens live in the OS keychain, or in a file only the user can read when the
  keychain gets in the way (an unsigned macOS build makes it prompt after every
  update). Never in the database, the repo, or logs. Never log tokens, auth codes,
  or user content.

## Concurrency

Most bugs in an app that syncs live here. Assume every `await` lets the world change.

- **Decide every interruption.** For each operation that takes time (a send, an
  upload, a load, a sync, a quit), the spec says what happens when anything else
  arrives mid-way: the user closes, repeats, undoes, or quits; the data changes or
  is deleted elsewhere; its owner goes away; the network drops. An undecided case
  is a bug waiting.
- **An action is bound to what the user saw.** Its target (account, items) is fixed
  when the user acts. Anything that runs later (a reply, a timer, a frame callback)
  checks that its target still holds before it acts.
- **Stale answers.** Before applying a response, check a version, generation, or
  sequence number, and drop it if a newer one already landed.
- **Local changes win.** A user's write made while a remote read was in flight
  survives that read: re-apply pending local changes on top of what comes back.
- **Ids are never reused.** Use `AUTOINCREMENT` or a stored sequence, never
  `max(id) + 1`. Subscription and listener ids are unique too.
- **Idempotent retries.** Anything sent may arrive twice or not at all. Number each
  request, have the receiver drop numbers it already applied, retry with backoff.
- **Everything in progress ends.** A spinner, a busy flag, or a "sending" state ends
  on every path: success, failure, cancellation, and panic. Release such state in a
  drop guard, not at the end of the happy path.
- **Locks.** Hold them briefly, never across `.await`, and never while calling
  outside code (callbacks, clocks, I/O). State that must agree lives under one
  lock.
- **Isolation.** A slow or failing resource (one account, one connection) never
  blocks the others: each gets its own queue, slots, and limits.
- **Shutdown** waits, with a deadline, for work the user already did to commit, then
  exits anyway.
- **Tests.** Every concurrency fix comes with a test that forces the interleaving,
  for example a fake server that holds a reply until the test releases it, or a
  note why it can't be forced.

## Rust

- Production lints: `clippy::pedantic` plus strict extras, rustdoc lints denied.
  Never loosen them. Use `#[expect(lint, reason = "...")]`, never a bare `#[allow]`.
  A lint or threshold is left off only on evidence that its hits are false
  positives: the count and what a sample showed, written beside it in the config.
- No `unwrap` or `expect` outside tests, no `unsafe`, no silent `as` casts
  (`try_from`).
- **Ownership.** Borrow to read, take by value to keep. Never take `&T` and clone
  inside. No alias hides an `Arc` or `Box`. Take `&Arc<T>` only to clone the handle
  into a task. Interior mutability only where shared ownership needs it.
  `parking_lot` locks; an async mutex only when held across `.await`, with the
  reason. Where a lint checks one of these rules (clippy `redundant_clone`), enable
  it; the rest is what the focused ownership review is for.
- **Types.** Every kind of id and unit is a newtype, converted to raw types only at
  the edges (SQL, serde, IPC). No trait with a single implementation. Parsers return
  slices or `Cow`. Helpers that only read take a connection, not the app context.

## UI

- Strict, type-aware lint: no `any`, no non-null assertions, every suppression names
  its rule and reason. Never suppress a framework's correctness rules (such as
  React's hooks rules).
- **Design tokens.** One file defines every color, text size, radius, shadow, font,
  and fixed width, named by role. Switch off the CSS framework's own palette and
  scales, so only the tokens produce classes. The gate rejects color literals and
  one-off values (Tailwind's `x-[...]`) in components, and two token kinds that make
  the same class (Tailwind 4: `--color-x` and `--text-x` both make `text-x`).
- Layout holds up with long text, small windows, and empty states. Minimum heights,
  not fixed ones.
- Never show a blank or half-built screen. Show what is already known (a cached
  summary, a preview) at once. A loading indicator appears only after a short
  delay, so fast loads don't flash.
- A local stand-in for a remote result (instant local search, a cached count) must
  be exact, or absent until the remote answer arrives. Never a superset: a filter
  the local code can't evaluate must not match everything.
- Every screen shows only the hints and actions that work there.
- The input path (keydown to paint) has performance budgets, measured on a fixture
  of realistic size. Keep work off it.

## Errors and logs

- Never drop an error silently. Return it, or log it with context. `let _ =` only
  for results with nowhere to go (a send to a closed channel).
- Logs are for people: one line per event, with context, no user content or secrets.
- Filter noisy third-party crates by target at the logger; don't raise the global
  level.

## Testing

### What a good test is

- **Behavior through the real app.** End-to-end tests drive the built UI against
  the real Rust core. External services are replaced by a fake that can produce any
  outcome: errors, timeouts, held or late replies, data changed elsewhere. Tests
  never touch the network or real accounts.
- **Expected values from outside the code:** the spec, a fixture, or a literal
  worked out by hand. Never computed by the code under test; never just "a mock was
  called".
- **Test first.** Write the test before the fix or feature, run it, and see it fail
  for the right reason; then make it pass. That one red run is the proof the test
  can catch the bug. Don't break working code afterwards to re-check it.
- **Every bug fix comes with a test that failed before the fix.** A known bug not
  yet fixed is an expected-fail test with a `BUG:` reason.
- **Conditions, never sleeps.** The app exposes its pending work (requests,
  timers, queued actions) so a test can wait until it is idle, and tests move app
  time with a fake clock.
- **Stable selectors.** Tests find elements by role, label, and app-owned `data-*`
  attributes, not by utility classes or a UI library's internals, so the UI can
  change without rewriting the suite.

### What to cover

The test plan is a document with an id per case, and each test title starts with
its id. A new user-visible behavior adds a case and a test. Plan along these axes:

- **Every effect × every path that triggers it.** If opening marks an item read,
  test click, key, auto-advance, notification, and deep link.
- **What the user perceives:** text size and contrast, no overlap, fit in a small
  window, an accessibility check (axe) per screen. "Visible" only means present.
- **Startup and modes:** first launch, restart with data, demo mode, an old
  database.
- **Failure paths:** offline, a server error at each step, timeouts, late replies.
- **Interruptions:** a grid of each operation that takes time × each event that
  can arrive mid-way. Every cell names a test, or says why it can't happen; the
  gate fails on an empty cell.
- **Realistic limits.** The fake applies production rate limits and latency by
  default, or the suite runs under both. An unlimited fake hides ordering and
  starvation bugs.

### Keeping the suite honest

- A flaky test is a bug. Never retry until green. Record its failure rate and fix
  the race.
- Unit tests only for complex pure logic the end-to-end suite can't reach (parsers,
  schedulers, rule engines).
- The gate checks the test plan against the suite: every case has a test, and
  every test id is a case.

## Review and QA

Tests catch what their author thought of. Close the gap from outside:

- **Use the app.** Look at every UI change in the running app. After each feature,
  someone who didn't write it (a fresh agent or a human) checks every screen,
  shortcut, and mode, small windows and long text, the console and the logs.
- **Real data before a release.** Use a real account of realistic size. Fakes miss
  slow networks, quotas, and odd data.
- **Focused reviews.** After large changes, run separate passes for ownership,
  concurrency, security, and test strength; a general code review does not replace
  them. Every finding is verified before it is fixed, and every fix comes with a
  test or a note why none is possible.
- **Review by flow, until clean.** Slice a review along data flows end to end, not
  by directory: a race spans the UI, the core, a worker, and the remote service. A
  concurrency pass checks every await point against what can arrive there (the
  user acts again, quits, or switches; the data changes elsewhere; the network
  drops). Repeat a pass until a round finds nothing: fixes bring new findings.
- **Learn from escapes.** When a bug gets past the gate, add the test or rule that
  would have caught it, here if it is general.

## Project specifics

The only section that changes from project to project.

- **Product:** Supermonkey, a keyboard-first Gmail client for macOS. Tauri 2, React
  19 with the React Compiler, TypeScript 7, Tailwind 4, Vite+, Rust with SQLite.
- **Docs:** [SPEC.md](SPEC.md) (behavior), [ARCHITECTURE.md](ARCHITECTURE.md)
  (design).
- **Commands.** `vp` (Vite+) for JS tooling, never `npm`, `pnpm`, or `npx`. `cargo`
  runs from `src-tauri`.

  ```sh
  vp install            # dependencies
  vp run demo           # app against the in-process fake Gmail, fresh data each launch
  vp run tauri dev      # app against real Gmail (needs google-oauth-client.json)
  vp run tauri build    # release .app
  vp check --fix        # UI format, lint, type check
  vp run gate           # every check; run before each commit
  vp run <task>         # one step: test:rust, lint:rust, lint:ts, typecheck, bindings, e2e
  ```

- **Layout.**
  - `src-tauri/src`: the Tauri shell and the `e2e-server` and `perf-fixture`
    binaries.
  - `src-tauri/crates`, lowest layer first (`src-tauri/tests/layers.rs` checks the
    edges): `sm-domain` (newtypes, mail model, views, splits, the query language),
    `sm-mime`, `sm-gmail` (client, quota, OAuth, token store), `sm-store` (schema,
    writer, read pool), `sm-ipc` (types generated into `src/bindings`), `sm-core`
    (actions, subscriptions, per-account workers), `sm-bridge` (command dispatch
    for Tauri and the debug web bridge), `sm-testkit` (E2E server, demo, perf
    fixture), `sm-fake-gmail`.
  - `src`: the UI: `store/` (mirror and chrome state), `keyboard/`, `ui/`,
    `compose/`, `ipc/`; `src/theme.css` holds the design tokens.
- **Tests.** [e2e/COVERAGE.md](e2e/COVERAGE.md) is the test plan, one line per case;
  its "Kept unit and property tests" list holds the allowed unit tests.
  `e2e/plan.spec.ts` checks it and `e2e/tokens.spec.ts` checks the tokens.
  Harness: `e2e/app.ts`, `e2e/control.ts` (faults, held replies), `e2e/seeds.ts`.
  `app.advance()` moves time; `app.settle()` waits for idle.
- **Releases.** `.github/workflows/release.yml` keeps a "release X.Y.Z" pull request
  open on `main`; merging it builds the universal `.dmg` and publishes the release.
  Only the user merges it. The build reads the OAuth client from the
  `GOOGLE_OAUTH_CLIENT_JSON` secret. Versions live in `package.json` and
  `src-tauri/Cargo.toml`.
- **Secrets.** `google-oauth-client.json` is git-ignored;
  `src-tauri/lint/oauth-client.json` is the gate's placeholder. Refresh tokens are in
  `tokens.json` (mode 0600) in the app's data folder.
