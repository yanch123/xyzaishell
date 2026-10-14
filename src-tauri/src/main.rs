#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use xyz_core::agent::{
    AckTerminalAction, AgentManager, ContinueTerminalTask, DecideTerminalAction, StartTerminalTask,
    TaskRunRequest, TerminalTaskSnapshot,
};
use xyz_core::ssh::{
    ApiError, ConnectRequest, HostKeyDecision, SessionManager, SessionRef, SessionSnapshot,
    TerminalSize, WriteReceipt,
};
use xyz_core::{
    model::{ModelConfigManager, ModelConfigStatus},
    tasks::{ChatSnapshot, TaskManager},
};

#[tauri::command]
fn get_session_snapshot(state: State<'_, SessionManager>) -> SessionSnapshot {
    state.snapshot()
}

#[tauri::command]
async fn connect_server(
    request: ConnectRequest,
    state: State<'_, SessionManager>,
) -> Result<SessionSnapshot, ApiError> {
    state.connect(request)
}

#[tauri::command]
fn confirm_host_key(
    request: HostKeyDecision,
    state: State<'_, SessionManager>,
) -> Result<(), ApiError> {
    state.confirm_host_key(request)
}

#[tauri::command]
fn disconnect_server(
    session: SessionRef,
    state: State<'_, SessionManager>,
) -> Result<(), ApiError> {
    state.disconnect(&session)
}

#[tauri::command]
fn resize_terminal(
    session: SessionRef,
    size: TerminalSize,
    state: State<'_, SessionManager>,
) -> Result<(), ApiError> {
    state.resize(&session, size)
}

#[tauri::command]
fn ack_terminal_output(
    session: SessionRef,
    seq: String,
    state: State<'_, SessionManager>,
) -> Result<(), ApiError> {
    state.ack_output(&session, &seq)
}

#[tauri::command]
async fn send_user_input(
    session: SessionRef,
    data_base64: String,
    state: State<'_, SessionManager>,
) -> Result<WriteReceipt, ApiError> {
    state.write(&session, &data_base64).await
}

#[tauri::command]
async fn interrupt_remote(
    session: SessionRef,
    state: State<'_, SessionManager>,
) -> Result<WriteReceipt, ApiError> {
    state.write(&session, "Aw==").await
}

#[tauri::command]
fn get_model_config_status(state: State<'_, TaskManager>) -> ModelConfigStatus {
    state.config_status()
}
#[tauri::command]
async fn reload_model_config(state: State<'_, TaskManager>) -> Result<ModelConfigStatus, ApiError> {
    Ok(state.reload_config())
}
#[tauri::command]
fn confirm_model_destination(
    revision: String,
    destination: String,
    state: State<'_, TaskManager>,
) -> Result<ModelConfigStatus, ApiError> {
    state.confirm_destination(&revision, &destination)
}
#[tauri::command]
fn list_chats(state: State<'_, TaskManager>) -> Vec<ChatSnapshot> {
    state.list_chats()
}
#[tauri::command]
fn create_chat(title: String, state: State<'_, TaskManager>) -> Result<ChatSnapshot, ApiError> {
    state.create_chat(&title)
}
#[tauri::command]
fn get_chat(chat_id: String, state: State<'_, TaskManager>) -> Result<ChatSnapshot, ApiError> {
    state.get_chat(&chat_id)
}
#[tauri::command]
async fn send_chat_message(
    chat_id: String,
    text: String,
    state: State<'_, TaskManager>,
) -> Result<ChatSnapshot, ApiError> {
    state.send_message(&chat_id, &text)
}
#[tauri::command]
fn cancel_chat_run(
    chat_id: String,
    run_id: String,
    state: State<'_, TaskManager>,
) -> Result<ChatSnapshot, ApiError> {
    state.cancel_run(&chat_id, &run_id)
}
#[tauri::command]
async fn probe_model_compatibility(
    state: State<'_, TaskManager>,
) -> Result<ChatSnapshot, ApiError> {
    state.probe()
}

#[tauri::command]
fn list_terminal_tasks(state: State<'_, AgentManager>) -> Vec<TerminalTaskSnapshot> {
    state.list()
}
#[tauri::command]
fn get_terminal_task(
    task_id: String,
    state: State<'_, AgentManager>,
) -> Option<TerminalTaskSnapshot> {
    state.get(&task_id)
}
#[tauri::command]
async fn start_terminal_task(
    request: StartTerminalTask,
    state: State<'_, AgentManager>,
) -> Result<TerminalTaskSnapshot, ApiError> {
    state.start(request)
}
#[tauri::command]
async fn ack_terminal_action(
    request: AckTerminalAction,
    state: State<'_, AgentManager>,
) -> Result<TerminalTaskSnapshot, ApiError> {
    state.acknowledge(request)
}
#[tauri::command]
async fn decide_terminal_action(
    request: DecideTerminalAction,
    state: State<'_, AgentManager>,
) -> Result<TerminalTaskSnapshot, ApiError> {
    state.decide(request)
}
#[tauri::command]
fn cancel_terminal_task(
    request: TaskRunRequest,
    state: State<'_, AgentManager>,
) -> Result<TerminalTaskSnapshot, ApiError> {
    state.cancel(request)
}
#[tauri::command]
async fn continue_terminal_task(
    request: ContinueTerminalTask,
    state: State<'_, AgentManager>,
) -> Result<TerminalTaskSnapshot, ApiError> {
    state.continue_task(request)
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let ssh = SessionManager::new(Arc::new(move |event| {
                handle.emit("ssh:event", event).is_ok()
            }));
            let handle = app.handle().clone();
            let config = ModelConfigManager::new();
            let chats = TaskManager::new(
                config.clone(),
                Arc::new(move |event| handle.emit("model:event", event).is_ok()),
            );
            let handle = app.handle().clone();
            app.manage(AgentManager::new(
                config,
                chats.clone(),
                ssh.clone(),
                Arc::new(move |event| handle.emit("agent:event", event).is_ok()),
            ));
            app.manage(ssh);
            app.manage(chats);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_session_snapshot,
            connect_server,
            confirm_host_key,
            disconnect_server,
            resize_terminal,
            ack_terminal_output,
            send_user_input,
            interrupt_remote,
            get_model_config_status,
            reload_model_config,
            confirm_model_destination,
            list_chats,
            create_chat,
            get_chat,
            send_chat_message,
            cancel_chat_run,
            probe_model_compatibility,
            list_terminal_tasks,
            get_terminal_task,
            start_terminal_task,
            ack_terminal_action,
            decide_terminal_action,
            cancel_terminal_task,
            continue_terminal_task
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start xyzaishell desktop host");
}
