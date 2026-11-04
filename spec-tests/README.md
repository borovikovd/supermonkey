# Kept tests from the old Rust code

The rewrite deletes the old Rust sources. These files hold the old pure tests
(COVERAGE section 9) as written against the old APIs, so their cases can be
ported to the new crates without losing any:

| Here                                                          | Old home                                | Port to                 |
| ------------------------------------------------------------- | --------------------------------------- | ----------------------- |
| `ub-rules/query.rs`, `splits.rs`, `snooze.rs`                 | `crates/ub-rules/src` (inline tests)    | `ub-domain`             |
| `ub-mime/sanitize.rs`, `no_panic.rs`, `fixtures/hostile.html` | `crates/ub-mime/tests`                  | `ub-mime`               |
| `ub-mime/unsubscribe.rs`                                      | `crates/ub-mime/src` (inline tests)     | `ub-mime`               |
| `ub-gmail/scheduler.rs`, `oauth.rs`, `common/`                | `crates/ub-gmail/tests`                 | `ub-gmail`              |
| `core/writer_channel.rs` (+ regressions)                      | `src-tauri/tests`                       | `ub-store` / `ub-core`  |
| `core/clock.rs`                                               | `src-tauri/src/clock.rs` (inline tests) | `ub-core`               |
| `core/logging.rs`, `perf_search.rs`, `fixture.rs`             | `src-tauri/tests`, `src-tauri/src`      | `ub-core`, `ub-testkit` |

A file leaves this folder once its cases pass in the new crate. The folder is
gone before the rewrite merges.
