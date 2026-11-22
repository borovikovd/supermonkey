use serde_json::Value;
use sm_bridge::Answer;
use sm_bridge::commands::{self as cmd, CmdResult};
use sm_ipc::{ChannelMessage, LaunchInfo};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};

use crate::Shell;

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
    let info = shell
        .core
        .lifecycle()
        .connect(sink)
        .await
        .map_err(|e| cmd::failed(&e))?;
    shell
        .connected
        .store(true, std::sync::atomic::Ordering::Release);
    crate::show_main(&app);
    Ok(info)
}

#[tauri::command]
pub(crate) async fn dispatch(
    app: AppHandle,
    shell: State<'_, Shell>,
    command: String,
    args: Value,
) -> CmdResult<Value> {
    match sm_bridge::dispatch(&shell.core, &command, args).await? {
        Answer::Reply(reply) => Ok(reply.into_json()),
        Answer::Quit => {
            crate::quit(app);
            Ok(Value::Null)
        }
    }
}
