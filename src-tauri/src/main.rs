#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod window_launch;

use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use xyz_core::agent::ReadActionOutput;
use xyz_core::agent::{
    AckTerminalAction, AgentManager, ContinueTerminalTask, ConversationScopeConsent,
    ConversationTurn, DecideTerminalAction, EnsureTaskSecrets, PrepareSecretInput,
    StartTerminalTask, TaskRunRequest, TaskSecretsView, TerminalTaskSnapshot,
};
use xyz_core::shell_integration::IntegrationStatus;
use xyz_core::shell_log::ShellLogService;
use xyz_core::shell_log_types::{
    ExportFormat, ShellLogExport, ShellLogPage, ShellLogQuery, ShellLogStatus,
};
use xyz_core::ssh::{
    ApiError, ConnectRequest, HostKeyDecision, SessionManager, SessionRef, SessionSnapshot,
    TerminalSize, WriteReceipt,
};
use xyz_core::{
    model::{
        ModelConfigManager, ModelConfigStatus, ModelTestRequest, ModelTestResult,
        SaveModelConfigRequest,
    },
    tasks::{ChatSnapshot, ChatSource, TaskManager},
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
    temporary_credential: Option<CredentialText>,
    remember_credential: Option<bool>,
    storage: State<'_, HostStorage>,
    state: State<'_, SessionManager>,
) -> Result<SessionSnapshot, ApiError> {
    let storage = storage.inner().clone();
    let profile_id = connection_id.clone();
    let request = tauri::async_runtime::spawn_blocking(move || {
        storage.connection_with_credential(
            &connection_id,
            &profile_revision,
            size,
            temporary_credential,
            remember_credential.unwrap_or(false),
        )
    })
    .await
    .map_err(|_| worker_error())??;
    state.connect_profile(request, profile_id)
}

#[tauri::command]
async fn delete_connection_profile(
    connection_id: String,
    expected_revision: String,
    state: State<'_, HostStorage>,
) -> Result<ProfileStatus, ApiError> {
    let storage = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || storage.delete(&connection_id, &expected_revision))
        .await
        .map_err(|_| worker_error())?
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
async fn save_model_config(
    request: SaveModelConfigRequest,
    state: State<'_, TaskManager>,
) -> Result<ModelConfigStatus, ApiError> {
    let tasks = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || tasks.save_config(request))
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
async fn test_model_config(
    request: ModelTestRequest,
    state: State<'_, TaskManager>,
) -> Result<ModelTestResult, ApiError> {
    state.test_config(request).await
}
#[tauri::command]
fn cancel_model_config_test(
    test_id: String,
    state: State<'_, TaskManager>,
) -> Result<(), ApiError> {
    state.cancel_config_test(&test_id)
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
fn create_chat(
    title: String,
    source: Option<SessionRef>,
    state: State<'_, TaskManager>,
    ssh: State<'_, SessionManager>,
) -> Result<ChatSnapshot, ApiError> {
    let snapshot = ssh.snapshot();
    let source = if let Some(expected) = source {
        if snapshot.session.as_ref() != Some(&expected) {
            return Err(ApiError::new(
                "CHAT_TARGET_CHANGED",
                "The SSH connection changed. Create a conversation for the current server.",
            ));
        }
        Some(ChatSource::from_session(&snapshot).ok_or_else(|| {
            ApiError::new(
                "SESSION_NOT_CONNECTED",
                "Connect before creating a server conversation.",
            )
        })?)
    } else {
        None
    };
    state.create_chat_with_source(&title, source)
}
#[tauri::command]
fn get_chat(chat_id: String, state: State<'_, TaskManager>) -> Result<ChatSnapshot, ApiError> {
    state.get_chat(&chat_id)
}
#[tauri::command]
fn confirm_conversation_scope(
    request: ConversationScopeConsent,
    state: State<'_, AgentManager>,
) -> Result<ModelConfigStatus, ApiError> {
    state.confirm_conversation_scope(request)
}
#[tauri::command]
async fn submit_conversation_turn(
    submission_id: String,
    chat_id: String,
    text: String,
    revision: String,
    destination: String,
    session: Option<SessionRef>,
    state: State<'_, AgentManager>,
) -> Result<ConversationTurn, ApiError> {
    let agent = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        agent.submit_idempotent_conversation_turn(
            &submission_id,
            &chat_id,
            &text,
            &revision,
            &destination,
            session.as_ref(),
        )
    })
    .await
    .map_err(|_| worker_error())?
}
#[tauri::command]
fn get_conversation_submission(
    submission_id: String,
    state: State<'_, AgentManager>,
) -> Result<Option<ConversationTurn>, ApiError> {
    state.get_conversation_submission(&submission_id)
}
#[tauri::command]
fn read_action_output(
    request: ReadActionOutput,
    state: State<'_, AgentManager>,
) -> Result<xyz_core::action_output::OutputPage, ApiError> {
    state.read_action_output(request)
}
#[tauri::command]
async fn send_chat_message(
    chat_id: String,
    text: String,
    state: State<'_, TaskManager>,
    ssh: State<'_, SessionManager>,
) -> Result<ChatSnapshot, ApiError> {
    state.validate_chat_target(&chat_id, &ssh.snapshot())?;
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

#[tauri::command]
fn get_shell_log_status(state: State<'_, ShellLogService>) -> ShellLogStatus {
    state.status()
}

#[tauri::command]
fn get_shell_integration_status(ssh: State<'_, SessionManager>) -> IntegrationStatus {
    ssh.shell_integration().status()
}

#[tauri::command]
async fn install_shell_integration(
    session: SessionRef,
    readiness_confirmed: bool,
    ssh: State<'_, SessionManager>,
) -> Result<IntegrationStatus, ApiError> {
    ssh.install_shell_integration(&session, readiness_confirmed)
        .await
}

#[tauri::command]
async fn start_shell_log(
    session: SessionRef,
    consent: bool,
    state: State<'_, ShellLogService>,
    ssh: State<'_, SessionManager>,
) -> Result<ShellLogStatus, ApiError> {
    if !consent {
        return Err(ApiError::new(
            "SHELL_LOG_CONSENT_REQUIRED",
            "Confirm local plaintext command recording before enabling it.",
        ));
    }
    let snapshot = ssh.snapshot();
    if snapshot.session.as_ref() != Some(&session)
        || snapshot.state != xyz_core::ssh::SessionState::Connected
    {
        return Err(ApiError::new(
            "SESSION_NOT_CONNECTED",
            "Connect before enabling command recording.",
        ));
    }
    let ready = ssh.shell_integration().ready_snapshot(&session)?;
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.start_shared(snapshot, consent, ready.shell_id, true)
    })
    .await
    .map_err(|_| worker_error())??;
    // Opening storage can yield while the connection closes. Never report a
    // stale capture as active after its disconnect event already passed.
    let current = ssh.snapshot();
    if current.session.as_ref() != Some(&session)
        || current.state != xyz_core::ssh::SessionState::Connected
    {
        state.disconnect(&session);
        return Err(ApiError::new(
            "SESSION_NOT_CONNECTED",
            "The SSH connection changed while command recording was being enabled.",
        ));
    }
    Ok(state.status())
}

#[tauri::command]
async fn set_shell_log_paused(
    paused: bool,
    state: State<'_, ShellLogService>,
    ssh: State<'_, SessionManager>,
) -> Result<ShellLogStatus, ApiError> {
    let service = state.inner().clone();
    let session = ssh.snapshot().session;
    let integration = ssh.shell_integration();
    tauri::async_runtime::spawn_blocking(move || {
        if paused {
            service.pause()
        } else {
            let session = session.ok_or_else(|| {
                ApiError::new(
                    "SESSION_NOT_CONNECTED",
                    "Connect before resuming shell logging.",
                )
            })?;
            integration.ready_snapshot(&session)?;
            service.resume(&session)
        }
    })
    .await
    .map_err(|_| worker_error())?
}

#[tauri::command]
async fn query_shell_log(
    query: ShellLogQuery,
    state: State<'_, ShellLogService>,
) -> Result<ShellLogPage, ApiError> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.query(query))
        .await
        .map_err(|_| worker_error())?
}

#[tauri::command]
async fn export_shell_log(
    query: ShellLogQuery,
    format: ExportFormat,
    state: State<'_, ShellLogService>,
) -> Result<ShellLogExport, ApiError> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.export(query, format))
        .await
        .map_err(|_| worker_error())?
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let launch = window_launch::parse_launch_target(std::env::args().skip(1))?;
            app.manage(launch);
            let handle = app.handle().clone();
            let storage = HostStorage::new();
            let ssh = SessionManager::with_persistent_trust(
                Arc::new(move |event| handle.emit("ssh:event", event).is_ok()),
                storage.configuration_root(),
            );
            let log_handle = app.handle().clone();
            let shell_log = ShellLogService::new(
                xyz_core::history::resolve_history_root(),
                ssh.observation(),
                Arc::new(move |status| {
                    let _ = log_handle.emit("shell-log:status", status);
                }),
            );
            ssh.attach_shell_log(shell_log.clone());
            app.manage(shell_log);
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
            window_launch::launch_new_window,
            window_launch::get_window_launch_target,
            delete_connection_profile,
            get_shell_integration_status,
            install_shell_integration,
            get_shell_log_status,
            start_shell_log,
            set_shell_log_paused,
            query_shell_log,
            export_shell_log,
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
            save_model_config,
            test_model_config,
            cancel_model_config_test,
            confirm_model_destination,
            list_chats,
            create_chat,
            get_chat,
            send_chat_message,
            submit_conversation_turn,
            get_conversation_submission,
            read_action_output,
            confirm_conversation_scope,
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
                if let Some(log) = app.try_state::<ShellLogService>() {
                    if let Some(ssh) = app.try_state::<SessionManager>() {
                        if let Some(session) = ssh.snapshot().session {
                            log.disconnect(&session);
                        }
                    }
                    let _ = log.flush();
                }
                if let Some(tasks) = app.try_state::<TaskManager>() {
                    let _ = tasks.flush_history();
                }
            }
        });
}
