use std::io::Write as _;
use std::net::SocketAddr;

use supermonkey_lib::testkit::server::{E2eServer, resolve_dist_dir};
use tokio::net::TcpListener;

#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "e2e-server announces READY url on stdout and startup errors on stderr"
)]
#[tokio::main]
async fn main() {
    supermonkey_lib::shell::logging::init();

    let (port, now_ms) = parse_args();
    let dist_dir = resolve_dist_dir();
    let server = match E2eServer::new(now_ms, dist_dir) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("failed to initialize e2e-server: {err}");
            return;
        }
    };

    let bind_addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match TcpListener::bind(bind_addr).await {
        Ok(l) => l,
        Err(err) => {
            eprintln!("failed to bind {bind_addr}: {err}");
            return;
        }
    };
    let actual = match listener.local_addr() {
        Ok(a) => a,
        Err(err) => {
            eprintln!("failed to read local_addr: {err}");
            return;
        }
    };

    let app = server.router();
    println!("READY http://{actual}");
    if let Err(err) = std::io::stdout().flush() {
        eprintln!("failed to flush READY line: {err}");
    }

    let runner = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());
    if let Err(err) = runner.await {
        eprintln!("e2e-server exited with error: {err}");
    }
    server.shutdown().await;
    drop(server);
}

fn parse_args() -> (u16, i64) {
    let mut port: u16 = 0;
    let mut now_ms: i64 = chrono::Utc::now().timestamp_millis();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                if let Some(v) = args.next()
                    && let Ok(p) = v.parse()
                {
                    port = p;
                }
            }
            "--now" => {
                if let Some(v) = args.next()
                    && let Ok(n) = v.parse()
                {
                    now_ms = n;
                }
            }
            _ => {}
        }
    }
    (port, now_ms)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            () = async {
                if let Some(ref mut s) = sigterm {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
