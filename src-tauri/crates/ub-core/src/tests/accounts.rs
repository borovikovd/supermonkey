//! Signing out and in: what a worker reports, racing a sign-in.

use super::{ACCOUNT_EMAIL, Harness};

async fn needs_signin(h: &Harness, account: ub_domain::AccountId) -> bool {
    h.write(move |tx| Ok(tx.read().accounts().get(account)?.unwrap().needs_signin))
        .await
}

#[tokio::test]
async fn a_sign_out_for_a_grant_older_than_the_last_sign_in_changes_nothing() {
    let h = Harness::start().await;
    let account = h.write(|tx| tx.accounts().create(ACCOUNT_EMAIL, "A")).await;
    let workers = h.app.workers();
    let refused = workers.grants().current(account);

    // A sign-in lands, then the sign-out a worker on the old grant reported.
    h.app.accounts().register(ACCOUNT_EMAIL).await.unwrap();
    workers.mark_signed_out(account, refused);
    assert!(!needs_signin(&h, account).await, "the newer sign-in stands");

    // The current grant refused signs the account out.
    workers.mark_signed_out(account, workers.grants().current(account));
    assert!(needs_signin(&h, account).await);
}
