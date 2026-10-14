use super::*;
use crate::{
    agent::{TerminalTarget, TerminalTaskSnapshot},
    model::ModelConfigManager,
    persistence::{ArchivedMessageState, DATABASE_NAME},
    ssh::SessionRef,
    storage::private_files::PrivateDirectory,
    tasks::{ChatMessage, MessageRole, MessageState, TaskManager},
};
use std::{fs, time::Duration};
struct Fixture {
    root: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("xyz-history-host-test-{}", uuid::Uuid::new_v4()));
        PrivateDirectory::open_or_create(&root).unwrap();
        let path = root.join("state");
        Self { root, path }
    }
    fn service(&self, redactor: ObservationBuffer) -> HistoryService {
        HistoryService::new(Ok(self.path.clone()), redactor, Arc::new(|_| {}))
    }
    fn manager(&self, redactor: ObservationBuffer) -> TaskManager {
        TaskManager::with_history(
            ModelConfigManager::from_path(Err(ApiError::new(
                "SYNTHETIC_NO_MODEL",
                "Synthetic no-network fixture",
            ))),
            Arc::new(|_| true),
            self.service(redactor),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn snapshot() -> ChatSnapshot {
    ChatSnapshot {
        source: None,
        restored_read_only: false,
        chat_id: id(),
        task_id: id(),
        title: "Synthetic history".into(),
        messages: vec![ChatMessage {
            id: id(),
            role: MessageRole::Assistant,
            text: "Partial synthetic output".into(),
            run_id: id(),
            state: MessageState::Streaming,
            error: None,
            destination: None,
            model: None,
        }],
        active_run_id: Some(id()),
        revision: "1".into(),
    }
}
fn terminal(chat: &ChatSnapshot, phase: &str) -> TerminalTaskSnapshot {
    TerminalTaskSnapshot {
        chat_id: chat.chat_id.clone(),
        task_id: chat.task_id.clone(),
        run_id: id(),
        session: SessionRef {
            session_id: id(),
            generation: "99".into(),
        },
        target: TerminalTarget {
            host: "synthetic.example.invalid".into(),
            port: 22,
            username: "synthetic-user".into(),
        },
        revision: "44".into(),
        phase: phase.into(),
        instruction: "Check synthetic system".into(),
        assistant_text: "Synthetic assistant note".into(),
        action: None,
        actions: vec![],
        error: None,
        owner: false,
        remote_outcome: "unknown".into(),
        terminal_ready: false,
        round: 1,
        secret_request: None,
    }
}

#[test]
fn memory_only_manager_creates_no_history_and_restart_requires_new_opt_in() {
    let fixture = Fixture::new();
    let manager = fixture.manager(ObservationBuffer::new());
    manager.create_chat("Memory-only title").unwrap();
    manager.flush_history().unwrap();
    assert!(!manager.history_status().enabled);
    assert!(!fixture.path.exists());
    assert!(manager.set_history_enabled(true).unwrap().enabled);
    assert_eq!(manager.list_saved_chats().unwrap().len(), 1);
    manager.set_history_enabled(false).unwrap();
    let restarted = fixture.manager(ObservationBuffer::new());
    assert!(!restarted.history_status().enabled);
    assert!(restarted.list_chats().is_empty());
    assert_eq!(
        restarted.list_saved_chats().unwrap_err().code,
        "HISTORY_DISABLED"
    );
}
#[test]
fn saves_current_and_future_redacted_snapshots_and_ignores_stale_events() {
    let fixture = Fixture::new();
    let redactor = ObservationBuffer::new();
    redactor.add_known_secret("synthetic-known-token").unwrap();
    let service = fixture.service(redactor);
    let mut chat = snapshot();
    chat.title = "Synthetic synthetic-known-token".into();
    chat.messages[0].text = "value synthetic-known-token".into();
    service.set_enabled(true, &[chat.clone()]).unwrap();
    let stale = chat.clone();
    chat.revision = "3".into();
    chat.messages[0].text = "New safe text".into();
    chat.messages[0].state = MessageState::Complete;
    chat.active_run_id = None;
    service.capture(&chat);
    service.capture(&stale);
    service.flush().unwrap();
    service.set_enabled(false, &[]).unwrap();
    let store = HistoryStore::open(&fixture.path, HistoryPreference::SaveRedactedText).unwrap();
    let row = store.read(&chat.chat_id).unwrap();
    assert_eq!(row.chat.messages[0].text, "New safe text");
    assert!(!row.chat.title.contains("synthetic-known-token"));
    drop(store);
    assert!(
        !String::from_utf8_lossy(&fs::read(fixture.path.join(DATABASE_NAME)).unwrap())
            .contains("synthetic-known-token")
    );
}
#[test]
fn explicit_restore_preserves_ids_but_no_run_or_approval_and_does_not_replace_open_chat() {
    let fixture = Fixture::new();
    let service = fixture.service(ObservationBuffer::new());
    let original = snapshot();
    service
        .set_enabled(true, std::slice::from_ref(&original))
        .unwrap();
    let entry = service.list().unwrap().remove(0);
    service.set_enabled(false, &[]).unwrap();
    let manager = fixture.manager(ObservationBuffer::new());
    manager.set_history_enabled(true).unwrap();
    assert!(manager.list_chats().is_empty());
    let restored = manager
        .restore_saved_chat(&entry.chat_id, &entry.revision)
        .unwrap();
    assert_eq!(restored.chat_id, original.chat_id);
    assert_eq!(restored.task_id, original.task_id);
    assert_eq!(restored.messages[0].id, original.messages[0].id);
    assert!(restored.active_run_id.is_none());
    assert!(restored.messages[0].run_id.is_empty());
    assert_eq!(
        restored.messages[0].error.as_ref().unwrap().code,
        "HISTORY_INTERRUPTED"
    );
    assert_eq!(
        manager
            .restore_saved_chat(&entry.chat_id, &entry.revision)
            .unwrap_err()
            .code,
        "CHAT_ALREADY_OPEN"
    );
    assert_eq!(
        manager
            .send_message(&entry.chat_id, "Explicit send still needs model setup")
            .unwrap_err()
            .code,
        "CHAT_READ_ONLY"
    );
    manager.set_history_enabled(false).unwrap();
}
#[test]
fn forgetting_suppresses_resurrection_even_after_reenable() {
    let fixture = Fixture::new();
    let service = fixture.service(ObservationBuffer::new());
    let mut chat = snapshot();
    service.set_enabled(true, &[chat.clone()]).unwrap();
    let row = service.list().unwrap().remove(0);
    service.forget(&row.chat_id, &row.revision).unwrap();
    chat.revision = "2".into();
    chat.messages[0].text = "Later in-memory text".into();
    service.capture(&chat);
    service.flush().unwrap();
    assert!(service.list().unwrap().is_empty());
    service.set_enabled(false, &[]).unwrap();
    service.set_enabled(true, &[chat]).unwrap();
    assert!(service.list().unwrap().is_empty());
    service.set_enabled(false, &[]).unwrap();
}
#[test]
fn another_instance_cannot_be_silently_adopted_or_overwritten() {
    let fixture = Fixture::new();
    let service = fixture.service(ObservationBuffer::new());
    let mut chat = snapshot();
    service.set_enabled(true, &[chat.clone()]).unwrap();
    let row = service.list().unwrap().remove(0);
    let mut other = HistoryStore::open(&fixture.path, HistoryPreference::SaveRedactedText).unwrap();
    chat.title = "Other instance title".into();
    other
        .replace(
            &RedactedChat::from_snapshot(&chat, str::to_owned).unwrap(),
            row.revision.parse().unwrap(),
        )
        .unwrap();
    chat.title = "Stale local title".into();
    chat.revision = "2".into();
    service.capture(&chat);
    assert_eq!(service.flush().unwrap_err().code, "HISTORY_CONFLICT");
    assert!(!service.status().enabled);
    assert_eq!(
        other.read(&chat.chat_id).unwrap().chat.title,
        "Other instance title"
    );
    service.set_enabled(false, &[]).unwrap();
    assert_eq!(
        service.set_enabled(true, &[chat]).unwrap_err().code,
        "HISTORY_CONFLICT"
    );
    service.set_enabled(false, &[]).unwrap();
}
#[test]
fn damaged_history_is_preserved_while_memory_chats_still_work() {
    let fixture = Fixture::new();
    let service = fixture.service(ObservationBuffer::new());
    service.set_enabled(true, &[]).unwrap();
    service.set_enabled(false, &[]).unwrap();
    let bytes = b"synthetic-corrupt-history";
    fs::write(fixture.path.join(DATABASE_NAME), bytes).unwrap();
    let manager = fixture.manager(ObservationBuffer::new());
    assert_eq!(
        manager.set_history_enabled(true).unwrap_err().code,
        "HISTORY_CORRUPT"
    );
    assert_eq!(fs::read(fixture.path.join(DATABASE_NAME)).unwrap(), bytes);
    assert!(!manager.history_status().enabled);
    assert!(manager.create_chat("Still usable in memory").is_ok());
}
#[test]
fn learned_secrets_filter_older_snapshots_saved_titles_and_restore() {
    let fixture = Fixture::new();
    let redactor = ObservationBuffer::new();
    let service = fixture.service(redactor.clone());
    let mut chat = snapshot();
    chat.title = "later-learned-secret".into();
    chat.messages[0].text = "Previously unknown later-learned-secret".into();
    service.set_enabled(true, &[chat.clone()]).unwrap();
    redactor.add_known_secret("later-learned-secret").unwrap();
    let row = service.list().unwrap().remove(0);
    assert!(!row.title.contains("later-learned-secret"));
    let restored = service.restore(&row.chat_id, &row.revision).unwrap();
    assert!(!restored.messages[0].text.contains("later-learned-secret"));
    service.set_enabled(false, &[]).unwrap();
    let manager = fixture.manager(redactor.clone());
    let open = manager.create_chat("newly-learned-secret").unwrap();
    redactor.add_known_secret("newly-learned-secret").unwrap();
    assert!(!manager
        .get_chat(&open.chat_id)
        .unwrap()
        .title
        .contains("newly-learned-secret"));
    assert!(!manager
        .list_chats()
        .last()
        .unwrap()
        .title
        .contains("newly-learned-secret"));
}
#[test]
fn disable_prevents_old_epoch_capture_and_new_writes() {
    let fixture = Fixture::new();
    let service = fixture.service(ObservationBuffer::new());
    let mut chat = snapshot();
    service.set_enabled(true, &[chat.clone()]).unwrap();
    let stale = service
        .prepare(&chat, service.shared.epoch.load(Ordering::SeqCst))
        .unwrap();
    service.set_enabled(false, &[]).unwrap();
    service.set_enabled(true, &[chat.clone()]).unwrap();
    service.sender.send(Command::Capture(stale)).unwrap();
    service.flush().unwrap();
    service.set_enabled(false, &[]).unwrap();
    chat.revision = "2".into();
    chat.messages[0].text = "Must stay memory only".into();
    service.capture(&chat);
    service.flush().unwrap();
    let store = HistoryStore::open(&fixture.path, HistoryPreference::SaveRedactedText).unwrap();
    assert_eq!(
        store.read(&chat.chat_id).unwrap().chat.messages[0].text,
        "Partial synthetic output"
    );
}
#[test]
fn terminal_final_summaries_are_inert_once_bounded_and_conservative() {
    let fixture = Fixture::new();
    let redactor = ObservationBuffer::new();
    redactor
        .add_known_secret("synthetic-task-password")
        .unwrap();
    let manager = fixture.manager(redactor);
    let chat = manager.create_chat("Synthetic task").unwrap();
    manager.set_history_enabled(true).unwrap();
    let mut task = terminal(&chat, "needs_user");
    task.instruction = "Use synthetic-task-password".into();
    task.assistant_text = "synthetic-task-password; remote result unknown".into();
    manager.record_terminal_summary(&task).unwrap();
    assert!(manager.get_chat(&chat.chat_id).unwrap().messages.is_empty());
    task.phase = "cancelled".into();
    task.owner = true;
    manager.record_terminal_summary(&task).unwrap();
    manager.record_terminal_summary(&task).unwrap();
    let snapshot = manager.get_chat(&chat.chat_id).unwrap();
    assert_eq!(snapshot.messages.len(), 1);
    let text = &snapshot.messages[0].text;
    assert!(text.contains("Historical terminal task"));
    assert!(text.contains("cancelled"));
    assert!(text.contains("Unknown"));
    assert!(!text.contains("synthetic-task-password"));
    assert!(!text.contains(&task.run_id));
    assert!(!text.contains(&task.session.session_id));
    assert!(snapshot.messages[0].run_id.is_empty());
    manager.flush_history().unwrap();
    let saved = manager.list_saved_chats().unwrap().remove(0);
    manager.set_history_enabled(false).unwrap();
    let restarted = fixture.manager(ObservationBuffer::new());
    restarted.set_history_enabled(true).unwrap();
    let restored = restarted
        .restore_saved_chat(&saved.chat_id, &saved.revision)
        .unwrap();
    assert_eq!(restored.task_id, chat.task_id);
    assert!(restored.active_run_id.is_none());
    assert_eq!(restored.messages.len(), 1);
    assert_eq!(restored.messages[0].text, *text);
    restarted.set_history_enabled(false).unwrap();
}
#[test]
fn terminal_summary_omits_action_commands_and_marks_excerpt_truncation() {
    let fixture = Fixture::new();
    let manager = fixture.manager(ObservationBuffer::new());
    let chat = manager.create_chat("Synthetic task").unwrap();
    let mut task = terminal(&chat, "completed");
    task.assistant_text = "长".repeat(5000);
    task.instruction = "Purpose".into();
    task.remote_outcome = "unknown".into();
    manager.record_terminal_summary(&task).unwrap();
    let text = manager
        .get_chat(&chat.chat_id)
        .unwrap()
        .messages
        .remove(0)
        .text;
    assert!(text.contains("excerpt truncated"));
    assert!(text.len() < 16 * 1024);
    assert!(text.contains("not proof of success"));
    task.run_id = id();
    task.task_id = id();
    assert_eq!(
        manager.record_terminal_summary(&task).unwrap_err().code,
        "HISTORY_TASK_MISMATCH"
    );
}
#[test]
fn interrupted_marker_survives_later_archive_writes() {
    let fixture = Fixture::new();
    let service = fixture.service(ObservationBuffer::new());
    let original = snapshot();
    service
        .set_enabled(true, std::slice::from_ref(&original))
        .unwrap();
    let row = service.list().unwrap().remove(0);
    let mut restored = service.restore(&row.chat_id, &row.revision).unwrap();
    restored.revision = "999".into();
    service.capture(&restored);
    service.flush().unwrap();
    service.set_enabled(false, &[]).unwrap();
    let store = HistoryStore::open(&fixture.path, HistoryPreference::SaveRedactedText).unwrap();
    assert_eq!(
        store.read(&row.chat_id).unwrap().chat.messages[0].state,
        ArchivedMessageState::Interrupted
    );
}
#[test]
fn model_key_filters_chat_title_without_authorizing_a_model_destination() {
    let fixture = Fixture::new();
    let directory = PrivateDirectory::open_or_create(&fixture.root).unwrap();
    let model=b"schema_version=1\n[model]\nprotocol='openai_chat_completions'\nbase_url='http://127.0.0.1:9/v1'\napi_key='synthetic-model-key'\nname='synthetic-model'\n[transport]\nallow_loopback_http=true\n";
    directory
        .update("model.toml", 16384, |_| {
            Ok(((), Some(zeroize::Zeroizing::new(model.to_vec()))))
        })
        .unwrap();
    let config = ModelConfigManager::from_path(Ok(fixture.root.join("model.toml")));
    assert!(config.status().ready, "{:?}", config.status().error);
    assert!(!config.status().consented);
    let manager = TaskManager::with_history(
        config,
        Arc::new(|_| true),
        fixture.service(ObservationBuffer::new()),
    );
    let chat = manager.create_chat("Title synthetic-model-key").unwrap();
    assert!(!chat.title.contains("synthetic-model-key"));
    manager.set_history_enabled(true).unwrap();
    assert!(!manager.list_saved_chats().unwrap()[0]
        .title
        .contains("synthetic-model-key"));
    assert!(!manager.config_status().consented);
    manager.set_history_enabled(false).unwrap();
}

#[tokio::test]
async fn restored_context_is_read_only_and_new_chat_filters_shared_task_and_ssh_secrets() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let fixture = Fixture::new();
    let original = snapshot();
    let directory = PrivateDirectory::open_or_create(&fixture.root).unwrap();
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", server.local_addr().unwrap());
    let model=format!("schema_version=1\n[model]\nprotocol='openai_chat_completions'\nbase_url='{base}'\napi_key='synthetic-model-key'\nname='mock-model'\n[transport]\nallow_loopback_http=true\n");
    directory
        .update("model.toml", 16384, |_| {
            Ok(((), Some(zeroize::Zeroizing::new(model.into_bytes()))))
        })
        .unwrap();
    let mut old = original.clone();
    old.messages[0].text = "Older text synthetic-task-value".into();
    let seed = fixture.service(ObservationBuffer::new());
    seed.set_enabled(true, &[old]).unwrap();
    let entry = seed.list().unwrap().remove(0);
    seed.set_enabled(false, &[]).unwrap();
    let redactor = ObservationBuffer::new();
    redactor.add_known_secret("synthetic-task-value").unwrap();
    redactor.add_known_secret("synthetic-ssh-value").unwrap();
    let manager = TaskManager::with_history(
        ModelConfigManager::from_path(Ok(fixture.root.join("model.toml"))),
        Arc::new(|_| true),
        fixture.service(redactor),
    );
    manager.set_history_enabled(true).unwrap();
    manager
        .restore_saved_chat(&entry.chat_id, &entry.revision)
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), server.accept())
            .await
            .is_err()
    );
    let config = manager.config_status();
    manager
        .confirm_destination(
            config.revision.as_deref().unwrap(),
            config.destination.as_deref().unwrap(),
        )
        .unwrap();
    let restored = manager.get_chat(&original.chat_id).unwrap();
    assert!(restored.restored_read_only);
    assert!(!restored.messages[0].text.contains("synthetic-task-value"));
    assert_eq!(
        manager
            .send_message(&original.chat_id, "Never send restored context")
            .unwrap_err()
            .code,
        "CHAT_READ_ONLY"
    );
    let fresh = manager.create_chat("Fresh context").unwrap();
    let response_text = "Echo synthetic-task-value synthetic-ssh-value synthetic-model-key";
    let server_task = tokio::spawn(async move {
        let (mut socket, _) = server.accept().await.unwrap();
        let mut bytes = Vec::new();
        let body;
        loop {
            let mut chunk = [0; 4096];
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&chunk[..n]);
            assert!(bytes.len() < 512 * 1024);
            if let Some(at) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..at]);
                let length: usize = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                if bytes.len() >= at + 4 + length {
                    body = String::from_utf8(bytes[at + 4..at + 4 + length].to_vec()).unwrap();
                    break;
                }
            }
        }
        let chunk = serde_json::json!({"id":"synthetic","object":"chat.completion.chunk","created":0,"model":"mock-model","choices":[{"index":0,"delta":{"role":"assistant","content":response_text},"finish_reason":null}]});
        let last = serde_json::json!({"id":"synthetic","object":"chat.completion.chunk","created":0,"model":"mock-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
        let sse = format!("data: {chunk}\n\ndata: {last}\n\ndata: [DONE]\n\n");
        let response=format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",sse.len());
        socket.write_all(response.as_bytes()).await.unwrap();
        body
    });
    manager
        .send_message(
            &fresh.chat_id,
            "New synthetic-task-value synthetic-ssh-value",
        )
        .unwrap();
    let complete = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let chat = manager.get_chat(&fresh.chat_id).unwrap();
            if chat.active_run_id.is_none() {
                break chat;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let body = server_task.await.unwrap();
    assert!(!body.contains("Older text"));
    for secret in [
        "synthetic-task-value",
        "synthetic-ssh-value",
        "synthetic-model-key",
    ] {
        assert!(!body.contains(secret));
        assert!(!serde_json::to_string(&complete).unwrap().contains(secret));
    }
    manager.flush_history().unwrap();
    manager.set_history_enabled(false).unwrap();
    let stored = HistoryStore::open(&fixture.path, HistoryPreference::SaveRedactedText).unwrap();
    let fresh_row = stored.read(&fresh.chat_id).unwrap();
    let fresh_payload = serde_json::to_string(&fresh_row.chat).unwrap();
    for secret in [
        "synthetic-task-value",
        "synthetic-ssh-value",
        "synthetic-model-key",
    ] {
        assert!(!fresh_payload.contains(secret));
    }
    drop(stored);
    let data = fs::read(fixture.path.join(DATABASE_NAME)).unwrap();
    for secret in ["synthetic-ssh-value", "synthetic-model-key"] {
        assert!(!String::from_utf8_lossy(&data).contains(secret));
    }
    // Restore is read-only: a secret not known when an old row was saved may
    // remain on disk. Learning it redacts reads/new writes, not past disk copies.
    assert!(String::from_utf8_lossy(&data).contains("synthetic-task-value"));
}

#[test]
fn saturated_capture_queue_stops_saving_with_visible_failure() {
    let fixture = Fixture::new();
    let service = fixture.service(ObservationBuffer::new());
    let mut chat = snapshot();
    service.set_enabled(true, &[chat.clone()]).unwrap();
    let blocker = rusqlite::Connection::open(fixture.path.join(DATABASE_NAME)).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    for revision in 2..200 {
        chat.revision = revision.to_string();
        service.capture(&chat);
        if !service.status().enabled {
            break;
        }
    }
    assert!(!service.status().enabled);
    assert_eq!(service.status().error.unwrap().code, "HISTORY_QUEUE_FULL");
    blocker.execute_batch("ROLLBACK").unwrap();
    let _ = service.flush();
    service.set_enabled(false, &[]).unwrap();
}

#[test]
fn saving_failure_retains_manage_consent_but_explicit_disable_revokes_it() {
    let fixture = Fixture::new();
    let service = fixture.service(ObservationBuffer::new());
    assert!(!service.status().manageable);
    let mut chat = snapshot();
    service
        .set_enabled(true, std::slice::from_ref(&chat))
        .unwrap();
    let row = service.list().unwrap().remove(0);
    let mut other = HistoryStore::open(&fixture.path, HistoryPreference::SaveRedactedText).unwrap();
    chat.title = "External replacement".into();
    other
        .replace(
            &RedactedChat::from_snapshot(&chat, str::to_owned).unwrap(),
            row.revision.parse().unwrap(),
        )
        .unwrap();
    chat.revision = "2".into();
    service.capture(&chat);
    assert!(service.flush().is_err());
    let status = service.status();
    assert!(!status.enabled);
    assert!(status.manageable);
    let newer = service.list().unwrap().remove(0);
    assert_eq!(newer.title, "External replacement");
    service.forget(&newer.chat_id, &newer.revision).unwrap();
    assert!(service.list().unwrap().is_empty());
    service.set_enabled(false, &[]).unwrap();
    assert!(!service.status().manageable);
    assert_eq!(service.list().unwrap_err().code, "HISTORY_DISABLED");
}

#[test]
fn opt_in_snapshot_handoff_serializes_concurrent_chat_creation() {
    let fixture = Fixture::new();
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let wait = Arc::new(Mutex::new(wait));
    let history = HistoryService::new(
        Ok(fixture.path.clone()),
        ObservationBuffer::new(),
        Arc::new(move |status| {
            if status.enabled {
                entered.send(()).unwrap();
                wait.lock().unwrap().recv().unwrap();
            }
        }),
    );
    let manager = TaskManager::with_history(
        ModelConfigManager::from_path(Err(ApiError::new("SYNTHETIC_NO_MODEL", "No network"))),
        Arc::new(|_| true),
        history,
    );
    manager.create_chat("Before opt-in").unwrap();
    let enabling = manager.clone();
    let enable = std::thread::spawn(move || enabling.set_history_enabled(true));
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let creating = manager.clone();
    let (created, completed) = mpsc::sync_channel(1);
    let create = std::thread::spawn(move || {
        let chat = creating.create_chat("Concurrent new chat").unwrap();
        created.send(chat).unwrap();
    });
    assert!(completed.recv_timeout(Duration::from_millis(20)).is_err());
    release.send(()).unwrap();
    enable.join().unwrap().unwrap();
    create.join().unwrap();
    let chat = completed.recv().unwrap();
    manager.flush_history().unwrap();
    let saved = manager.list_saved_chats().unwrap();
    assert_eq!(saved.len(), 2);
    assert!(saved.iter().any(|entry| entry.chat_id == chat.chat_id));
    manager.set_history_enabled(false).unwrap();
}
