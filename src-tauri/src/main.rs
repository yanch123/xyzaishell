#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use xyz_core::agent::{
    AckTerminalAction, AgentManager, ContinueTerminalTask, DecideTerminalAction, EnsureTaskSecrets,
    PrepareSecretInput, StartTerminalTask, TaskRunRequest, TaskSecretsView, TerminalTaskSnapshot,
};
use xyz_core::ssh::{
    ApiError, ConnectRequest, HostKeyDecision, SessionManager, SessionRef, SessionSnapshot,
    TerminalSize, WriteReceipt,
};
use xyz_core::{
    model::{ModelConfigManager, ModelConfigStatus},
    tasks::{ChatSnapshot, TaskManager},
};

use xyz_core::host_storage::{CredentialText, HostStorage, ProfileStatus, SaveProfileRequest};
use xyz_core::storage::credentials::CredentialMode;

fn worker_error() -> ApiError {
    ApiError::new(
        "HOST_WORKER_FAILED",
        "The storage worker could not complete this operation. Reload status before retrying.",
    )
}
#[tauri::command]
async fn get_profile_status(state: State<'_, HostStorage>) -> Result<ProfileStatus, ApiError> {
    let storage = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || storage.status())
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
async fn save_connection_profile(
    request: SaveProfileRequest,
    state: State<'_, HostStorage>,
) -> Result<ProfileStatus, ApiError> {
    let storage = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || storage.save(request))
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
async fn set_session_credential(
    connection_id: String,
    expected_revision: String,
    value: Option<CredentialText>,
    state: State<'_, HostStorage>,
) -> Result<ProfileStatus, ApiError> {
    let storage = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        storage.set_session(&connection_id, &expected_revision, value)
    })
    .await
    .map_err(|_| worker_error())?
}
#[tauri::command]
async fn set_credential_mode(
    expected_revision: String,
    mode: CredentialMode,
    state: State<'_, HostStorage>,
) -> Result<ProfileStatus, ApiError> {
    let storage = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || storage.set_mode(&expected_revision, mode))
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
async fn connect_saved_profile(
    connection_id: String,
    profile_revision: String,
    size: TerminalSize,
    storage: State<'_, HostStorage>,
    state: State<'_, SessionManager>,
) -> Result<SessionSnapshot, ApiError> {
    let storage = storage.inner().clone();
    let request = tauri::async_runtime::spawn_blocking(move || {
        storage.connection(&connection_id, &profile_revision, size)
    })
    .await
    .map_err(|_| worker_error())??;
    state.connect(request)
}

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
    let agent = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agent.start(request))
        .await
        .map_err(|_| worker_error())?
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

#[tauri::command]
async fn get_task_secrets(
    request: TaskRunRequest,
    state: State<'_, AgentManager>,
) -> Result<TaskSecretsView, ApiError> {
    let agent = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agent.get_task_secrets(request))
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
async fn ensure_task_secrets(
    request: EnsureTaskSecrets,
    state: State<'_, AgentManager>,
) -> Result<TaskSecretsView, ApiError> {
    let agent = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agent.ensure_task_secrets(request))
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
async fn reload_task_secrets(
    request: TaskRunRequest,
    state: State<'_, AgentManager>,
) -> Result<TaskSecretsView, ApiError> {
    let agent = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agent.reload_task_secrets(request))
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
async fn prepare_secret_input(
    request: PrepareSecretInput,
    state: State<'_, AgentManager>,
) -> Result<TerminalTaskSnapshot, ApiError> {
    let agent = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agent.prepare_secret_input(request))
        .await
        .map_err(|_| worker_error())?
}

#[tauri::command]
fn get_history_status(state: State<'_, TaskManager>) -> xyz_core::history::HistoryStatus {
    state.history_status()
}
#[tauri::command]
async fn set_history_enabled(
    enabled: bool,
    state: State<'_, TaskManager>,
) -> Result<xyz_core::history::HistoryStatus, ApiError> {
    let tasks = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || tasks.set_history_enabled(enabled))
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
async fn list_saved_chats(
    state: State<'_, TaskManager>,
) -> Result<Vec<xyz_core::history::HistoryEntry>, ApiError> {
    let tasks = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || tasks.list_saved_chats())
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
async fn restore_saved_chat(
    chat_id: String,
    expected_revision: String,
    state: State<'_, TaskManager>,
) -> Result<ChatSnapshot, ApiError> {
    let tasks = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        tasks.restore_saved_chat(&chat_id, &expected_revision)
    })
    .await
    .map_err(|_| worker_error())?
}
#[tauri::command]
async fn forget_saved_chat(
    chat_id: String,
    expected_revision: String,
    state: State<'_, TaskManager>,
) -> Result<xyz_core::history::HistoryStatus, ApiError> {
    let tasks = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        tasks.forget_saved_chat(&chat_id, &expected_revision)
    })
    .await
    .map_err(|_| worker_error())?
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let storage = HostStorage::new();
            let ssh = SessionManager::with_persistent_trust(
                Arc::new(move |event| handle.emit("ssh:event", event).is_ok()),
                storage.configuration_root(),
            );
            let task_secrets = storage
                .configuration_root()
                .and_then(|root| xyz_core::task_secrets::TaskSecrets::open(&root))
                .map(Arc::new);
            app.manage(storage);
            let handle = app.handle().clone();
            let config = ModelConfigManager::new();
            let history_handle = app.handle().clone();
            let history = xyz_core::history::HistoryService::new(
                xyz_core::history::resolve_history_root(),
                ssh.observation(),
                Arc::new(move |status| {
                    let _ = history_handle.emit("history:event", status);
                }),
            );
            let chats = TaskManager::with_history(
                config.clone(),
                Arc::new(move |event| handle.emit("model:event", event).is_ok()),
                history,
            );
            let handle = app.handle().clone();
            let agent = AgentManager::new(
                config,
                chats.clone(),
                ssh.clone(),
                Arc::new(move |event| handle.emit("agent:event", event).is_ok()),
            );
            let agent = match task_secrets {
                Ok(secrets) => agent.with_task_secrets(secrets),
                Err(_) => agent,
            };
            app.manage(agent);
            app.manage(ssh);
            app.manage(chats);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_history_status,
            set_history_enabled,
            list_saved_chats,
            restore_saved_chat,
            forget_saved_chat,
            get_profile_status,
            save_connection_profile,
            set_session_credential,
            set_credential_mode,
            connect_saved_profile,
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
            continue_terminal_task,
            get_task_secrets,
            ensure_task_secrets,
            reload_task_secrets,
            prepare_secret_input
        ])
        .build(tauri::generate_context!())
        .expect("Unable to start xyzaishell desktop host")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(tasks) = app.try_state::<TaskManager>() {
                    let _ = tasks.flush_history();
                }
            }
        });
}
