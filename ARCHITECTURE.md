# Ubermensch core: architecture

The design of the Rust side, as rewritten from scratch on branch `rewrite`. It
says how the code is split, how data and control flow, and the rules every module
follows. SPEC.md says what the app does; this says how the Rust is built to do it.

## 1. Goals

- **Clean layers.** Each crate has one job and depends only on crates below it.
  Domain rules never touch SQL, HTTP, Tauri, or the clock.
- **No god objects.** Services are small and get their dependencies passed in.
  Nothing holds "everything".
- **One way to do each thing.** One path for writes, one for Gmail calls, one for
  errors, one for time, one for logging.
- **Idiomatic, production Rust.**
  - The AGENTS.md rules: pedantic lints, newtypes, borrowing, no `unwrap`, no `as`.
  - Typed errors.
  - Cancellation-safe async.
  - Clear ownership.
- **Same behavior.** The UI, the E2E suite, and the fake Gmail don't change, and
  the rewrite is done when they pass.

Non-goals: a new schema migration path (the rewrite starts from an empty database;
accounts sign in again), Windows and Linux, and new features.

## 2. The frozen contract

The rewrite is judged against these, which it may not change without a decision:

| Surface      | What is fixed                                                                                                                                                                                                                                                                                                                                                                | Checked by                                                    |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| IPC types    | The 47 files in `src/bindings`: field names, tags, optionality, and doc comments                                                                                                                                                                                                                                                                                             | The gate's bindings step: generated TS must be byte-identical |
| Commands     | `connect`, `submit`, `fetch_rows`, `get_bodies`, `autocomplete`, `add_account`, `sign_in_again`, `open_attachment`, `open_url`, `unsubscribe`, `block`, `blocked_senders`, `unblock`, `sender_threads`, `add_attachment`, `quit_now`, `quit`, `perf_report`, `snooze_options`, `check_split_rule`: names, argument names, results, and error strings where the UI reads them | E2E, through the web bridge                                   |
| Channel      | One `ChannelMessage { rev, ack, updates }` per commit; the update types `list`, `conversation`, `composer`, `account`, `app`; first loads as their own message (SPEC UI state)                                                                                                                                                                                               | E2E, and the writer/channel property test                     |
| Actions      | The 17 `Action` types (`reset` … `moreResults`) and their semantics                                                                                                                                                                                                                                                                                                          | E2E                                                           |
| Test control | The 34 `/__test/*` endpoints `e2e/control.ts` calls, their JSON, and `settle()` meaning "idle"                                                                                                                                                                                                                                                                               | E2E                                                           |
| Fake Gmail   | `crates/ub-fake-gmail`, unchanged                                                                                                                                                                                                                                                                                                                                            | E2E                                                           |
| Packaging    | `tauri.conf.json`, capabilities, `build.rs` inputs (OAuth client file, quota env), binaries `ubermensch` and `e2e-server`, the `fake` feature                                                                                                                                                                                                                                | Release dry run                                               |

## 3. Crates

Crates live under `src-tauri/crates/` with a `ub-` prefix (`ub-domain`, `ub-mime`, `ub-gmail`, `ub-store`, `ub-ipc`, `ub-core`, `ub-bridge`, `ub-testkit`): a crate named `core` would shadow Rust's own. Below and in code paths they are written without the prefix.

```
                 app (src-tauri: Tauri shell, binary `ubermensch`)
                  │        │
                  │     testkit (debug + `fake`: /__test server, e2e-server, demo, perf fixture)
                  │        │
                 bridge ───┘   command dispatch for Tauri and the debug web bridge
                  │
                 core          application: actions, queries, subscriptions, sync, ops, outbox, drafts
     ┌──────┬─────┼──────┬──────────┐
   store  gmail  mime   ipc        (ipc: the frozen IPC types, and nothing else)
     └──────┴─────┴──────┴──▶ domain
```

| Crate     | Job                                                                                                                                                                                                                                                                              | Depends on                                                                               | Never                                       |
| --------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- | ------------------------------------------- |
| `domain`  | Ids and units (newtypes); the small enums rules depend on (`View`, kinds, states); the mail model (thread, message, label, draft, split, pin); view membership and the latest-message rule; split routing; the search and rule query language; snooze times; addressing defaults | `serde`, `thiserror` (and, behind features, `rusqlite` and `ts-rs` for trait impls only) | I/O, async, SQL, HTTP, the system clock     |
| `mime`    | Parse Gmail payloads, sanitize HTML, build outgoing MIME, quote blocks, `List-Unsubscribe`, address lists                                                                                                                                                                        | `ammonia`, `mail-builder`, `domain` (for `Addr`)                                         | I/O                                         |
| `gmail`   | One HTTP client per account, OAuth (loopback + PKCE, refresh, revoke), the quota bucket and priority scheduler, the outcome taxonomy (SPEC Gmail errors), wire types                                                                                                             | `reqwest`, `oauth2`, `tokio`, `domain`                                                   | SQL, Tauri, app state                       |
| `store`   | SQLite: schema, the writer actor, the read pool, typed repositories, `rev` stamping, change tracking, FTS                                                                                                                                                                        | `rusqlite`, `domain`                                                                     | HTTP, Tauri, async runtime types in its API |
| `ipc`     | The contract: every type exported to TypeScript (`src/bindings`), with serde and ts-rs derives                                                                                                                                                                                   | `domain` (`ts`), `serde`, `ts-rs`                                                        | Behavior                                    |
| `core`    | The application: action handlers, queries, subscriptions and patches, the per-account workers (sync, fetch, ops, outbox, drafts, contacts), notifications, snooze timer, launch and shutdown. Takes `ipc` actions in and gives `ipc` read models and channel messages out        | `domain`, `mime`, `gmail`, `store`, `ipc`, `tokio`, `tokio-util`                         | Tauri, HTTP servers                         |
| `bridge`  | One `dispatch(command, args)` over `core`, used by the Tauri commands and by the debug web bridge (`axum`, behind a `web` feature)                                                                                                                                               | `core`, `ipc`, `axum` (optional)                                                         | Business rules                              |
| `testkit` | The `/__test` control server, the `e2e-server` code, demo mode, the perf fixture (written through `store`), the recording `Platform`                                                                                                                                             | `core`, `bridge`, `ipc`, `store`, `ub-fake-gmail`                                        | Being linked into release builds            |
| `app`     | Tauri setup, plugins, window and Dock, the Tauri `Platform`, command handlers that call `bridge::dispatch`                                                                                                                                                                       | `bridge`, `core`, `ipc`, `tauri*`                                                        | Logic beyond wiring                         |

`rusqlite` is used only in `store` (and in `domain`'s `sqlite` impls), `reqwest` only in `gmail` (the one-click unsubscribe POST goes through `gmail`'s HTTP client too), and `tauri` only in `app`. A workspace test (`tests/layers.rs`, reading `cargo metadata`) fails on any other dependency edge, so the layering can't drift.

## 4. Runtime

- **Tauri main thread:** windows, menus, and plugin callbacks only. Commands that read or wait are `async`, so nothing blocks it.
- **One tokio runtime** (Tauri's) for all async work.
- **The writer:** one OS thread that owns the only write connection and runs jobs in priority order: UI actions and first loads, then sync results, then background work (SPEC UI state, Ordering). Callers put jobs in one of three lanes (a mutex and a condvar, not a channel: a channel can't take the highest lane first, and a bounded one would block an async caller's thread when full) and await a oneshot reply; a sync write nothing waits on (`CoreWriter::queue`) drops its reply. The queue is unbounded: callers that await their reply hold it to one job each, and the fire-and-forget writes are a fixed few per Gmail answer. See section 6.
- **The read pool:** a few read-only WAL connections behind `spawn_blocking`, for window fetches, bodies, autocomplete, and search.
- **Per account:** one `AccountWorker`, a supervised task group owned by a `CancellationToken`. Its tasks:
  - poller (`history.list` every 45 s, on focus and wake);
  - fetch queue (priority-ordered, de-duplicated thread fetches);
  - ops runner (queued Gmail mutations, replayed in order);
  - outbox runner (undo send, `drafts.send`);
  - draft reconciler;
  - contacts backfill;
  - the body reads of opened and prefetched conversations: at most one per thread, stopped once no subscription shows the thread.

  Removing or signing out an account cancels the token and waits for the group to stop.

- **App-wide tasks:** the effect router (section 8.4), the snooze timer, and the Dock badge count. Wake from sleep is a poller noticing a gap in app time of more than three poll intervals; notification click-through is a focus within 30 s of a notification shown in the background (SPEC Sync, Notifications).
- **Shutdown:** quitting waits up to 5 s for pending UI actions to commit (SPEC), then cancels the root token and, within the same deadline, waits for every task to end, the account workers' and op lanes' included (a child task group is tracked by its parents), and exits.

## 5. Data flow

### 5.1 A UI action

```
UI ──submit(actions)──▶ bridge::dispatch ──▶ core::Actions::submit
     (numbered, batched)                     │ enqueue writer job (priority: UI)
                                             ▼
                        writer thread: begin tx
                          for each action > last ack:
                            handler(&mut WriteTx, action) → domain decides, store writes
                          commit; ack = last number
                          subscriptions.patches(&changes, &snapshot)
                          emit ChannelMessage { rev, ack, updates }
                          hand Effects to core's effect router
                                             │
                     effect router ──▶ AccountWorker (ops runner / outbox / fetch queue)
                                             │ Gmail calls through gmail::Scheduler
                                             ▼
                        results come back as writer jobs (priority: sync) → same commit path
```

An action handler is a plain function `fn(&mut WriteTx, &Action) -> Result<(), ActionError>`. It reads and writes through repository methods, decides with `domain` rules, and records effects on the transaction. It never sees SQL or a channel.

### 5.2 A subscription

`subscribe` is an action, so it is ordered with the others. The writer computes the first load from the committed snapshot and emits it as its own message. The subscription registry lives on the writer thread, so no lock guards it. After each commit, the registry asks which subscriptions the commit's `Changes` can affect and builds patches only for those. The store's `Changes` say only what was written: thread, message, draft, and outbox ids, and which tables of which account (`Table`). Core alone maps that to read models (`subs/affects.rs`): each read model lists the tables it reads next to the code that builds it (`APP_READS` and `SUMMARY_READS` in `views/account.rs`, the list and search ones in `subs/lists.rs`), and conversations and composers go by the thread and draft ids. A new summary field or view is a change to core only. The patch builders are pure functions over read models: old ids plus new ids give moves, and changed rows are listed whether or not they are in the window.

### 5.3 Sync

A worker fetches from Gmail, parses and sanitizes the result off the writer thread (`mime`), and sends a `SyncWrite` job: a typed value such as `StoreThread(ParsedThread)` or `ApplyHistory(..)`. The writer applies it in a small transaction, so sync never holds the writer for long and UI actions jump the queue.

## 6. Store

- **Schema:** one `schema.sql`, created fresh. A `user_version` pragma guards it. An unknown older file is moved aside and a new one created; there is no migration code.
- **`Writer`:** the actor handle. `call(priority, job) -> impl Future<Output = Result<T, StoreError>>`. Jobs are closures `FnOnce(&mut WriteTx) -> Result<T>`, run in order of priority, then arrival.
- **`WriteTx`:** wraps the transaction. It is the only way to write, and has no raw `execute`. Repositories hang off it: `tx.threads()`, `tx.messages()`, `tx.drafts()`, `tx.outbox()`, `tx.ops()`, `tx.labels()`, `tx.accounts()`, `tx.splits()`, `tx.contacts()`, `tx.settings()`.
  - Every write method stamps `rev` and records in the transaction's `Changes` the rows and the account's tables it wrote, never which read models that makes stale. Change tracking is a property of the write, not something callers remember to do.
  - Writing a message re-derives its thread's labels and stamps the thread, by the domain rule.
  - Effects (`tx.effect(Effect::Ops(account))`) are collected on the transaction and released only after commit.
- **Reads:** `ReadPool::with(|r: &ReadTx| ..)`. `ReadTx` exposes the same read-only repository methods as `WriteTx`, which returns domain types or read models (`ListRow`-like core types), never `rusqlite::Row`.
- **SQL style:**
  - `prepare_cached`;
  - named columns;
  - each statement next to the method that uses it, in that repository's module;
  - no SQL strings built at runtime except the FTS5 query, which `domain` builds and `store` binds.

## 7. Gmail

- **`Gmail`:** one client per account: base URL, shared `reqwest::Client`, and `Arc<Tokens>`. One method per endpoint, returning `Result<T, GmailError>`. A 401 refreshes once inside.
- **`Outcome`:** the SPEC's taxonomy, one variant per row of the Gmail errors table (offline, no response, unauthorized, invalid grant, rate limited, server error, not found, conflict, rejected with Google's reason). Callers match on it; nothing parses error strings.
- **`Scheduler`:** the quota bucket (1,000 capacity, 4,800 per minute refill, background cap) and at most 8 requests at once per account, by priority 1 to 6. `sched.run(Priority, cost, future)` is the only way to call Gmail.
- **`Retry`:** one policy type (back-off with jitter and `Retry-After`) used by the workers, with the per-call exceptions SPEC names (`drafts.create`, `drafts.send`).
- **OAuth:** the loopback server, PKCE, state check, token refresh shortly before expiry, and revoke. Tokens persist through a `TokenStore` port with two implementations, the 0600 file and memory.
- **One-click unsubscribe:** the `POST` goes through the same `reqwest::Client`, to `https` only and following no redirect. Debug builds also allow loopback `http`, where the E2E server stands in for a list.

## 8. Core

### 8.1 Services, not one engine

`core::App` is a small root that owns the writer, the read pool, the scheduler, the `Platform`, the clock, and a map of `AccountWorker`s. It exposes narrow handles:

| Handle      | Does                                                                             |
| ----------- | -------------------------------------------------------------------------------- |
| `Actions`   | `submit`, `last_ack`; turns numbered actions into writer jobs                    |
| `Queries`   | Window rows, bodies, autocomplete, snooze options, sender threads                |
| `Accounts`  | Add account (sign-in), sign in again, remove; starts and stops workers           |
| `Mail`      | Attachments, open URL, unsubscribe, block and blocked senders                    |
| `Lifecycle` | Launch (wake snoozes, settle the outbox, untouched drafts), focus and wake, quit |

`Lifecycle::launch` queues one writer job before any page can connect: due snoozes wake, the outbox settles, and untouched drafts go. Then the snooze timer and the account workers start. `Lifecycle::focus` polls every account and returns the notification a focus counts as a click on, which the shell opens.

Each handle holds only the `Arc`s it needs. A handle is a struct with methods, not a trait: there is one implementation.

### 8.2 Account worker

`AccountWorker::new(parts, grant)` builds a worker from its `WorkerParts` (its Gmail client on the sign-in's grant, its task group, the account's op lane), and `start(replaced)` starts the task group of section 4 once the worker it replaces has stopped. Its tasks share an `AccountCtx`: the account id, its `Gmail`, its scheduler slot, its `Kicks` (`tokio::sync::Notify` for ops, outbox, poll, and online), and the writer handle. Tasks talk to each other only through kicks and the database.

### 8.3 Ports

| Port         | Implementations                     | Why a trait                                                    |
| ------------ | ----------------------------------- | -------------------------------------------------------------- |
| `Platform`   | Tauri (`app`), recorder (`testkit`) | Notifications, badge, open URL or path, focus, sign-in browser |
| `Clock`      | System, manual (`testkit`)          | Tests move time with `app.advance()`                           |
| `TokenStore` | 0600 file, memory                   | Tests and demo keep tokens out of the user's folder            |

Each has exactly two implementations. AGENTS.md forbids traits with one.

### 8.4 Writer-side modules and effects

Rules that write live in modules the writer runs, each a set of functions over `&mut WriteTx` and the writer state: `actions` (one module per group of UI actions), `ops` (queueing, revert, rejection), `outbox` (a pure send state machine and its steps), `drafts` (whose `lifecycle` alone reads and changes a draft's stage, waits, and Gmail copy, through named transitions, and keeps two writes of one draft to Gmail from overlapping; the store keeps that state as plain data), `snooze` (waking due threads, as the same label change as Return to inbox), and `notify` (which new mail notifies, decided in the poll's own transaction so open conversations and stored messages agree). They record `Effect`s; the writer releases them to the router only after the commit lands.

The router (`effects::Router`) is one task that hands each effect to its worker: fetches, Gmail listings, ops, outbox and draft work go to the account's worker; `Notify` goes to the `Notifier`, which posts it and remembers it for click-through; `Snoozes` re-arms the snooze timer; `Badge` kicks the badge task, which recounts unread mail once per wake, so a burst of commits costs one count. Each account's ops run in an `OpsLane` that outlives its worker, so a replaced worker's in-flight op lands before the next one replays, and a second sign-in never sends a queued op twice. An op's row goes only when its result is written, so an op that quitting cuts off stays queued and is sent again on the next launch; `batchModify` and `labels.create` (by its 409) make that harmless.

Every task, timer, kick, and queued job counts in one `Activity`, so the E2E server's `settle()` can tell when the core is idle, and its manual clock moves app time.

## 9. Errors and logging

- Each crate has its own `thiserror` enum: `domain::RuleError`, `store::StoreError`, `gmail::GmailError` (carrying an `Outcome`), `core::Error`.
- `anyhow` only in `app` and `testkit` glue.
- Commands map `core::Error` to the strings the UI reads, and only in `bridge`.
- `tracing` with targets per crate. One line per event, with context, and no user content (addresses, subjects, bodies) or secrets. Noisy third-party targets are filtered at the subscriber.

## 10. Testing

- **E2E** (unchanged) is the main judge, through `testkit`'s `e2e-server`.
- **Pure unit tests**, ported from the old code with the same cases (the kept unit tests in e2e/COVERAGE.md):
  - `domain`: query language, splits, snooze;
  - `mime`: sanitize, no-panic fuzz, unsubscribe header;
  - `gmail`: scheduler, OAuth.
- **Core tests** (`ub-core/src/tests`) for what E2E can't observe: a started `App` on a temporary database.
- **Perf bench:** the ignored `perf` tests in `ub-core` time local search and autocomplete per keystroke on the perf fixture (`perf-fixture`, fake feature) against SPEC Performance. The UI budgets so far were measured in Chrome through the web bridge on a development Mac, not on an M1 MacBook Air in WKWebView.
- **Property test:** the writer/channel invariants (`store` + `core`), replacing `tests/writer_channel.rs`.
- **Layering test** (section 3), and a test that no crate but `store` names `rusqlite`.
- **Mutation probes** for every new guard, per AGENTS.md.

## 11. Conventions

- **Modules:** one concept per file. `mod.rs` only re-exports and documents. Items are `pub(crate)` unless another crate needs them.
- **Names:** types are nouns (`OutboxRunner`), functions are verbs (`apply_history`), no `Manager`, `Helper`, or `Util`. Booleans read as questions (`is_list`, `has_body`).
- **Ownership:** per AGENTS.md: borrow to read, take by value to keep, `Arc<T>` written out, and `&Arc<T>` only to clone into a task.
- **Async:**
  - no lock across `.await`;
  - `parking_lot` locks for short sections;
  - every spawned task belongs to a `CancellationToken`;
  - `select!` arms are cancellation-safe or documented.
- **Ids and units** are newtypes (`AccountId`, `ThreadId`, `GmailThreadId`, `Millis`, `Rev`, `ActionNo`). They convert to raw values only at SQL, serde, and IPC.
- **Docs:** every public item has a one-line doc saying what it is. Comments say why, not what.

## 12. Order of work

The rewrite landed in slices, each green on its own E2E specs before the next started:

1. **Skeleton:** `domain` ids, the `store` schema and writer, subscriptions and the channel, the `ipc` types, the `bridge` with its web bridge, a minimal `testkit`, and the `app` shell.
2. **Accounts, sync, reading.**
3. **Actions:** triage, undo, labels, offline.
4. **Compose:** replies, new mail, drafts, undo send, recipients, attachments.
5. **The rest:** search, splits, snooze, notifications, settings, palette, unsubscribe and block.
6. **Finish:** demo mode, the perf fixture and bench, and the full gate (`vp run gate`), which is now the only gate.

## 13. Decisions from review

1. **The debug web bridge lives in `bridge`**, behind a `web` feature, so `testkit` and the debug `app` share one dispatch. Release builds leave the feature off.
2. **Logging uses `tracing`**, with spans around Gmail calls and writer jobs.
3. **The `e2e-server` binary stays in the root package**, as a thin `main` over `testkit`, so `e2e/global-setup.ts` builds it as before.
4. **`ipc` sits below `core`** and holds only the contract types, so `core` takes and returns them directly instead of mirroring 47 types with conversions. `domain`'s `sqlite` and `ts` features add trait impls for its newtypes and small enums, since the orphan rule keeps those impls out of `store` and `ipc`.
5. **Starting and launching are two steps.** `App::start` opens the database and the read pool and starts the writer; nothing talks to Gmail until `Lifecycle::launch`. The E2E server seeds accounts between the two, as the old server did before its engine ran. The Gmail endpoints, OAuth client, and request budget are part of `core::Config`.

## 14. Running and building

- **Debug web bridge:** debug builds started with `UBERMENSCH_WEB_BRIDGE=1` serve the command surface on `127.0.0.1:1430`, so the UI can run in a browser against the real core for automated checks. Release builds leave the `web` feature off and never start it.
- **Demo mode:** `vp run demo` is a debug build with `--features fake` and `UBERMENSCH_FAKE=1`. It runs the in-process fake Gmail with demo mail and two accounts, on a data folder in the system's temporary directory that is emptied at each launch.
- **Attachments** are picked with the webview's file input and copied into the draft's folder by the `add_attachment` command, so no dialog plugin is needed.
- **App icon:** `src-tauri/icons/app-icon.svg` is the source. `vp run tauri icon src-tauri/icons/app-icon.svg` makes every size; delete the `android` and `ios` folders it adds.
