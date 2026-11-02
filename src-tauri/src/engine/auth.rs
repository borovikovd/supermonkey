//! Google sign-in (SPEC Accounts): system browser, loopback redirect, PKCE.

use crate::LogErr as _;
use std::sync::Arc;

use ub_gmail::{Gmail, OAuthTokens, Priority, cost};

use super::EngineRef;
use crate::model::AccountId;

/// A completed sign-in.
struct SignIn {
    email: String,
    tokens: Arc<OAuthTokens>,
    refresh_token: String,
}

impl SignIn {
    /// Keeps the refresh token in the token store for the next launch.
    fn store_token(&self, e: &EngineRef) -> anyhow::Result<()> {
        e.store
            .store_refresh_token(&self.email, &self.refresh_token)
            .map_err(|err| anyhow::anyhow!("Keychain: {}", err.0))
    }
}

async fn sign_in(e: &EngineRef) -> anyhow::Result<SignIn> {
    if e.config.oauth.client_id.is_empty() {
        anyhow::bail!(
            "This build has no Google OAuth client. Put google-oauth-client.json in the project and rebuild."
        );
    }
    let platform = e.platform.clone();
    let auth = ub_gmail::authorize(&e.config.oauth, &e.http, move |url| {
        platform.open_url(url);
    })
    .await
    .map_err(|err| anyhow::anyhow!("Sign-in failed: {:?}", err.outcome))?;
    let refresh_token = auth.refresh_token.0;
    let tokens = Arc::new(OAuthTokens::with_access_token(
        e.config.oauth.clone(),
        e.http.clone(),
        refresh_token.clone(),
        auth.access_token,
    ));
    let gmail = Gmail::new(
        e.config.gmail_base.clone(),
        "",
        e.http.clone(),
        tokens.clone(),
    );
    let profile = e
        .scheduler
        .account("sign-in")
        .run(Priority::P1, cost::GET_PROFILE, gmail.get_profile())
        .await
        .map_err(|err| anyhow::anyhow!("Could not read the Gmail profile: {:?}", err.outcome))?;
    Ok(SignIn {
        email: profile.email_address,
        tokens,
        refresh_token,
    })
}

pub async fn add_account(e: &EngineRef) -> anyhow::Result<AccountId> {
    let signed_in = sign_in(e).await?;
    signed_in.store_token(e)?;
    let email = signed_in.email.clone();
    let id = e
        .writer
        .call(move |ctx| crate::accounts::create(ctx, &email))
        .await?;
    e.start_account(id, signed_in.email, signed_in.tokens);
    e.platform.focus_window();
    Ok(id)
}

pub async fn sign_in_again(e: &EngineRef, account: AccountId) -> anyhow::Result<()> {
    let expected: String = e.reads.with(|c| {
        Ok(
            c.query_row("SELECT email FROM accounts WHERE id = ?1", [account], |r| {
                r.get(0)
            })?,
        )
    })?;
    let signed_in = sign_in(e).await?;
    let email = &signed_in.email;
    if !email.eq_ignore_ascii_case(&expected) {
        anyhow::bail!("You signed in as {email}. Sign in as {expected} to reconnect this account.");
    }
    signed_in.store_token(e)?;
    e.writer
        .call(move |ctx| {
            ctx.tx.execute(
                "UPDATE accounts SET needs_signin = 0, sync_status = 'idle' WHERE id = ?1",
                [account],
            )?;
            ctx.touch_app();
            ctx.touch_summary(account);
            Ok(())
        })
        .await?;
    e.start_account(account, signed_in.email, signed_in.tokens);
    Ok(())
}

/// After Remove account: stop sync, revoke and delete the token, delete files.
pub async fn forget(e: EngineRef, account: AccountId, email: String) {
    e.stop_account(account);
    if let Ok(Some(token)) = e.store.load_refresh_token(&email) {
        ub_gmail::revoke(&e.config.oauth, &e.http, &token)
            .await
            .log_err("revoke token");
    }
    e.store
        .delete_refresh_token(&email)
        .log_err("delete refresh token");
    match std::fs::remove_dir_all(super::attachments::account_dir(&e, account)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            log::warn!("delete attachments of account {account}: {err}");
        }
        _ => {}
    }
}
