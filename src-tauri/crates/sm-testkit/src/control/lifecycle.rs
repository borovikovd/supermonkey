use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::json;
use sm_bridge::Session;
use sm_core::clock::ManualClock;
use tokio::runtime::Handle;
use tokio::sync::Notify;

use crate::host::TestHost;
use crate::server::{Core, Ctl, Fail, SETTLE_LIMIT, Server, done};

pub(crate) async fn start(State(s): State<Arc<Server>>) -> Ctl {
    let quota = *s.quota.lock();
    let mut core = s.core.lock().await;
    let app = match std::mem::replace(&mut *core, Core::Down) {
        Core::Ready { app, quota: opened } if opened == quota => app,
        Core::Ready { app, .. } => {
            app.lifecycle().quit().await;
            drop(app);
            s.open_core(quota)?
        }
        Core::Down => s.open_core(quota)?,
        running @ Core::Running { .. } => {
            *core = running;
            return Err(Fail("already running".into()));
        }
    };
    let exited = Arc::new(Notify::new());
    let host = Box::new(TestHost {
        pages: s.bridge.pages(),
        platform: Arc::clone(&s.platform),
        exited: Arc::clone(&exited),
    });
    let clock = Arc::<ManualClock>::clone(&s.clock);
    let session = Session::start(app, host, clock, &Handle::current(), &s.activity);
    s.bridge.set_session(Some(Arc::clone(&session)));
    *core = Core::Running { session, exited };
    Ok(done())
}

#[derive(Deserialize)]
pub(crate) struct StopReq {
    graceful: bool,
}

pub(crate) async fn stop(State(s): State<Arc<Server>>, Json(req): Json<StopReq>) -> Ctl {
    let mut core = s.core.lock().await;
    let (session, exited) = match std::mem::replace(&mut *core, Core::Down) {
        Core::Running { session, exited } => (session, exited),
        other @ (Core::Ready { .. } | Core::Down) => {
            *core = other;
            return Err(Fail("the core is not running".into()));
        }
    };
    if !req.graceful {
        s.bridge.set_session(None);
        session.terminated().await;
        return Ok(done());
    }
    let quit = quit(&session, &exited).await;
    s.bridge.set_session(None);
    quit.map(|()| done())
}

async fn quit(session: &Arc<Session>, exited: &Notify) -> Result<(), Fail> {
    session.request_quit();
    if tokio::time::timeout(SETTLE_LIMIT, exited.notified())
        .await
        .is_ok()
    {
        return Ok(());
    }
    session.terminated().await;
    Err(Fail(
        "the app did not quit: the page never answered, and app time never reached the quit deadline"
            .into(),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdleReq {
    #[serde(default)]
    timeout_ms: Option<u64>,
}

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

pub(crate) async fn poll(State(s): State<Arc<Server>>) -> Ctl {
    s.poll().await;
    Ok(done())
}

pub(crate) async fn clock_now(State(s): State<Arc<Server>>) -> Ctl {
    Ok(Json(json!({ "now": s.now() })))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClockReq {
    #[serde(default)]
    advance_ms: Option<u64>,
    #[serde(default)]
    stepwise: bool,
    #[serde(default)]
    jump_ms: Option<u64>,
}

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

pub(crate) async fn focus(State(s): State<Arc<Server>>, Json(req): Json<FocusReq>) -> Ctl {
    let Some(session) = s.session().await else {
        return Err(Fail("the core is not running".into()));
    };
    let clicked = session.focus(req.focused);
    if !req.focused {
        return Ok(done());
    }
    let Some(n) = clicked else {
        return Ok(Json(json!({ "openThread": null })));
    };
    let thread = match n.thread {
        Some(t) => s.gmail_thread(t).await,
        None => None,
    };
    Ok(Json(json!({
        "openThread": { "account": n.account_email, "thread": thread },
    })))
}

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
        "windowsShown": recorded.windows_shown,
        "events": recorded.events,
        "exits": recorded.exits,
    })))
}

pub(crate) async fn clear_recorded(State(s): State<Arc<Server>>) -> Ctl {
    s.platform.clear();
    Ok(done())
}
