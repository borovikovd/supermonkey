//! Google sign-in (SPEC Accounts): system browser, loopback redirect, PKCE.

use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;
use ub_gmail::{Gmail, OAuthTokens, Priority, cost};

use super::EngineRef;
use crate::model::*;

async fn sign_in(app: &AppHandle) -> anyhow::Result<(String, Arc<OAuthTokens>, String)> {
    let e = app.state::<EngineRef>().inner().clone();
    if e.config.oauth.client_id.is_empty() {
        anyhow::bail!(
            "This build has no Google OAuth client. Put google-oauth-client.json in the project and rebuild."
        );
    }
    let opener = app.clone();
    let auth = ub_gmail::authorize(&e.config.oauth, &e.http, move |url| {
        let _ = opener.opener().open_url(url, None::<&str>);
    })
    .await
    .map_err(|err| anyhow::anyhow!("Sign-in failed: {:?}", err.outcome))?;
    let refresh = auth.refresh_token.0.clone();
    let tokens = Arc::new(OAuthTokens::with_access_token(
        e.config.oauth.clone(),
        e.http.clone(),
        refresh.clone(),
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
    Ok((profile.email_address, tokens, refresh))
}

pub async fn add_account(app: &AppHandle) -> anyhow::Result<AccountId> {
    let e = app.state::<EngineRef>().inner().clone();
    let (email, tokens, refresh) = sign_in(app).await?;
    e.store
        .store_refresh_token(&email, &refresh)
        .map_err(|err| anyhow::anyhow!("Keychain: {}", err.0))?;
    let em = email.clone();
    let id = e
        .writer
        .call(move |ctx| crate::accounts::create(ctx, &em))
        .await?;
    e.start_account(id, email, tokens);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_focus();
    }
    Ok(id)
}

pub async fn sign_in_again(app: &AppHandle, account: AccountId) -> anyhow::Result<()> {
    let e = app.state::<EngineRef>().inner().clone();
    let expected: String = e.reads.with(|c| {
        Ok(
            c.query_row("SELECT email FROM accounts WHERE id = ?1", [account], |r| {
                r.get(0)
            })?,
        )
    })?;
    let (email, tokens, refresh) = sign_in(app).await?;
    if !email.eq_ignore_ascii_case(&expected) {
        anyhow::bail!("You signed in as {email}. Sign in as {expected} to reconnect this account.");
    }
    e.store
        .store_refresh_token(&email, &refresh)
        .map_err(|err| anyhow::anyhow!("Keychain: {}", err.0))?;
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
    e.start_account(account, email, tokens);
    Ok(())
}

/// After Remove account: stop sync, revoke and delete the token, delete files.
pub async fn forget(e: EngineRef, account: AccountId, email: String) {
    e.stop_account(account);
    if let Ok(Some(token)) = e.store.load_refresh_token(&email) {
        let _ = ub_gmail::revoke(&e.config.oauth, &e.http, &token).await;
    }
    let _ = e.store.delete_refresh_token(&email);
    let _ = std::fs::remove_dir_all(super::attachments::account_dir(&e, account));
}
