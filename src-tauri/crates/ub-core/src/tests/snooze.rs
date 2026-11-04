//! The snooze timer on a manual clock (SPEC Snooze): the E2E clock fires
//! every timer due by an advance, so it can't tell a timer re-armed for a
//! new snooze from one that looks again after a minute.

use std::num::NonZeroUsize;
use std::sync::Arc;

use ub_domain::{Millis, ModifyKind};
use ub_ipc::{Action, ModifyAction, SnoozeChange};
use ub_store::ReadPool;

use super::{Harness, eventually, seed};
use crate::Config;
use crate::activity::Activity;
use crate::clock::ManualClock;

#[tokio::test]
async fn a_snooze_set_rearms_the_timer_for_its_time() {
    let activity = Activity::new();
    let start = Millis(1_900_000_000_000);
    let clock = Arc::new(ManualClock::new(Arc::clone(&activity), start));
    let mut h = Harness::start_with(
        tempfile::tempdir().unwrap(),
        Arc::clone(&activity),
        Arc::<ManualClock>::clone(&clock),
    )
    .await;
    let (account, threads) = h.write(seed).await;
    let thread = threads[0];
    h.app.lifecycle().launch();
    eventually("the core is idle after launch", || activity.is_idle()).await;

    // Ten seconds out: well inside the timer's minute between looks.
    h.act(vec![Action::Modify(ModifyAction {
        account,
        kind: ModifyKind::Snooze,
        threads: vec![thread],
        add: Vec::new(),
        remove: vec!["INBOX".into()],
        snooze: Some(SnoozeChange::Set(start.plus(10_000))),
        undoable: true,
    })])
    .await;
    eventually("the timer is re-armed", || activity.is_idle()).await;
    let pool = ReadPool::open(&h.dir.path().join(Config::DB_FILE), NonZeroUsize::MIN).unwrap();
    let snoozed = || {
        pool.with(|r| r.threads().row(thread))
            .unwrap()
            .unwrap()
            .snooze_until
    };
    assert_eq!(snoozed(), Some(start.plus(10_000)));

    clock.advance(10_000);
    eventually("the thread wakes at its time", || snoozed().is_none()).await;
}
