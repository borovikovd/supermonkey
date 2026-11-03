//! Google sign-in (SPEC Accounts): system browser, loopback redirect, PKCE.

use crate::LogErr as _;
use std::sync::Arc;

use ub_gmail::{Gmail, OAuthTokens, Priority, RefreshToken, cost};

use super::Engine;
use crate::model::AccountId;

/// A completed sign-in.
struct SignIn {
    email: String,
    tokens: Arc<OAuthTokens>,
    refresh_token: RefreshToken,
}

impl SignIn {
    /// Keeps the refresh token in the token store for the next launch.
    fn store_token(&self, e: &Engine) -> anyhow::Result<()> {
        e.store
            .store_refresh_token(&self.email, &self.refresh_token)
            .map_err(|err| anyhow::anyhow!("Keychain: {}", err.0))
    }
}

/// Starting a sign-in cancels the one before it: the user left it in the
/// browser, and its loopback listener would otherwise wait forever.
async fn sign_in(e: &Engine) -> anyhow::Result<SignIn> {
    // A removed account still being revoked: its revoke would also kill a
    // grant for the same address made now.
    drop(crate::clock::parked(e.forgetting.write()).await);
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    let previous = e.sign_in_cancel.lock().replace(cancel);
    if let Some(previous) = previous {
        // Err means that sign-in already finished.
        let _ = previous.send(());
    }
    tokio::select! {
        signed_in = sign_in_once(e) => signed_in,
        _ = cancelled => anyhow::bail!("Sign-in cancelled"),
    }
}

async fn sign_in_once(e: &Engine) -> anyhow::Result<SignIn> {
    if e.config.oauth.client_id.is_empty() {
        anyhow::bail!(
            "This build has no Google OAuth client. Put google-oauth-client.json in the project and rebuild."
        );
    }
    let auth = ub_gmail::authorize(&e.config.oauth, &e.http, |url| {
        e.platform.open_url(url);
    })
    .await
    .map_err(|err| anyhow::anyhow!("Sign-in failed: {:?}", err.outcome))?;
    let refresh_token = auth.refresh_token;
    let tokens = Arc::new(OAuthTokens::with_access_token(
        e.config.oauth.clone(),
        e.http.clone(),
        refresh_token.clone(),
        auth.access_token,
    ));
    let gmail = Gmail::new(&e.config.gmail_base, e.http.clone(), Arc::clone(&tokens));
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

pub async fn add_account(e: &Arc<Engine>) -> anyhow::Result<AccountId> {
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

pub async fn sign_in_again(e: &Arc<Engine>, account: AccountId) -> anyhow::Result<()> {
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
/// A sign-in waits until this is done (see [`Engine::forgetting`]).
pub async fn forget(e: Arc<Engine>, account: AccountId, email: String) {
    let _forgetting = e.forgetting.read().await;
    e.stop_account(account);
    let store = Arc::clone(&e.store);
    let who = email.clone();
    let token = tokio::task::spawn_blocking(move || store.load_refresh_token(&who))
        .await
        .map_err(anyhow::Error::from)
        .and_then(|r| r.map_err(|err| anyhow::anyhow!("Keychain: {}", err.0)));
    match token {
        Ok(Some(token)) => ub_gmail::revoke(&e.config.oauth, &e.http, &token.0)
            .await
            .log_err("revoke token"),
        Ok(None) => {}
        Err(err) => log::warn!("read refresh token of a removed account: {err:#}"),
    }
    let store = Arc::clone(&e.store);
    let dir = super::attachments::account_dir(&e, account);
    let cleaned = tokio::task::spawn_blocking(move || {
        store
            .delete_refresh_token(&email)
            .log_err("delete refresh token");
        match std::fs::remove_dir_all(dir) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                log::warn!("delete attachments of account {account}: {err}");
            }
            _ => {}
        }
    })
    .await;
    cleaned.log_err("forget account");
}
