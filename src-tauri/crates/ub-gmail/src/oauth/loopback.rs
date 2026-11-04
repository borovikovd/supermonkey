//! The loopback port the browser returns to after consent.
//!
//! Connections are read at the same time, so a browser's speculative
//! connection that never sends a request cannot hold up the real redirect.

use std::net::Ipv4Addr;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

/// The most bytes of request head read before giving up on a connection.
const MAX_HEAD: usize = 16 * 1024;
/// How long one connection may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait after a failed `accept` before the next.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Ubermensch</title>\
<body style=\"font:16px -apple-system,sans-serif;padding:4em;text-align:center\">\
<p>You are signed in. You can close this tab.</p>";
const FAILED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Ubermensch</title>\
<body style=\"font:16px -apple-system,sans-serif;padding:4em;text-align:center\">\
<p>Sign-in did not complete. You can close this tab and try again.</p>";

/// A listener on `127.0.0.1` at a free port.
pub(super) struct Loopback {
    listener: TcpListener,
    port: u16,
}

/// What the browser brought back to the redirect URI.
pub(super) enum Redirect {
    /// The consent page gave a code.
    Code(String),
    /// The consent page gave an error code, such as `access_denied`.
    Error(String),
    /// The `state` did not match this sign-in.
    StateMismatch,
}

impl Loopback {
    /// Binds a free port.
    pub(super) async fn bind() -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    /// The redirect URI to give Google.
    pub(super) fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// Serves the port until a request carrying `code` or `error` arrives,
    /// answers it with a page for the user, and returns what it carried.
    /// Other requests, such as `/favicon.ico`, get a 404.
    pub(super) async fn wait(&self, expected_state: &str) -> Redirect {
        let mut reading = JoinSet::new();
        loop {
            tokio::select! {
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        reading.spawn(read_target(stream));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "sign-in loopback accept failed");
                        tokio::time::sleep(ACCEPT_RETRY).await;
                    }
                },
                Some(read) = reading.join_next(), if !reading.is_empty() => {
                    // A connection that failed or sent no request head is dropped.
                    let Ok(Some((mut stream, target))) = read else {
                        continue;
                    };
                    let Some(redirect) = parse(&target, expected_state) else {
                        respond(&mut stream, "404 Not Found", "").await;
                        continue;
                    };
                    let (status, page) = match &redirect {
                        Redirect::Code(_) => ("200 OK", DONE_PAGE),
                        Redirect::Error(_) => ("200 OK", FAILED_PAGE),
                        Redirect::StateMismatch => ("400 Bad Request", FAILED_PAGE),
                    };
                    respond(&mut stream, status, page).await;
                    return redirect;
                }
            }
        }
    }
}

/// Reads the request target (`/path?query`) of a connection's first request.
async fn read_target(mut stream: TcpStream) -> Option<(TcpStream, String)> {
    let read = async {
        let mut head = Vec::new();
        let mut chunk = [0_u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 || head.len() > MAX_HEAD {
                return None;
            }
            head.extend_from_slice(chunk.get(..n)?);
        }
        // The request line is `METHOD target HTTP/1.1`.
        let head = String::from_utf8_lossy(&head);
        head.lines()
            .next()?
            .split_whitespace()
            .nth(1)
            .map(str::to_owned)
    };
    let target = tokio::time::timeout(HEAD_TIMEOUT, read).await.ok()??;
    Some((stream, target))
}

/// What a request target carries, or `None` if it is not the redirect.
fn parse(target: &str, expected_state: &str) -> Option<Redirect> {
    let url = url::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    let answer = match (param("code"), param("error")) {
        (_, Some(error)) => Redirect::Error(error),
        (Some(code), None) => Redirect::Code(code),
        (None, None) => return None,
    };
    if param("state").as_deref() == Some(expected_state) {
        Some(answer)
    } else {
        Some(Redirect::StateMismatch)
    }
}

/// Answers and closes. The browser may already be gone; that only matters to
/// the user's tab, so a failure is logged and dropped.
async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let written = async {
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    };
    if let Err(err) = written.await {
        tracing::debug!(error = %err, "sign-in loopback answer not delivered");
    }
}
