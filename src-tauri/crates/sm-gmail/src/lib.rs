mod client;
mod http;
mod ids;
mod oauth;
mod outcome;
mod quota;
mod retry;
mod token_store;
mod unsubscribe;
mod wire;

pub use client::{GMAIL_BASE_URL, Gmail, InvalidBaseUrl};
pub use http::{HttpClient, HttpClientError};
pub use ids::{FilterId, GmailAttachmentId, HistoryId, PageToken};
pub use oauth::{
    AccessToken, GMAIL_MODIFY_SCOPE, GMAIL_SETTINGS_SCOPE, Grant, OAuthConfig, RefreshToken,
    SignIn, SignInError, Tokens, revoke,
};
pub use outcome::{GmailError, Outcome};
pub use quota::{AccountScheduler, Permit, Priority, QuotaConfig, Scheduler, Units};
pub use retry::Backoff;
pub use token_store::{TokenFile, TokenStoreError};
pub use unsubscribe::one_click_unsubscribe;
pub use wire::{
    Draft, DraftPage, Filter, FilterAction, FilterCriteria, Format, HistoryLabelChange,
    HistoryMessage, HistoryPage, HistoryRecord, Label, LabelType, Message, Profile, SendAs, Thread,
    ThreadPage, ThreadQuery,
};
