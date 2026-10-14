#![cfg(target_os = "linux")]
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
use xyz_core::{
    model::ModelConfigManager,
    tasks::{ChatSnapshot, MessageState, TaskManager},
};

struct Fixture {
    dir: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new(base: &str, extra: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("xyz-model-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("model.toml");
        let this = Self { dir, path };
        this.write(base, extra);
        this
    }
    fn write(&self, base: &str, extra: &str) {
        fs::write(&self.path, format!("schema_version=1\n[model]\nprotocol='openai_chat_completions'\nbase_url='{base}'\napi_key='synthetic-private-key'\nname='mock-model'\n[transport]\nallow_loopback_http=true\n{extra}")).unwrap();
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn manager(&self) -> TaskManager {
        TaskManager::new(
            ModelConfigManager::from_path(Ok(self.path.clone())),
            Arc::new(|_| true),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn consent(manager: &TaskManager) {
    let s = manager.config_status();
    assert!(s.ready, "{:?}", s.error);
    manager
        .confirm_destination(
            s.revision.as_deref().unwrap(),
            s.destination.as_deref().unwrap(),
        )
        .unwrap();
}
async fn listener() -> (TcpListener, String) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", l.local_addr().unwrap());
    (l, url)
}
async fn read_request(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let mut part = [0; 4096];
        let n = socket.read(&mut part).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&part[..n]);
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..index]);
            let length: usize = header
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .map(str::to_owned)
                })
                .unwrap()
                .parse()
                .unwrap();
            if bytes.len() >= index + 4 + length {
                return String::from_utf8(bytes).unwrap();
            }
        }
        assert!(bytes.len() <= 512 * 1024);
    }
}
fn sse_text(text: &str, finish: &str) -> String {
    let mut body = String::new();
    let chars: Vec<_> = text.chars().collect();
    for part in chars.chunks(16 * 1024) {
        let text: String = part.iter().collect();
        let chunk = serde_json::json!({"id":"mock","object":"chat.completion.chunk","created":0,"model":"mock-model","choices":[{"index":0,"delta":{"role":"assistant","content":text},"finish_reason":null}]});
        body.push_str(&format!("data: {chunk}\n\n"));
    }
    let last = serde_json::json!({"id":"mock","object":"chat.completion.chunk","created":0,"model":"mock-model","choices":[{"index":0,"delta":{},"finish_reason":finish}]});
    body.push_str(&format!("data: {last}\n\ndata: [DONE]\n\n"));
    body
}
async fn reply(socket: &mut TcpStream, status: &str, body: &str) {
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = socket.write_all(response.as_bytes()).await;
}
async fn finished(manager: &TaskManager, id: &str) -> ChatSnapshot {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let c = manager.get_chat(id).unwrap();
            if c.active_run_id.is_none() {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn no_network_before_exact_destination_consent() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, "");
    let manager = fixture.manager();
    let chat = manager.create_chat("Private chat").unwrap();
    let status = manager.config_status();
    assert!(status.ready);
    assert!(!status.consented);
    assert_eq!(
        manager
            .send_message(&chat.chat_id, "private user text")
            .unwrap_err()
            .code,
        "MODEL_CONSENT_REQUIRED"
    );
    assert_eq!(manager.probe().unwrap_err().code, "MODEL_CONSENT_REQUIRED");
    assert!(manager
        .confirm_destination(
            status.revision.as_deref().unwrap(),
            "https://other.example/v1"
        )
        .is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), server.accept())
            .await
            .is_err()
    );
    assert!(manager.get_chat(&chat.chat_id).unwrap().messages.is_empty());
    assert!(!serde_json::to_string(&status)
        .unwrap()
        .contains("synthetic-private-key"));
}
#[tokio::test]
async fn streams_real_text_and_keeps_chat_history_separate() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, "");
    let manager = fixture.manager();
    consent(&manager);
    let server = tokio::spawn(async move {
        let mut requests = vec![];
        for answer in ["你好, streamed reply", "second reply", "other chat reply"] {
            let (mut socket, _) = server.accept().await.unwrap();
            requests.push(read_request(&mut socket).await);
            reply(&mut socket, "200 OK", &sse_text(answer, "stop")).await;
        }
        requests
    });
    let one = manager.create_chat("One").unwrap();
    let two = manager.create_chat("Two").unwrap();
    manager
        .send_message(&one.chat_id, "first-private-turn")
        .unwrap();
    let one_done = finished(&manager, &one.chat_id).await;
    assert_eq!(one_done.messages[1].text, "你好, streamed reply");
    assert_eq!(one_done.messages[1].state, MessageState::Complete);
    manager
        .send_message(&one.chat_id, "follow-up-turn")
        .unwrap();
    finished(&manager, &one.chat_id).await;
    manager.send_message(&two.chat_id, "isolated-turn").unwrap();
    finished(&manager, &two.chat_id).await;
    let requests = server.await.unwrap();
    assert!(requests[0].starts_with("POST /v1/chat/completions HTTP/1.1"));
    assert!(requests[0].contains("Bearer synthetic-private-key"));
    for request in &requests {
        let body: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["stream"], true);
        assert!(body
            .get("tools")
            .is_none_or(|t| t.as_array().is_some_and(Vec::is_empty)));
        assert!(!body.to_string().contains("synthetic-private-key"));
    }
    assert!(requests[1].contains("first-private-turn"));
    assert!(requests[1].contains("streamed reply"));
    assert!(!requests[2].contains("first-private-turn"));
    assert!(!requests[2].contains("follow-up-turn"));
    assert_eq!(manager.get_chat(&one.chat_id).unwrap().task_id, one.task_id);
}
#[tokio::test]
async fn cancel_before_spawn_transmits_nothing_and_rejects_late_state() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, "");
    let manager = fixture.manager();
    consent(&manager);
    let chat = manager.create_chat("Cancel").unwrap();
    let started = manager
        .send_message(&chat.chat_id, "must not send")
        .unwrap();
    let cancelled = manager
        .cancel_run(&chat.chat_id, started.active_run_id.as_deref().unwrap())
        .unwrap();
    assert_eq!(cancelled.messages[1].state, MessageState::Cancelled);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), server.accept())
            .await
            .is_err()
    );
    assert_eq!(
        manager.get_chat(&chat.chat_id).unwrap().revision,
        cancelled.revision
    );
    assert!(manager
        .cancel_run(&chat.chat_id, started.active_run_id.as_deref().unwrap())
        .is_err());
}
#[tokio::test]
async fn cancel_inflight_retains_partial_but_never_completes_late() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, "");
    let manager = fixture.manager();
    consent(&manager);
    let (received, wait) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = server.accept().await.unwrap();
        read_request(&mut socket).await;
        let _ = received.send(());
        tokio::time::sleep(Duration::from_millis(200)).await;
        reply(&mut socket, "200 OK", &sse_text("late output", "stop")).await;
    });
    let chat = manager.create_chat("Cancel active").unwrap();
    let started = manager.send_message(&chat.chat_id, "test").unwrap();
    wait.await.unwrap();
    let cancelled = manager
        .cancel_run(&chat.chat_id, started.active_run_id.as_deref().unwrap())
        .unwrap();
    server.await.unwrap();
    assert_eq!(
        manager.get_chat(&chat.chat_id).unwrap().revision,
        cancelled.revision
    );
    assert!(manager.get_chat(&chat.chat_id).unwrap().messages[1]
        .text
        .is_empty());
}
#[tokio::test]
async fn reload_revokes_consent_and_running_request_keeps_snapshot() {
    let (old_server, old_base) = listener().await;
    let (new_server, new_base) = listener().await;
    let fixture = Fixture::new(&old_base, "");
    let manager = fixture.manager();
    consent(&manager);
    let (received, wait) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = old_server.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        received.send(request).unwrap();
        released.await.unwrap();
        reply(
            &mut socket,
            "200 OK",
            &sse_text("old recipient reply", "stop"),
        )
        .await;
    });
    let chat = manager.create_chat("Snapshot").unwrap();
    manager
        .send_message(&chat.chat_id, "old recipient only")
        .unwrap();
    wait.await.unwrap();
    fixture.write(&new_base, "");
    let status = manager.reload_config();
    assert!(!status.consented);
    assert_eq!(status.destination.as_deref(), Some(new_base.as_str()));
    let new_chat = manager.create_chat("New").unwrap();
    assert_eq!(
        manager
            .send_message(&new_chat.chat_id, "not authorized")
            .unwrap_err()
            .code,
        "MODEL_CONSENT_REQUIRED"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), new_server.accept())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    let done = finished(&manager, &chat.chat_id).await;
    assert_eq!(
        done.messages[1].destination.as_deref(),
        Some(old_base.as_str())
    );
    server.await.unwrap();
}
#[tokio::test]
async fn provider_errors_never_expose_body_or_credentials() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, "");
    let manager = fixture.manager();
    consent(&manager);
    let server = tokio::spawn(async move {
        let (mut socket, _) = server.accept().await.unwrap();
        read_request(&mut socket).await;
        reply(
            &mut socket,
            "401 Unauthorized",
            "synthetic-private-key private-user-text",
        )
        .await;
    });
    let chat = manager.create_chat("Error").unwrap();
    manager.send_message(&chat.chat_id, "hello").unwrap();
    let done = finished(&manager, &chat.chat_id).await;
    assert_eq!(done.messages[1].state, MessageState::Error);
    let serialized = serde_json::to_string(&done).unwrap();
    assert!(!serialized.contains("synthetic-private-key"));
    assert!(!serialized.contains("private-user-text"));
    server.await.unwrap();
}
#[tokio::test]
async fn output_bounds_and_token_limit_are_visible_failures() {
    for (text, finish, code) in [
        ("x".repeat(65537), "stop", "MODEL_OUTPUT_LIMIT"),
        ("partial".into(), "length", "MODEL_OUTPUT_TRUNCATED"),
    ] {
        let (server, base) = listener().await;
        let fixture = Fixture::new(&base, "");
        let manager = fixture.manager();
        consent(&manager);
        let server = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            read_request(&mut socket).await;
            reply(&mut socket, "200 OK", &sse_text(&text, finish)).await;
        });
        let chat = manager.create_chat("Limit").unwrap();
        manager.send_message(&chat.chat_id, "hello").unwrap();
        let done = finished(&manager, &chat.chat_id).await;
        assert_eq!(done.messages[1].state, MessageState::Error);
        assert_eq!(done.messages[1].error.as_ref().unwrap().code, code);
        assert!(done.messages[1].text.len() <= 65536);
        server.await.unwrap();
    }
}
#[tokio::test]
async fn idle_deadline_is_bounded() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(
        &base,
        "[request]\nstream_idle_timeout_seconds=1\nrequest_timeout_seconds=3\n",
    );
    let manager = fixture.manager();
    consent(&manager);
    let server = tokio::spawn(async move {
        let (mut socket, _) = server.accept().await.unwrap();
        read_request(&mut socket).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    });
    let chat = manager.create_chat("Idle").unwrap();
    manager.send_message(&chat.chat_id, "hello").unwrap();
    let done = finished(&manager, &chat.chat_id).await;
    assert_eq!(done.messages[1].state, MessageState::Error);
    assert_eq!(
        done.messages[1].error.as_ref().unwrap().code,
        "MODEL_IDLE_TIMEOUT"
    );
    server.abort();
}
#[test]
fn secure_config_rejects_unsafe_file_paths_and_redacts_key() {
    let fixture = Fixture::new("https://example.com/v1", "");
    assert!(fixture.manager().config_status().ready);
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!fixture.manager().config_status().ready);
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o600)).unwrap();
    let link = fixture.dir.join("alias.toml");
    std::os::unix::fs::symlink(&fixture.path, &link).unwrap();
    assert!(!ModelConfigManager::from_path(Ok(link)).status().ready);
    let hard = fixture.dir.join("hard.toml");
    fs::hard_link(&fixture.path, &hard).unwrap();
    assert!(!fixture.manager().config_status().ready);
    fs::remove_file(hard).unwrap();
    fs::set_permissions(&fixture.dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!fixture.manager().config_status().ready);
}

#[tokio::test]
async fn total_deadline_stops_a_live_stream_without_retry() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(
        &base,
        "[request]\nstream_idle_timeout_seconds=2\nrequest_timeout_seconds=1\n",
    );
    let manager = fixture.manager();
    consent(&manager);
    let server = tokio::spawn(async move {
        let (mut socket, _) = server.accept().await.unwrap();
        read_request(&mut socket).await;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();
        for _ in 0..20 {
            let item = serde_json::json!({"id":"live","object":"chat.completion.chunk","created":0,"model":"mock-model","choices":[{"index":0,"delta":{"content":"x"},"finish_reason":null}]});
            let frame = format!("data: {item}\n\n");
            let chunk = format!("{:x}\r\n{frame}\r\n", frame.len());
            if socket.write_all(chunk.as_bytes()).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), server.accept())
                .await
                .is_err()
        );
    });
    let chat = manager.create_chat("Total deadline").unwrap();
    manager.send_message(&chat.chat_id, "hello").unwrap();
    let done = finished(&manager, &chat.chat_id).await;
    assert_eq!(done.messages[1].state, MessageState::Error);
    assert!(matches!(
        done.messages[1].error.as_ref().unwrap().code.as_str(),
        "MODEL_TIMEOUT" | "MODEL_REQUEST_FAILED"
    ));
    assert!(!done.messages[1].text.is_empty());
    server.await.unwrap();
}
#[tokio::test]
async fn unsolicited_tool_calls_are_rejected_without_execution() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, "");
    let manager = fixture.manager();
    consent(&manager);
    let server = tokio::spawn(async move {
        let (mut socket, _) = server.accept().await.unwrap();
        read_request(&mut socket).await;
        let item = serde_json::json!({"id":"tool","object":"chat.completion.chunk","created":0,"model":"mock-model","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"terminal_action","arguments":"{\"command_text\":\"touch /tmp/forbidden\"}"}}]},"finish_reason":"tool_calls"}]});
        reply(
            &mut socket,
            "200 OK",
            &format!("data: {item}\n\ndata: [DONE]\n\n"),
        )
        .await;
    });
    let chat = manager.create_chat("Tools forbidden").unwrap();
    manager.send_message(&chat.chat_id, "hello").unwrap();
    let done = finished(&manager, &chat.chat_id).await;
    assert_eq!(done.messages[1].state, MessageState::Error);
    assert_eq!(
        done.messages[1].error.as_ref().unwrap().code,
        "MODEL_TOOLS_UNAVAILABLE"
    );
    server.await.unwrap();
}
#[tokio::test]
async fn local_input_and_concurrency_limits_fail_before_transmission() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, "");
    let manager = fixture.manager();
    consent(&manager);
    let mut chats = vec![];
    for i in 0..32 {
        chats.push(manager.create_chat(&format!("Chat {i}")).unwrap());
    }
    assert_eq!(
        manager.create_chat("Excess").unwrap_err().code,
        "CHAT_LIMIT"
    );
    for text in [" ".into(), "x".repeat(16385), "nul\0text".into()] {
        assert_eq!(
            manager
                .send_message(&chats[0].chat_id, &text)
                .unwrap_err()
                .code,
            "CHAT_INPUT_INVALID"
        );
    }
    let mut starts = vec![];
    for chat in &chats[..4] {
        starts.push(manager.send_message(&chat.chat_id, "pending").unwrap());
    }
    assert_eq!(
        manager
            .send_message(&chats[4].chat_id, "extra concurrent request")
            .unwrap_err()
            .code,
        "MODEL_BUSY"
    );
    for chat in starts {
        manager
            .cancel_run(&chat.chat_id, chat.active_run_id.as_deref().unwrap())
            .unwrap();
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), server.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn transcript_byte_and_message_limits_do_not_silently_trim() {
    for (turns, answer) in [(3, "x".repeat(65536)), (64, "ok".into())] {
        let (server, base) = listener().await;
        let fixture = Fixture::new(&base, "");
        let manager = fixture.manager();
        consent(&manager);
        let server = tokio::spawn(async move {
            for _ in 0..turns {
                let (mut socket, _) = server.accept().await.unwrap();
                read_request(&mut socket).await;
                reply(&mut socket, "200 OK", &sse_text(&answer, "stop")).await;
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(100), server.accept())
                    .await
                    .is_err()
            );
        });
        let chat = manager.create_chat("Bounded history").unwrap();
        for _ in 0..turns {
            manager.send_message(&chat.chat_id, "a").unwrap();
            let done = finished(&manager, &chat.chat_id).await;
            assert_eq!(done.messages.last().unwrap().state, MessageState::Complete);
        }
        let before = manager.get_chat(&chat.chat_id).unwrap();
        assert_eq!(
            manager.send_message(&chat.chat_id, "b").unwrap_err().code,
            "CHAT_CONTEXT_FULL"
        );
        let after = manager.get_chat(&chat.chat_id).unwrap();
        assert_eq!(after.messages.len(), turns * 2);
        assert_eq!(after.messages[0].text, "a");
        assert_eq!(before.revision, after.revision);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn rejected_connection_probe_does_not_leak_an_empty_chat_slot() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, "");
    let manager = fixture.manager();
    consent(&manager);
    let mut active = vec![];
    for _ in 0..4 {
        let chat = manager.create_chat("Existing").unwrap();
        active.push(manager.send_message(&chat.chat_id, "pending").unwrap());
    }
    assert_eq!(manager.probe().unwrap_err().code, "MODEL_BUSY");
    assert_eq!(manager.list_chats().len(), 4);
    for chat in active {
        manager
            .cancel_run(&chat.chat_id, chat.active_run_id.as_deref().unwrap())
            .unwrap();
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), server.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn split_key_echo_is_redacted_with_original_snapshot_across_reload_and_cancel() {
    for cancel_run in [false, true] {
        let (server, base) = listener().await;
        let fixture = Fixture::new(&base, "");
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = events.clone();
        let manager = TaskManager::new(
            ModelConfigManager::from_path(Ok(fixture.path.clone())),
            Arc::new(move |event| {
                recorded.lock().unwrap().push(event);
                true
            }),
        );
        consent(&manager);
        let (partial_tx, partial_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let responder = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let event = |content: &str, finish: serde_json::Value| {
                format!(
                    "data: {}\n\n",
                    serde_json::json!({"id":"mock","object":"chat.completion.chunk","created":0,"model":"mock-model","choices":[{"index":0,"delta":{"role":"assistant","content":content},"finish_reason":finish}]})
                )
            };
            let first = event("prefix synthetic-private-", serde_json::Value::Null);
            let last = event("key suffix", serde_json::json!("stop")) + "data: [DONE]\n\n";
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{first}",first.len()+last.len()).as_bytes()).await.unwrap();
            partial_tx.send(()).unwrap();
            release_rx.await.unwrap();
            let _ = socket.write_all(last.as_bytes()).await;
            request
        });
        let chat = manager.create_chat("Secret echo test").unwrap();
        let running = manager
            .send_message(&chat.chat_id, "Do not expose synthetic-private-key")
            .unwrap();
        partial_rx.await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let chat = manager.get_chat(&chat.chat_id).unwrap();
                if chat.messages[1].text.contains("[REDACTED]") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let config_text = fs::read_to_string(&fixture.path)
            .unwrap()
            .replace("synthetic-private-key", "new-config-private-key");
        fs::write(&fixture.path, config_text).unwrap();
        manager.reload_config();
        if cancel_run {
            manager
                .cancel_run(&chat.chat_id, running.active_run_id.as_ref().unwrap())
                .unwrap();
        }
        release_tx.send(()).unwrap();
        let done = finished(&manager, &chat.chat_id).await;
        if cancel_run {
            assert_eq!(done.messages[1].state, MessageState::Cancelled);
        } else {
            assert_eq!(done.messages[1].text, "prefix [REDACTED] suffix");
        }
        let request = responder.await.unwrap();
        assert!(!request
            .split("\r\n\r\n")
            .nth(1)
            .unwrap()
            .contains("synthetic-private-key"));
        let public = serde_json::to_string(&*events.lock().unwrap()).unwrap();
        assert!(!public.contains("synthetic-private-"));
        assert!(!serde_json::to_string(&done)
            .unwrap()
            .contains("synthetic-private-"));
    }
}
