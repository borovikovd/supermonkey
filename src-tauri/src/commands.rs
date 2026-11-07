//! The Tauri commands: `connect`, which hands the core the page's channel,
//! and `dispatch`, which runs every other command through
//! [`ub_bridge::dispatch`], the code the debug web bridge runs too. A
//! command is defined once, in `ub_ipc::Command`; nothing here names one.

use serde_json::Value;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use ub_bridge::Answer;
use ub_bridge::commands::{self as cmd, CmdResult};
use ub_ipc::{ChannelMessage, LaunchInfo};

use crate::Shell;

/// The page's first call: channel messages go to `channel` from now on.
/// Shows the window, which starts hidden until the UI is ready.
#[tauri::command]
pub(crate) async fn connect(
    app: AppHandle,
    shell: State<'_, Shell>,
    channel: Channel<ChannelMessage>,
) -> CmdResult<LaunchInfo> {
    let sink = Box::new(move |message: ChannelMessage| {
        if let Err(e) = channel.send(message) {
            tracing::debug!(error = %e, "the page's channel is gone");
        }
    });
    let info = cmd::connect(&shell.core, sink).await?;
    shell
        .connected
        .store(true, std::sync::atomic::Ordering::Release);
    crate::show_main(&app);
    Ok(info)
}

/// Runs the command `command` with `args`, as the web bridge's
/// `/invoke/{command}` does. `quit_now` quits once the core has committed
/// the page's last actions.
#[tauri::command]
pub(crate) async fn dispatch(
    app: AppHandle,
    shell: State<'_, Shell>,
    command: String,
    args: Value,
) -> CmdResult<Value> {
    match ub_bridge::dispatch(&shell.core, &command, args).await? {
        Answer::Reply(reply) => Ok(reply.into_json()),
        Answer::Quit => {
            crate::quit(&app);
            Ok(Value::Null)
        }
    }
}
