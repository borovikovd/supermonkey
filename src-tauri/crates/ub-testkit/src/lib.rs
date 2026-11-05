//! Test and demo support (ARCHITECTURE.md, sections 3 and 13): the E2E
//! server, its `/__test` control API, and the recording platform; demo
//! mode ([`Demo`]); and the perf fixture ([`fixture`]). Linked only into
//! debug builds with the `fake` feature.
//!
//! [`run`] serves, on one origin:
//! - the debug web bridge (`/invoke`, `/events`) over the real core;
//! - the `/__test` control API that `e2e/control.ts` calls;
//! - the remote image host (`/img`) and a sink for hostile links (`/sink`);
//! - the built UI for every other path.
//!
//! The core runs against the fake Gmail, in a temporary data folder, on a
//! manual clock. A test seeds accounts and mail through `/__test`, calls
//! `POST /__test/start` to launch the core, and only then loads the page.
//! `POST /__test/stop` quits (or crashes) the core in-process; the next
//! `start` relaunches it on the same database, fake Gmail, and clock. Time
//! moves only through `/__test/clock`, and `/__test/idle` waits until the
//! core has nothing left to do.

mod control;
mod demo;
pub mod fixture;
mod platform;
mod server;

pub use demo::Demo;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use ub_bridge::WebBridge;
use ub_core::Config;
use ub_core::activity::{Activity, Tasks};
use ub_core::clock::ManualClock;
use ub_domain::Millis;
use ub_fake_gmail::FakeGmail;
use ub_gmail::{OAuthConfig, TokenFile};

use crate::platform::RecordingPlatform;
use crate::server::{Core, Server, fast_quota};

/// Runs the E2E server on 127.0.0.1:`port` (0 picks a free one) until the
/// process is told to stop (`SIGTERM` or `SIGINT`), serving the UI from
/// `dist`. App time starts at `now`. Prints `READY <url>` on stdout once
/// listening. Its temporary data folder is deleted on the way out: a test
/// run starts hundreds of servers, and each folder holds a database.
pub async fn run(port: u16, dist: PathBuf, now: Millis) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let data_dir = dir.path().to_path_buf();
    let activity = Activity::new();
    let clock = Arc::new(ManualClock::new(Arc::clone(&activity), now));
    let fake = FakeGmail::start();
    let fake_clock = Arc::clone(&clock);
    fake.set_clock(move || u64::try_from(ub_core::clock::Clock::now(&*fake_clock).0).unwrap_or(0));
    let oauth = OAuthConfig {
        client_id: "e2e".into(),
        client_secret: "e2e".into(),
        auth_url: fake.auth_url(),
        token_url: fake.token_url(),
        revoke_url: fake.revoke_url(),
    };
    let browser = Tasks::new(tokio::runtime::Handle::current(), Arc::clone(&activity));
    let bridge = WebBridge::new(None, Arc::clone(&activity));
    let mut server = Server {
        fake,
        users: Mutex::new(HashMap::new()),
        tokens: Arc::new(TokenFile::new(data_dir.join(Config::TOKEN_FILE))),
        platform: Arc::new(RecordingPlatform::new(browser)),
        bridge: bridge.clone(),
        clock,
        activity,
        core: tokio::sync::Mutex::new(Core::Down),
        quota: Mutex::new(fast_quota()),
        oauth,
        data_dir,
        dist,
        hits: Mutex::new(Vec::new()),
    };
    let quota = fast_quota();
    let app = server.open_core(quota)?;
    *server.core.get_mut() = Core::Ready { app, quota };
    let routes = bridge.router().merge(control::router(Arc::new(server)));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    #[expect(
        clippy::print_stdout,
        reason = "the test runner reads the address from stdout"
    )]
    {
        println!("READY http://{addr}");
    }
    // Not a graceful shutdown: the page's event stream never ends on its own.
    tokio::select! {
        result = axum::serve(listener, routes).into_future() => result?,
        signal = told_to_stop() => signal?,
    }
    dir.close()?;
    Ok(())
}

/// Waits for `SIGTERM` (how the test runner stops a server) or `SIGINT`.
async fn told_to_stop() -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
    Ok(())
}
