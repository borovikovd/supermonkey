mod control;
mod demo;
pub mod fixture;
mod host;
mod platform;
mod server;

pub use demo::Demo;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use sm_bridge::WebBridge;
use sm_core::Config;
use sm_core::activity::{Activity, Tasks};
use sm_core::clock::ManualClock;
use sm_domain::Millis;
use sm_fake_gmail::FakeGmail;
use sm_gmail::{OAuthConfig, TokenFile};

use crate::platform::RecordingPlatform;
use crate::server::{Core, Server, fast_quota};

pub async fn run(port: u16, dist: PathBuf, now: Millis) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut server = new_server(dir.path().to_path_buf(), dist, now);
    let quota = fast_quota();
    let app = server.open_core(quota)?;
    *server.core.get_mut() = Core::Ready { app, quota };
    let routes = server
        .bridge
        .router()
        .merge(control::router(Arc::new(server)));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    #[expect(
        clippy::print_stdout,
        reason = "the test runner reads the address from stdout"
    )]
    {
        println!("READY http://{addr}");
    }
    tokio::select! {
        result = axum::serve(listener, routes).into_future() => result?,
        signal = told_to_stop() => signal?,
    }
    dir.close()?;
    Ok(())
}

fn new_server(data_dir: PathBuf, dist: PathBuf, now: Millis) -> Server {
    let activity = Activity::new();
    let clock = Arc::new(ManualClock::new(Arc::clone(&activity), now));
    let fake = FakeGmail::start();
    let fake_clock = Arc::clone(&clock);
    fake.set_clock(move || u64::try_from(sm_core::clock::Clock::now(&*fake_clock).0).unwrap_or(0));
    let oauth = OAuthConfig {
        client_id: "e2e".into(),
        client_secret: "e2e".into(),
        auth_url: fake.auth_url(),
        token_url: fake.token_url(),
        revoke_url: fake.revoke_url(),
    };
    let browser = Tasks::new(tokio::runtime::Handle::current(), Arc::clone(&activity));
    Server {
        fake,
        users: Mutex::new(HashMap::new()),
        tokens: Arc::new(TokenFile::new(data_dir.join(Config::TOKEN_FILE))),
        platform: Arc::new(RecordingPlatform::new(browser)),
        bridge: WebBridge::new(None, Arc::clone(&activity)),
        clock,
        activity,
        #[expect(
            clippy::disallowed_types,
            reason = "held while the core starts or quits, which awaits"
        )]
        core: tokio::sync::Mutex::new(Core::Down),
        quota: Mutex::new(fast_quota()),
        oauth,
        data_dir,
        dist,
        hits: Mutex::new(Vec::new()),
    }
}

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
