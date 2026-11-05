//! Gmail faults and holds, the network, and the request log.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use ub_fake_gmail::{Answer, Fault, LoggedRequest, Matcher, Release};

use crate::server::{Ctl, Fail, SETTLE_LIMIT, Server, done};

/// Picks requests: by Gmail method (`api`, e.g. `messages.batchModify`),
/// HTTP method and path glob, account, and query parameters.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MatchReq {
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    query: HashMap<String, String>,
    #[serde(default)]
    nth: Option<usize>,
    #[serde(default)]
    times: Option<usize>,
}

impl MatchReq {
    fn matcher(&self) -> Matcher {
        let mut m = Matcher::new(
            self.method.as_deref().unwrap_or("*"),
            self.path.as_deref().unwrap_or("*"),
        );
        if let Some(api) = &self.api {
            m = m.for_api(api);
        }
        if let Some(account) = &self.account {
            m = m.account(account);
        }
        for (k, v) in &self.query {
            m = m.query(k, v);
        }
        if let Some(n) = self.nth {
            m = m.nth(n);
        }
        if let Some(t) = self.times {
            m = m.times(t);
        }
        m
    }
}

/// What a matched request suffers.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FaultSpec {
    /// `drop`, `noResponse`, `status`, `statusAfterApply`, `delay`, `hold`,
    /// `holdAnswer`, or `omitThreads`.
    kind: String,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    retry_after_secs: Option<u64>,
    #[serde(default)]
    delay_ms: Option<u64>,
    #[serde(default)]
    threads: Vec<String>,
}

impl FaultSpec {
    fn fault(self) -> Result<Fault, Fail> {
        let status = |s: Option<u16>| s.ok_or_else(|| Fail("a status fault needs `status`".into()));
        let reason = |r: Option<String>, status: u16| {
            r.unwrap_or_else(|| {
                match status {
                    429 => "rateLimitExceeded",
                    500..=599 => "backendError",
                    _ => "failedPrecondition",
                }
                .into()
            })
        };
        Ok(match self.kind.as_str() {
            "drop" => Fault::Drop,
            "noResponse" => Fault::NoResponse,
            "status" => {
                let code = status(self.status)?;
                Fault::Status {
                    status: code,
                    reason: reason(self.reason, code),
                    retry_after_secs: self.retry_after_secs,
                }
            }
            "statusAfterApply" => {
                let code = status(self.status)?;
                Fault::StatusAfterApply {
                    status: code,
                    reason: reason(self.reason, code),
                }
            }
            "delay" => Fault::Delay(Duration::from_millis(self.delay_ms.unwrap_or(1000))),
            "hold" => Fault::Hold,
            "holdAnswer" => Fault::HoldAnswer,
            "omitThreads" => Fault::OmitThreads(self.threads),
            other => return Err(Fail(format!("unknown fault kind {other}"))),
        })
    }
}

#[derive(Deserialize)]
pub(crate) struct FaultReq {
    #[serde(flatten)]
    on: MatchReq,
    #[serde(flatten)]
    fault: FaultSpec,
}

/// Makes matching Gmail requests fail or wait (SPEC Gmail errors). Returns
/// the fault's id, which `release` takes for a hold.
pub(crate) async fn fault(State(s): State<Arc<Server>>, Json(req): Json<FaultReq>) -> Ctl {
    let id = s.fake.inject(req.on.matcher(), req.fault.fault()?);
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReleaseReq {
    /// One hold, by the id `fault` returned.
    #[serde(default)]
    id: Option<u64>,
    /// Holds on this Gmail method. With neither, every hold.
    #[serde(default)]
    api: Option<String>,
    /// What released requests suffer instead of being handled normally.
    #[serde(default)]
    outcome: Option<FaultSpec>,
    /// Waits until the holds have parked at least this many requests first,
    /// so a release never overtakes the request it is meant for.
    #[serde(default)]
    min: usize,
}

/// Ends holds and lets the requests they parked go on.
pub(crate) async fn release(State(s): State<Arc<Server>>, Json(req): Json<ReleaseReq>) -> Ctl {
    let which = match (req.id, req.api) {
        (Some(id), _) => Release::Hold(id),
        (None, Some(api)) => Release::Api(api),
        (None, None) => Release::All,
    };
    let outcome = req.outcome.map(FaultSpec::fault).transpose()?;
    let deadline = tokio::time::Instant::now() + SETTLE_LIMIT;
    while s.fake.held_by(&which) < req.min {
        if tokio::time::Instant::now() > deadline {
            return Err(Fail(format!(
                "release: {} of {} requests held after {}s",
                s.fake.held_by(&which),
                req.min,
                SETTLE_LIMIT.as_secs(),
            )));
        }
        // The fake has no signal for a newly parked request.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let released = s.fake.release(&which, outcome.as_ref());
    Ok(Json(json!({ "released": released })))
}

/// Removes every fault and hold, then retries what is backing off, as a
/// network change would. Requests already held stay held until released.
pub(crate) async fn clear_faults(State(s): State<Arc<Server>>) -> Ctl {
    s.fake.clear_faults();
    s.retry().await;
    Ok(done())
}

#[derive(Deserialize)]
pub(crate) struct OnReq {
    on: bool,
}

/// The network goes away (every Gmail request fails to connect) or comes
/// back.
pub(crate) async fn offline(State(s): State<Arc<Server>>, Json(req): Json<OnReq>) -> Ctl {
    s.fake.set_offline(req.on).await;
    if !req.on {
        s.retry().await;
    }
    Ok(done())
}

fn answer_json(answer: Option<&Answer>) -> Value {
    match answer {
        None => Value::Null,
        Some(Answer::Status(code)) => json!(code),
        Some(Answer::Dropped) => json!("dropped"),
        Some(Answer::NoResponse) => json!("noResponse"),
    }
}

fn request_json(r: &LoggedRequest) -> Value {
    let mut params = Map::new();
    for (k, v) in r.info.query_pairs() {
        match params.get_mut(&k) {
            Some(Value::String(have)) => {
                have.push(',');
                have.push_str(&v);
            }
            _ => {
                params.insert(k, Value::String(v));
            }
        }
    }
    json!({
        "seq": r.seq,
        "api": r.info.api,
        "method": r.info.method,
        "path": r.info.path,
        "query": r.info.query,
        "params": params,
        "account": r.info.account,
        "body": r.body,
        "at": r.at_ms,
        "held": r.held,
        "answer": answer_json(r.answer.as_ref()),
        "origin": r.origin,
        "userAgent": r.user_agent,
    })
}

/// Every request the fake received, in arrival order.
pub(crate) async fn requests(State(s): State<Arc<Server>>) -> Ctl {
    let list: Vec<Value> = s.fake.requests().iter().map(request_json).collect();
    Ok(Json(json!(list)))
}
