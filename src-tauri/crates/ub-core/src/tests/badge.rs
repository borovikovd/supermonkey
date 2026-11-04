//! The Dock badge's recount after commits.

use super::{Harness, eventually, seed};
use crate::activity::DEFERRED_KICK;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commits_while_the_badge_is_being_set_make_one_more_count() {
    let h = Harness::start().await;
    let (account, _) = h.write(seed).await;
    eventually("the launch badge is set", || h.activity.is_idle()).await;
    let before = h.platform.badges.lock().len();

    // The Dock is slow: the first recount waits while 20 more commits land.
    h.platform.badge_gate.close();
    let flip = move |on: bool| {
        move |tx: &mut ub_store::WriteTx<'_>| tx.accounts().set_needs_signin(account, on)
    };
    h.write(flip(true)).await;
    eventually("a recount waits on the Dock", || {
        h.platform.badge_gate.waiting() == 1
    })
    .await;
    for i in 0..20 {
        h.write(flip(i % 2 == 0)).await;
    }
    // Every commit's effect is routed: only the waiting count is left.
    eventually("the commits' effects are routed", || {
        h.activity
            .snapshot()
            .keys()
            .all(|label| *label == "badge" || *label == DEFERRED_KICK)
    })
    .await;
    h.platform.badge_gate.open();
    eventually("the core goes idle", || h.activity.is_idle()).await;
    assert_eq!(
        h.platform.badges.lock().len() - before,
        2,
        "the held count, then one for everything that came meanwhile"
    );
}
