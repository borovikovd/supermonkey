//! Starting and stopping the core, `settle()`, app time, focus, and what
//! the platform recorded.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::json;
use ub_ipc::AppEvent;

use crate::server::{Core, Ctl, Fail, SETTLE_LIMIT, Server, done};

/// Launches the core: the seeded accounts start syncing. After a stop, the
/// core relaunches on the same database.
pub(crate) async fn start(State(s): State<Arc<Server>>) -> Ctl {
    let quota = *s.quota.lock();
    let mut core = s.core.lock().await;
    let app = match std::mem::replace(&mut *core, Core::Down) {
        Core::Ready { app, quota: opened } if opened == quota => app,
        // The budget changed since the core opened: open it again with the new one.
        Core::Ready { app, .. } => {
            app.lifecycle().quit().await;
            drop(app);
            s.open_core(quota)?
        }
        Core::Down => s.open_core(quota)?,
        running @ Core::Running(_) => {
            *core = running;
            return Err(Fail("already running".into()));
        }
    };
    app.lifecycle().launch();
    s.bridge.set_app(Some(Arc::clone(&app)));
    *core = Core::Running(app);
    Ok(done())
}

#[derive(Deserialize)]
pub(crate) struct StopReq {
    /// Quit as Cmd+Q does (the page closes an open composer first); otherwise crash.
    graceful: bool,
}

/// Quits or crashes the core. The fake Gmail and the clock live on; `start`
/// relaunches.
pub(crate) async fn stop(State(s): State<Arc<Server>>, Json(req): Json<StopReq>) -> Ctl {
    let mut core = s.core.lock().await;
    let app = match std::mem::replace(&mut *core, Core::Down) {
        Core::Running(app) => app,
        other => {
            *core = other;
            return Err(Fail("the core is not running".into()));
        }
    };
    if req.graceful {
        let quit = s.bridge.quit_called();
        tokio::pin!(quit);
        quit.as_mut().enable();
        if s.bridge.emit(&AppEvent::QuitRequested) > 0
            && tokio::time::timeout(SETTLE_LIMIT, quit).await.is_err()
        {
            tracing::warn!("the page did not call quit_now");
        }
        // The composer close the page sent reaches the database before the core stops.
        s.settle().await?;
    }
    s.bridge.set_app(None);
    app.lifecycle().quit().await;
    Ok(done())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdleReq {
    /// Default 10 s.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// Waits until the core is idle. Returns where the page's channel should be
/// and the sign-ins the core is running, for the page's half of `settle()`.
pub(crate) async fn idle(State(s): State<Arc<Server>>, Query(q): Query<IdleReq>) -> Ctl {
    s.settle_within(q.timeout_ms.map_or(SETTLE_LIMIT, Duration::from_millis))
        .await?;
    let (rev, ack) = s.bridge.last_sent();
    Ok(Json(json!({
        "rev": rev,
        "ack": ack,
        "signIns": s.bridge.sign_ins(),
        "held": s.fake.held(),
    })))
}

/// A history poll runs now for every account.
pub(crate) async fn poll(State(s): State<Arc<Server>>) -> Ctl {
    s.poll().await;
    Ok(done())
}

/// App time.
pub(crate) async fn clock_now(State(s): State<Arc<Server>>) -> Ctl {
    Ok(Json(json!({ "now": s.now() })))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClockReq {
    /// Moves app time forward, firing each timer due by then once.
    #[serde(default)]
    advance_ms: Option<u64>,
    /// With `advance_ms`: fires timers one by one at their own times, letting
    /// the core settle between, so a back-off runs every retry it would.
    #[serde(default)]
    stepwise: bool,
    /// Moves the wall clock only: the Mac slept, and no timer noticed.
    #[serde(default)]
    jump_ms: Option<u64>,
}

/// Moves app time and returns it once the core has settled.
pub(crate) async fn clock(State(s): State<Arc<Server>>, Json(req): Json<ClockReq>) -> Ctl {
    if let Some(ms) = req.jump_ms {
        s.clock.jump(ms);
    }
    if let Some(mut left) = req.advance_ms {
        if req.stepwise {
            while let Some(wait) = s.clock.next_due_within(left) {
                s.clock.advance(wait);
                left = left.saturating_sub(wait);
                s.settle().await?;
            }
        }
        s.clock.advance(left);
        s.settle().await?;
    }
    Ok(Json(json!({ "now": s.now() })))
}

#[derive(Deserialize)]
pub(crate) struct FocusReq {
    focused: bool,
}

/// The app window gains or loses focus. Focus soon after a background
/// notification counts as a click on it: the page gets `notification-open`,
/// as the app's webview does.
pub(crate) async fn focus(State(s): State<Arc<Server>>, Json(req): Json<FocusReq>) -> Ctl {
    let Some(app) = s.running().await else {
        return Err(Fail("the core is not running".into()));
    };
    let clicked = app.lifecycle().focus(req.focused);
    if !req.focused {
        return Ok(done());
    }
    let Some(n) = clicked else {
        return Ok(Json(json!({ "openThread": null })));
    };
    s.bridge.emit(&AppEvent::NotificationOpen {
        account: n.account,
        thread: n.thread,
    });
    let thread = match n.thread {
        Some(t) => s.gmail_thread(t).await,
        None => None,
    };
    Ok(Json(json!({
        "openThread": { "account": n.account_email, "thread": thread },
    })))
}

/// What the core asked of the OS: notifications (with Gmail thread ids),
/// the badge, opened links and files, and focus requests.
pub(crate) async fn platform(State(s): State<Arc<Server>>) -> Ctl {
    let recorded = s.platform.recorded();
    let mut notifications = Vec::with_capacity(recorded.notifications.len());
    for n in recorded.notifications {
        let thread = match n.thread {
            Some(t) => s.gmail_thread(t).await,
            None => None,
        };
        notifications.push(json!({
            "title": n.title,
            "body": n.text(),
            "account": n.account_email,
            "thread": thread,
        }));
    }
    Ok(Json(json!({
        "notifications": notifications,
        "badge": recorded.badge,
        "openedUrls": recorded.opened_urls,
        "openedFiles": recorded.opened_files,
        "focusRequests": recorded.focus_requests,
    })))
}

/// Forgets what the platform recorded so far (the badge stays as it is).
pub(crate) async fn clear_recorded(State(s): State<Arc<Server>>) -> Ctl {
    s.platform.clear();
    Ok(done())
}
