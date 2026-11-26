use serde_json::Value;
use tauri::State;
use tauri::ipc::Channel;

use crate::core::{ChannelSink, log_err};
use crate::ipc::{ChannelMessage, CommandError, LaunchInfo, Reply};
use crate::session::Session;

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command macro requires owned Channel and State parameters"
)]
pub(super) fn connect(
    session: State<'_, Session>,
    channel: Channel<ChannelMessage>,
) -> Result<LaunchInfo, CommandError> {
    let sink = ChannelSink::new(move |msg: ChannelMessage| {
        log_err("send channel message to webview", channel.send(msg));
    });
    session.connect(sink)
}

#[tauri::command]
pub(super) async fn dispatch(
    session: State<'_, Session>,
    command: String,
    args: Value,
) -> Result<Value, CommandError> {
    session.dispatch(&command, args).await.map(Reply::into_json)
}
