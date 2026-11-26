mod client;
mod http;
mod oauth;
mod quota;
mod tokens;
mod wire;

pub use client::{GMAIL_BASE_URL, Gmail, InvalidBaseUrl, one_click_unsubscribe};
pub use http::{Backoff, GmailError, HttpClient, HttpClientError, Outcome};
pub use oauth::{
    AccessToken, AuthCallback, GMAIL_MODIFY_SCOPE, GMAIL_SETTINGS_SCOPE, Grant, OAuthConfig,
    RefreshToken, SignIn, SignInError, Tokens, revoke,
};
pub use quota::{AccountScheduler, Permit, Priority, QuotaConfig, Scheduler, Units};
pub use tokens::{TokenFile, TokenStoreError};
pub use wire::{
    Draft, DraftPage, Filter, FilterAction, FilterCriteria, Format, HistoryMessageRef, HistoryPage,
    HistoryRecord, Label, LabelType, Message, Profile, SendAs, Thread, ThreadPage, ThreadQuery,
    ThreadRef,
};
