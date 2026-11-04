//! The control API's routes, by path.

use std::sync::Arc;

use axum::Router;
use axum::routing::{any, get, post};

use super::{accounts, faults, gmail, lifecycle, web};
use crate::server::Server;

/// The control routes, the image host and sink, and the built UI for every
/// other path.
pub(crate) fn router(server: Arc<Server>) -> Router {
    Router::new()
        .route("/__test/account", post(accounts::add_account))
        .route("/__test/user", post(accounts::add_user))
        .route("/__test/quota", post(accounts::quota))
        .route("/__test/start", post(lifecycle::start))
        .route("/__test/stop", post(lifecycle::stop))
        .route("/__test/idle", get(lifecycle::idle))
        .route("/__test/deliver", post(gmail::deliver))
        .route("/__test/deliver-batch", post(gmail::deliver_batch))
        .route("/__test/gmail/{email}", get(gmail::gmail_state))
        .route("/__test/gmail/{email}/modify", post(gmail::modify))
        .route("/__test/gmail/{email}/delete", post(gmail::delete_message))
        .route(
            "/__test/gmail/{email}/delete-thread",
            post(gmail::delete_thread),
        )
        .route("/__test/gmail/{email}/label", post(gmail::label))
        .route("/__test/gmail/{email}/send-as", post(gmail::send_as))
        .route("/__test/gmail/{email}/drafts", post(gmail::create_draft))
        .route(
            "/__test/gmail/{email}/drafts/{id}",
            post(gmail::update_draft),
        )
        .route(
            "/__test/gmail/{email}/drafts/{id}/delete",
            post(gmail::delete_draft),
        )
        .route(
            "/__test/gmail/{email}/drafts/{id}/send",
            post(gmail::send_draft),
        )
        .route(
            "/__test/gmail/{email}/expire-tokens",
            post(gmail::expire_tokens),
        )
        .route(
            "/__test/gmail/{email}/expire-history",
            post(gmail::expire_history),
        )
        .route("/__test/gmail/{email}/filters", get(accounts::filters))
        .route("/__test/invalid-grant", post(accounts::invalid_grant))
        .route("/__test/settings-scope", post(accounts::settings_scope))
        .route("/__test/sign-in", post(accounts::next_sign_in))
        .route("/__test/tokens", get(accounts::tokens))
        .route("/__test/tokens/mode", get(accounts::tokens_mode))
        .route("/__test/tokens/forget", post(accounts::forget_token))
        .route("/__test/revocations", get(accounts::revocations))
        .route("/__test/fault", post(faults::fault))
        .route("/__test/release", post(faults::release))
        .route("/__test/clear-faults", post(faults::clear_faults))
        .route("/__test/offline", post(faults::offline))
        .route("/__test/requests", get(faults::requests))
        .route("/__test/poll", post(lifecycle::poll))
        .route(
            "/__test/clock",
            get(lifecycle::clock_now).post(lifecycle::clock),
        )
        .route("/__test/focus", post(lifecycle::focus))
        .route("/__test/platform", get(lifecycle::platform))
        .route("/__test/clear-recorded", post(lifecycle::clear_recorded))
        .route("/__test/hits", get(web::hits))
        .route("/__test/db-contains", get(web::db_contains))
        .route("/img/{name}", get(web::remote_image))
        .route("/sink/{*rest}", any(web::sink))
        .fallback(get(web::static_file))
        .with_state(server)
}
