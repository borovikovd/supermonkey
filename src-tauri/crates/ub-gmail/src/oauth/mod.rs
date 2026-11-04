//! Google OAuth for an installed app (SPEC Accounts): sign-in through the
//! system browser with a loopback redirect and PKCE, access tokens refreshed
//! shortly before they expire, and revoke.
//!
//! The `oauth2` crate makes the consent URL, the PKCE pair, and the state.
//! The token calls are plain form posts through [`HttpClient`](crate::HttpClient),
//! so their failures classify like Gmail calls.

mod config;
mod endpoints;
mod loopback;
mod secrets;
mod sign_in;
mod tokens;

pub use config::{GMAIL_MODIFY_SCOPE, GMAIL_SETTINGS_SCOPE, OAuthConfig};
pub use endpoints::revoke;
pub use secrets::{AccessToken, Grant, RefreshToken};
pub use sign_in::{SignIn, SignInError};
pub use tokens::Tokens;
