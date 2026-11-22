use serde_json::Value;
use sm_core::App;
use sm_ipc::{Command, Reply};

use crate::commands::{self, CmdResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Reply(Reply),
    Quit,
}

#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive arm per IPC command, each a single core call; splitting needs a catch-all arm"
)]
pub async fn dispatch(app: &App, name: &str, args: Value) -> CmdResult<Answer> {
    let reply = match Command::parse(name, args)? {
        Command::Connect { reply } => reply.send(app.lifecycle().launch_info()),
        Command::Submit { actions, reply } => {
            app.actions().submit(actions);
            reply.send(())
        }
        Command::FetchRows {
            drafts,
            keys,
            reply,
        } => reply.send(shown(app.queries().fetch_rows(drafts, keys).await)?),
        Command::GetBodies { ids, reply } => {
            reply.send(shown(app.queries().get_bodies(ids).await)?)
        }
        Command::Autocomplete {
            account,
            query,
            reply,
        } => reply.send(shown(app.queries().autocomplete(account, query).await)?),
        Command::AddAccount { reply } => reply.send(shown(app.accounts().add().await)?),
        Command::SignInAgain { account, reply } => {
            reply.send(shown(app.accounts().sign_in_again(account).await)?)
        }
        Command::OpenAttachment { attachment, reply } => {
            reply.send(shown(app.mail().open_attachment(attachment).await)?)
        }
        Command::OpenUrl { url, reply } => reply.send(shown(app.mail().open_url(&url))?),
        Command::Unsubscribe {
            account,
            thread,
            reply,
        } => reply.send(shown(app.mail().unsubscribe(account, thread).await)?),
        Command::Block {
            account,
            thread,
            domain,
            reply,
        } => reply.send(shown(app.mail().block(account, thread, domain).await)?),
        Command::BlockedSenders { account, reply } => {
            reply.send(shown(app.mail().blocked_senders(account).await)?)
        }
        Command::Unblock { account, id, reply } => {
            reply.send(shown(app.mail().unblock(account, id).await)?)
        }
        Command::SenderThreads {
            account,
            thread,
            reply,
        } => reply.send(shown(app.queries().sender_threads(account, thread).await)?),
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
        Command::SnoozeOptions { text, reply } => reply.send(app.queries().snooze_options(&text)),
    };
    reply
        .map(Answer::Reply)
        .map_err(|e| format!("couldn't encode the result: {e}"))
}

fn shown<T>(result: Result<T, sm_core::Error>) -> CmdResult<T> {
    result.map_err(|e| commands::message(&e))
}
