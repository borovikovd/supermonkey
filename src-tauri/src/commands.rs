use serde_json::Value;
use sm_bridge::commands::{self as cmd, CmdResult};
use sm_ipc::{ChannelMessage, LaunchInfo};
use tauri::State;
use tauri::ipc::Channel;

use crate::Shell;

#[tauri::command]
pub(crate) async fn connect(
    shell: State<'_, Shell>,
    channel: Channel<ChannelMessage>,
) -> CmdResult<LaunchInfo> {
    let sink = Box::new(move |message: ChannelMessage| {
        if let Err(e) = channel.send(message) {
            tracing::debug!(error = %e, "the page's channel is gone");
        }
    });
    shell
        .session
        .connect(sink)
        .await
        .map_err(|e| cmd::failed(&e))
}

#[tauri::command]
pub(crate) async fn dispatch(
    shell: State<'_, Shell>,
    command: String,
    args: Value,
) -> CmdResult<Value> {
    shell.session.dispatch(&command, args).await
}
