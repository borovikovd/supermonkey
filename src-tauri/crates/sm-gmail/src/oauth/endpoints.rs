//! Google's token and revoke endpoints.

use reqwest::StatusCode;
use serde::Deserialize;

use super::{AccessToken, OAuthConfig, RefreshToken};
use crate::http::{self, HttpClient, ReadResponse};
use crate::outcome::{GmailError, Outcome};

/// A token endpoint's answer.
#[derive(Deserialize)]
pub(super) struct TokenAnswer {
    pub(super) access_token: String,
    pub(super) expires_in: Option<u64>,
    pub(super) refresh_token: Option<String>,
}

/// An OAuth error body: `{"error": "...", "error_description": "..."}`, not a
/// Google API error.
#[derive(Deserialize)]
struct OAuthErrorBody {
    error: String,
    error_description: Option<String>,
}

/// Trades the refresh token for a new access token. Google rejecting the
/// grant is [`Outcome::InvalidGrant`].
pub(super) async fn refresh(
    config: &OAuthConfig,
    http: &HttpClient,
    refresh_token: &RefreshToken,
) -> Result<AccessToken, GmailError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.secret()),
        ("client_id", config.client_id.as_str()),
        ("client_secret", config.client_secret.as_str()),
    ];
    let answer = token_request(config, http, &form).await?;
    tracing::debug!("access token refreshed");
    Ok(AccessToken::issued(answer.access_token, answer.expires_in))
}

/// Posts a token request and reads the answer.
pub(super) async fn token_request(
    config: &OAuthConfig,
    http: &HttpClient,
    form: &[(&str, &str)],
) -> Result<TokenAnswer, GmailError> {
    let request = http.inner().post(&config.token_url).form(form);
    let response = ReadResponse::read(http::send(request).await?).await?;
    let status = response.status;
    if status.is_success() {
        return response.json("token answer");
    }
    if let Ok(err) = serde_json::from_slice::<OAuthErrorBody>(&response.body) {
        if err.error == "invalid_grant" {
            return Err(Outcome::InvalidGrant.into());
        }
        if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
            let description = err.error_description.unwrap_or_default();
            return Err(Outcome::Rejected {
                status: status.as_u16(),
                message: format!("{}: {description}", err.error),
                reason: Some(err.error),
            }
            .into());
        }
    }
    Err(response.to_error())
}

/// Revokes the grant behind `token` with Google: every token of the grant
/// stops working. A token Google already considers invalid counts as revoked.
pub async fn revoke(
    config: &OAuthConfig,
    http: &HttpClient,
    token: &RefreshToken,
) -> Result<(), GmailError> {
    let request = http
        .inner()
        .post(&config.revoke_url)
        .form(&[("token", token.secret())]);
    let response = http::send(request).await?;
    if response.status().is_success() {
        tracing::debug!("grant revoked");
        return Ok(());
    }
    let response = ReadResponse::read(response).await?;
    match serde_json::from_slice::<OAuthErrorBody>(&response.body) {
        Ok(err) if err.error == "invalid_token" => Ok(()),
        _ => Err(response.to_error()),
    }
}
