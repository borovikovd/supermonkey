//! Account workers (ARCHITECTURE.md, section 8.2), filled by the accounts and
//! sync slice: `AccountWorker::spawn(deps, account, tokens, cancel)` starts a
//! task group under its own [`crate::activity::Tasks::child`]: the history
//! poller, the fetch queue, and the contacts backfill. Sign-in, sign in
//! again, and remove account start and stop it.
