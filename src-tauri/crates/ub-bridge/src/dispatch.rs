//! Commands by name, with JSON arguments and results: what the debug web
//! bridge serves (ARCHITECTURE.md, section 2, Commands).

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use ub_core::App;

use crate::commands::{self, CmdResult};

/// What a command asks of the host that runs it.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// The command ran; this is its result (`null` for none).
    Value(Value),
    /// `quit_now` or `quit`: the host quits once the core has committed the
    /// page's last actions. Only the host can do that: the app exits, and the
    /// E2E server stops the core.
    Quit,
}

/// Runs `command` with its arguments, named as the UI names them (an object
/// such as `{"account": 1, "thread": 2}`). `connect` only returns the launch
/// info here: the host connects the page's channel its own way.
pub async fn dispatch(app: &App, command: &str, mut args: Value) -> CmdResult<Reply> {
    let args = &mut args;
    Ok(Reply::Value(match command {
        "connect" => json(app.lifecycle().launch_info())?,
        "submit" => {
            commands::submit(app, arg(args, "actions")?);
            Value::Null
        }
        "fetch_rows" => {
            json(commands::fetch_rows(app, arg(args, "drafts")?, arg(args, "keys")?).await?)?
        }
        "get_bodies" => json(commands::get_bodies(app, arg(args, "ids")?).await?)?,
        "autocomplete" => {
            json(commands::autocomplete(app, arg(args, "account")?, arg(args, "query")?).await?)?
        }
        "add_account" => json(commands::add_account(app).await?)?,
        "sign_in_again" => json(commands::sign_in_again(app, arg(args, "account")?).await?)?,
        "open_attachment" => json(commands::open_attachment(app, arg(args, "attachment")?).await?)?,
        "open_url" => json(commands::open_url(app, &arg::<String>(args, "url")?)?)?,
        "unsubscribe" => {
            json(commands::unsubscribe(app, arg(args, "account")?, arg(args, "thread")?).await?)?
        }
        "block" => json(
            commands::block(
                app,
                arg(args, "account")?,
                arg(args, "thread")?,
                arg(args, "domain")?,
            )
            .await?,
        )?,
        "blocked_senders" => json(commands::blocked_senders(app, arg(args, "account")?).await?)?,
        "unblock" => {
            json(commands::unblock(app, arg(args, "account")?, &arg::<String>(args, "id")?).await?)?
        }
        "sender_threads" => {
            json(commands::sender_threads(app, arg(args, "account")?, arg(args, "thread")?).await?)?
        }
        "add_attachment" => json(
            commands::add_attachment(
                app,
                arg(args, "draft")?,
                arg(args, "filename")?,
                arg(args, "mimeType")?,
                &arg::<String>(args, "data")?,
            )
            .await?,
        )?,
        "quit_now" | "quit" => return Ok(Reply::Quit),
        "perf_report" => {
            commands::perf_report(
                &arg::<String>(args, "action")?,
                arg(args, "ms")?,
                arg(args, "budget")?,
            );
            Value::Null
        }
        "snooze_options" => json(commands::snooze_options(app, &arg::<String>(args, "text")?))?,
        other => return Err(format!("unknown command {other}")),
    }))
}

/// Takes the argument `key` out of `args`; a missing one reads as `null`.
fn arg<T: DeserializeOwned>(args: &mut Value, key: &str) -> CmdResult<T> {
    let value = args.get_mut(key).map(Value::take).unwrap_or_default();
    T::deserialize(value).map_err(|e| format!("{key}: {e}"))
}

fn json(value: impl Serialize) -> CmdResult<Value> {
    serde_json::to_value(value).map_err(|e| format!("couldn't encode the result: {e}"))
}
