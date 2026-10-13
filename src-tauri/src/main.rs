#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use xyz_core::ssh::{
    ApiError, ConnectRequest, HostKeyDecision, SessionManager, SessionRef, SessionSnapshot,
    TerminalSize, WriteReceipt,
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

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(SessionManager::new(Arc::new(move |event| {
                handle.emit("ssh:event", event).is_ok()
            })));
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
            interrupt_remote
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start xyzaishell desktop host");
}
