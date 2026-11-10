//! Every command by name, with JSON arguments and results: what both hosts
//! run, the app's Tauri `dispatch` command and the debug web bridge
//! (ARCHITECTURE.md, section 2, Commands). The names, arguments, and reply
//! types are [`Command`]'s, so this match can't miss one or answer one with
//! another's reply.

use serde_json::Value;
use sm_core::App;
use sm_ipc::{Command, Reply};

use crate::commands::{self, CmdResult};

/// What a command asks of the host that runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The command ran; this is its reply.
    Reply(Reply),
    /// `quit_now`: the host quits once the core has committed the page's
    /// last actions. Only the host can do that: the app exits, and the E2E
    /// server stops the core.
    Quit,
}

/// Runs the command `name` with its arguments, named as the page names
/// them (an object such as `{"account": 1, "thread": 2}`). `connect` only
/// returns the launch info here: each host connects the page's channel its
/// own way.
pub async fn dispatch(app: &App, name: &str, args: Value) -> CmdResult<Answer> {
    let reply = match Command::parse(name, args)? {
        Command::Connect { reply } => reply.send(app.lifecycle().launch_info()),
        Command::Submit { actions, reply } => {
            commands::submit(app, actions);
            reply.send(())
        }
        Command::FetchRows {
            drafts,
            keys,
            reply,
        } => reply.send(commands::fetch_rows(app, drafts, keys).await?),
        Command::GetBodies { ids, reply } => reply.send(commands::get_bodies(app, ids).await?),
        Command::Autocomplete {
            account,
            query,
            reply,
        } => reply.send(commands::autocomplete(app, account, query).await?),
        Command::AddAccount { reply } => reply.send(commands::add_account(app).await?),
        Command::SignInAgain { account, reply } => {
            reply.send(commands::sign_in_again(app, account).await?)
        }
        Command::OpenAttachment { attachment, reply } => {
            reply.send(commands::open_attachment(app, attachment).await?)
        }
        Command::OpenUrl { url, reply } => reply.send(commands::open_url(app, &url)?),
        Command::Unsubscribe {
            account,
            thread,
            reply,
        } => reply.send(commands::unsubscribe(app, account, thread).await?),
        Command::Block {
            account,
            thread,
            domain,
            reply,
        } => reply.send(commands::block(app, account, thread, domain).await?),
        Command::BlockedSenders { account, reply } => {
            reply.send(commands::blocked_senders(app, account).await?)
        }
        Command::Unblock { account, id, reply } => {
            reply.send(commands::unblock(app, account, &id).await?)
        }
        Command::SenderThreads {
            account,
            thread,
            reply,
        } => reply.send(commands::sender_threads(app, account, thread).await?),
        Command::AddAttachment {
            draft,
            filename,
            mime_type,
            data,
            reply,
        } => reply.send(commands::add_attachment(app, draft, filename, mime_type, &data).await?),
        Command::QuitNow { .. } => return Ok(Answer::Quit),
        Command::PerfReport {
            action,
            ms,
            budget,
            reply,
        } => {
            commands::perf_report(&action, ms, budget);
            reply.send(())
        }
        Command::SnoozeOptions { text, reply } => reply.send(commands::snooze_options(app, &text)),
    };
    reply
        .map(Answer::Reply)
        .map_err(|e| format!("couldn't encode the result: {e}"))
}
