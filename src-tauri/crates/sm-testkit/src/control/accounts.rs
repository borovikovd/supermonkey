//! Gmail users, accounts, the request budget, and OAuth: sign-ins, the
//! token file, revocations, and grants.

use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::{Value, json};
use sm_core::Config;
use sm_fake_gmail::FakeUser;
use sm_gmail::{RefreshToken, TokenStore, TokenStoreError};

use crate::platform::SignInMode;
use crate::server::{Core, Ctl, Fail, Server, done, fast_quota};

#[derive(Deserialize)]
pub(crate) struct UserReq {
    email: String,
    #[serde(default)]
    name: Option<String>,
}

fn create_user(s: &Server, req: &UserReq) -> FakeUser {
    let user = s.fake.add_user(&req.email);
    if let Some(name) = &req.name {
        user.set_display_name(name);
    }
    s.users
        .lock()
        .insert(req.email.to_lowercase(), user.clone());
    user
}

/// Runs a token-file call off the async threads.
async fn token_file<T: Send + 'static>(
    s: &Server,
    call: impl FnOnce(&dyn TokenStore) -> Result<T, TokenStoreError> + Send + 'static,
) -> Result<T, Fail> {
    let tokens = Arc::clone(&s.tokens);
    Ok(tokio::task::spawn_blocking(move || call(&*tokens)).await??)
}

/// A Gmail user that exists in the fake but not in the app (for Add account).
pub(crate) async fn add_user(State(s): State<Arc<Server>>, Json(req): Json<UserReq>) -> Ctl {
    create_user(&s, &req);
    Ok(done())
}

/// A Gmail user already signed in to the app: its refresh token is in the
/// token file and the app has the account.
pub(crate) async fn add_account(State(s): State<Arc<Server>>, Json(req): Json<UserReq>) -> Ctl {
    let app = match &*s.core.lock().await {
        Core::Ready { app, .. } | Core::Running(app) => Arc::clone(app),
        Core::Down => return Err(Fail("the core is down; start it first".into())),
    };
    let user = create_user(&s, &req);
    let token = RefreshToken::new(user.refresh_token());
    let email = req.email.clone();
    token_file(&s, move |tokens| tokens.store(&email, token)).await?;
    let id = app.accounts().register(&req.email).await?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
pub(crate) struct QuotaReq {
    production: bool,
}

/// The request budget from the next start on: production's, or one that
/// never waits.
pub(crate) async fn quota(State(s): State<Arc<Server>>, Json(req): Json<QuotaReq>) -> Ctl {
    *s.quota.lock() = if req.production {
        sm_gmail::QuotaConfig::default()
    } else {
        fast_quota()
    };
    Ok(done())
}

#[derive(Deserialize)]
pub(crate) struct SignInReq {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    abandon: bool,
    #[serde(default)]
    deny: bool,
}

/// How the next sign-in in the browser goes: as a user, abandoned, or denied.
pub(crate) async fn next_sign_in(State(s): State<Arc<Server>>, Json(req): Json<SignInReq>) -> Ctl {
    if req.deny {
        s.fake.deny_next_sign_in();
    }
    s.platform.next_sign_in(match (req.abandon, req.email) {
        (true, _) => SignInMode::Abandon,
        (false, Some(email)) => SignInMode::As(email),
        (false, None) => SignInMode::FirstUser,
    });
    Ok(done())
}

/// The token file's entries, as stored on disk.
pub(crate) async fn tokens(State(s): State<Arc<Server>>) -> Ctl {
    let entries = token_file(&s, |tokens| tokens.entries()).await?;
    let items: Vec<Value> = entries
        .into_iter()
        .map(|(account, token)| json!({ "account": account, "token": token.secret() }))
        .collect();
    Ok(Json(json!(items)))
}

/// The token file's permission bits, such as "600", or null before the
/// first save.
pub(crate) async fn tokens_mode(State(s): State<Arc<Server>>) -> Ctl {
    let mode = tokio::fs::metadata(s.data_dir.join(Config::TOKEN_FILE))
        .await
        .ok()
        .map(|m| format!("{:o}", m.permissions().mode() & 0o777));
    Ok(Json(json!(mode)))
}

#[derive(Deserialize)]
pub(crate) struct EmailReq {
    email: String,
}

/// Deletes an account's token from the file, as if it was never saved.
pub(crate) async fn forget_token(State(s): State<Arc<Server>>, Json(req): Json<EmailReq>) -> Ctl {
    token_file(&s, move |tokens| tokens.delete(&req.email)).await?;
    Ok(done())
}

/// Tokens revoked at Google, oldest first.
pub(crate) async fn revocations(State(s): State<Arc<Server>>) -> Ctl {
    let list: Vec<Value> = s
        .fake
        .revocations()
        .into_iter()
        .map(|r| json!({ "token": r.token, "account": r.email }))
        .collect();
    Ok(Json(json!(list)))
}

#[derive(Deserialize)]
pub(crate) struct InvalidGrantReq {
    email: String,
    on: bool,
}

/// Google revokes (or restores) the account's refresh token.
pub(crate) async fn invalid_grant(
    State(s): State<Arc<Server>>,
    Json(req): Json<InvalidGrantReq>,
) -> Ctl {
    let user = s.user(&req.email)?;
    user.set_invalid_grant(req.on);
    if req.on {
        user.expire_access_tokens();
    }
    Ok(done())
}

#[derive(Deserialize)]
pub(crate) struct SettingsScopeReq {
    email: String,
    granted: bool,
}

/// Whether the user's grant includes Gmail filters, until the next sign-in
/// decides.
pub(crate) async fn settings_scope(
    State(s): State<Arc<Server>>,
    Json(req): Json<SettingsScopeReq>,
) -> Ctl {
    s.user(&req.email)?.set_settings_scope(req.granted);
    Ok(done())
}

/// The user's Gmail filters, as `{id, from, add, remove}`.
pub(crate) async fn filters(State(s): State<Arc<Server>>, Path(email): Path<String>) -> Ctl {
    let filters: Vec<Value> = s
        .user(&email)?
        .filters()
        .into_iter()
        .map(|f| json!({ "id": f.id, "from": f.from, "add": f.add, "remove": f.remove }))
        .collect();
    Ok(Json(json!(filters)))
}
