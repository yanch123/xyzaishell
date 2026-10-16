#![cfg(target_os = "linux")]
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
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
        let dir =
            std::env::temp_dir().join(format!("xyz-context-runtime-{}", uuid::Uuid::new_v4()));
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

use serde_json::{json, Value};
use xyz_core::context::{
    CompactionMode, CompactionState, MeasurementScope, RequestKind, TurnPhase,
};

fn body(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}
async fn turn(
    manager: &TaskManager,
    server: &TcpListener,
    chat: &str,
    input: &str,
    answer: &str,
) -> Value {
    manager.send_message(chat, input).unwrap();
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), server.accept())
        .await
        .unwrap()
        .unwrap();
    let sent = body(&read_request(&mut socket).await);
    reply(&mut socket, "200 OK", &sse_text(answer, "stop")).await;
    let done = finished(manager, chat).await;
    assert_eq!(
        done.messages.last().unwrap().state,
        MessageState::Complete,
        "{:?}",
        done.messages.last().unwrap().error
    );
    sent
}
async fn seed(manager: &TaskManager, server: &TcpListener, chat: &str) {
    for i in 0..4 {
        turn(
            manager,
            server,
            chat,
            &format!("user constraint {i}: never execute commands"),
            &format!("old-answer-{i}: {}", "historical detail ".repeat(300)),
        )
        .await;
    }
}
async fn compacted(manager: &TaskManager, chat: &str) -> xyz_core::tasks::CompactionJobStatus {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(job) = manager.get_context_compaction(chat).unwrap() {
                if matches!(
                    job.state,
                    CompactionState::Succeeded
                        | CompactionState::Failed
                        | CompactionState::Cancelled
                        | CompactionState::Stale
                        | CompactionState::NoEffect
                ) {
                    return job;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
async fn start_compact(manager: &TaskManager, chat: &str, mode: CompactionMode) -> String {
    let usage = manager.get_context_usage(chat, None, None).await.unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    manager
        .compact_context(chat, &usage.context_revision, mode, &id)
        .await
        .unwrap();
    id
}
fn summary(request: &Value) -> String {
    let payload: Value = serde_json::from_str(
        request["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    json!({"goal":"Retain constraints", "user_constraints":[], "findings":[], "hypotheses":[], "remaining_steps":[], "uncertainties":[], "coverage":{"source_ids":payload["source_manifest"],"omitted_ranges":[]}, "summary_text":"Earlier assistant material summarized; no command execution is established."}).to_string()
}
const MANUAL: &str = "[request]\ncontext_auto_compact=false\n";

#[tokio::test]
async fn local_draft_preview_is_ephemeral_and_requires_no_consent_or_network() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, MANUAL);
    let manager = fixture.manager();
    let chat = manager.create_chat("Local preview").unwrap();
    let committed = manager
        .get_context_usage(&chat.chat_id, None, None)
        .await
        .unwrap();
    let preview = manager
        .get_context_usage(
            &chat.chat_id,
            Some(&committed.context_revision),
            Some("unsent private draft"),
        )
        .await
        .unwrap();
    assert_eq!(preview.measurement_scope, MeasurementScope::DraftPreview);
    assert_eq!(preview.context_revision, committed.context_revision);
    assert!(preview.estimate.estimated_input_tokens > committed.estimate.estimated_input_tokens);
    let current = manager
        .get_context_usage(&chat.chat_id, None, None)
        .await
        .unwrap();
    assert_eq!(current.projection_id, committed.projection_id);
    assert_eq!(current.request_kind, RequestKind::Conversation);
    assert!(manager.get_chat(&chat.chat_id).unwrap().messages.is_empty());
    assert!(
        tokio::time::timeout(Duration::from_millis(75), server.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn quick_compaction_preserves_transcript_and_constraints_but_changes_actual_request() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, MANUAL);
    let manager = fixture.manager();
    consent(&manager);
    let chat = manager.create_chat("Quick").unwrap();
    seed(&manager, &server, &chat.chat_id).await;
    let before = manager.get_chat(&chat.chat_id).unwrap();
    let usage = manager
        .get_context_usage(&chat.chat_id, None, None)
        .await
        .unwrap();
    let id = start_compact(&manager, &chat.chat_id, CompactionMode::Quick).await;
    let result = compacted(&manager, &chat.chat_id).await;
    assert_eq!(
        result.state,
        CompactionState::Succeeded,
        "{:?}",
        result.error
    );
    assert!(result.after_input_tokens < result.before_input_tokens);
    assert!(
        tokio::time::timeout(Duration::from_millis(75), server.accept())
            .await
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(manager.get_chat(&chat.chat_id).unwrap().messages).unwrap(),
        serde_json::to_value(&before.messages).unwrap()
    );
    let duplicate = manager
        .compact_context(
            &chat.chat_id,
            &usage.context_revision,
            CompactionMode::Quick,
            &id,
        )
        .await
        .unwrap();
    assert_eq!(duplicate.job_id, id);
    assert_eq!(duplicate.state, CompactionState::Succeeded);
    assert_eq!(
        manager
            .compact_context(
                &chat.chat_id,
                &usage.context_revision,
                CompactionMode::Quick,
                &uuid::Uuid::new_v4().to_string()
            )
            .await
            .unwrap_err()
            .code,
        "CONTEXT_STALE"
    );
    let sent = turn(&manager, &server, &chat.chat_id, "continue", "done").await;
    let wire = sent.to_string();
    assert!(wire.contains("Historical working-memory summary"));
    for i in 0..4 {
        assert!(wire.contains(&format!("user constraint {i}")));
    }
    assert!(!wire.contains(&before.messages[1].text));
    assert!(wire.contains(&before.messages[5].text));
    assert!(wire.contains(&before.messages[7].text));
}

#[tokio::test]
async fn llm_summary_is_no_tool_and_replaces_covered_sources_only() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, MANUAL);
    let manager = fixture.manager();
    consent(&manager);
    let chat = manager.create_chat("LLM").unwrap();
    seed(&manager, &server, &chat.chat_id).await;
    let original = manager.get_chat(&chat.chat_id).unwrap();
    manager
        .get_context_usage(&chat.chat_id, None, Some("UNSENT-DRAFT-SECRET"))
        .await
        .unwrap();
    start_compact(&manager, &chat.chat_id, CompactionMode::Llm).await;
    let (mut socket, _) = server.accept().await.unwrap();
    let sent = body(&read_request(&mut socket).await);
    assert!(sent
        .get("tools")
        .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty)));
    assert!(!sent.to_string().contains("UNSENT-DRAFT-SECRET"));
    assert!(!sent.to_string().contains("synthetic-private-key"));
    reply(&mut socket, "200 OK", &sse_text(&summary(&sent), "stop")).await;
    let result = compacted(&manager, &chat.chat_id).await;
    assert_eq!(
        result.state,
        CompactionState::Succeeded,
        "{:?}",
        result.error
    );
    assert_eq!(
        serde_json::to_value(manager.get_chat(&chat.chat_id).unwrap().messages).unwrap(),
        serde_json::to_value(&original.messages).unwrap()
    );
    let sent = turn(&manager, &server, &chat.chat_id, "continue", "done").await;
    assert!(sent
        .to_string()
        .contains("Earlier assistant material summarized"));
    assert!(!sent.to_string().contains(&original.messages[1].text));
}

#[tokio::test]
async fn invalid_llm_summary_preserves_projection_without_quick_fallback() {
    for invalid in [
        "not JSON",
        "{}",
        "unknown-reference",
        "provider-error",
        "truncated",
        "tool-call",
    ] {
        let (server, base) = listener().await;
        let fixture = Fixture::new(&base, MANUAL);
        let manager = fixture.manager();
        consent(&manager);
        let chat = manager.create_chat("Failure").unwrap();
        seed(&manager, &server, &chat.chat_id).await;
        let before = manager
            .get_context_usage(&chat.chat_id, None, None)
            .await
            .unwrap();
        start_compact(&manager, &chat.chat_id, CompactionMode::Llm).await;
        let (mut socket, _) = server.accept().await.unwrap();
        let sent = body(&read_request(&mut socket).await);
        let mut response: Value = serde_json::from_str(&summary(&sent)).unwrap();
        response["coverage"]["source_ids"] = json!(["invented-source"]);
        let (status, output) = match invalid {
            "unknown-reference" => ("200 OK", sse_text(&response.to_string(), "stop")),
            "provider-error" => (
                "401 Unauthorized",
                "private provider diagnostic synthetic-private-key".into(),
            ),
            "truncated" => ("200 OK", sse_text(&summary(&sent), "length")),
            "tool-call" => (
                "200 OK",
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({"id":"tool", "object":"chat.completion.chunk", "choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call1","type":"function","function":{"name":"terminal_action","arguments":"{}"}}]},"finish_reason":"tool_calls"}]})
                ),
            ),
            _ => ("200 OK", sse_text(invalid, "stop")),
        };
        reply(&mut socket, status, &output).await;
        let result = compacted(&manager, &chat.chat_id).await;
        assert_eq!(result.state, CompactionState::Failed);
        let after = manager
            .get_context_usage(&chat.chat_id, None, None)
            .await
            .unwrap();
        assert_eq!(before.projection_id, after.projection_id);
        assert_eq!(before.context_revision, after.context_revision);
        assert!(
            tokio::time::timeout(Duration::from_millis(75), server.accept())
                .await
                .is_err()
        );
        assert_eq!(manager.get_chat(&chat.chat_id).unwrap().messages.len(), 8);
    }
}

#[tokio::test]
async fn cancelled_or_reconfigured_compaction_cannot_commit_late_response() {
    for reconfigure in [false, true] {
        let (server, base) = listener().await;
        let fixture = Fixture::new(&base, MANUAL);
        let manager = fixture.manager();
        consent(&manager);
        let chat = manager.create_chat("Fence").unwrap();
        seed(&manager, &server, &chat.chat_id).await;
        let before = manager
            .get_context_usage(&chat.chat_id, None, None)
            .await
            .unwrap();
        let id = start_compact(&manager, &chat.chat_id, CompactionMode::Llm).await;
        let (mut socket, _) = server.accept().await.unwrap();
        let sent = body(&read_request(&mut socket).await);
        if reconfigure {
            fixture.write(
                &base,
                "[request]\ncontext_auto_compact=false\nmax_output_tokens=2048\n",
            );
            manager.reload_config();
        } else {
            assert_eq!(
                manager
                    .cancel_context_compaction(&chat.chat_id, &id)
                    .unwrap()
                    .state,
                CompactionState::Cancelled
            );
        }
        reply(&mut socket, "200 OK", &sse_text(&summary(&sent), "stop")).await;
        tokio::time::sleep(Duration::from_millis(75)).await;
        let result = compacted(&manager, &chat.chat_id).await;
        assert!(matches!(
            result.state,
            CompactionState::Cancelled | CompactionState::Stale
        ));
        let after = manager
            .get_context_usage(&chat.chat_id, None, None)
            .await
            .unwrap();
        assert_eq!(
            before.estimate.estimated_input_tokens,
            after.estimate.estimated_input_tokens
        );
        assert_eq!(manager.get_chat(&chat.chat_id).unwrap().messages.len(), 8);
    }
}

#[tokio::test]
async fn automatic_thresholds_use_turn_phase_and_cancel_does_not_start_after_turn_work() {
    for (ratio, cancel) in [(0.85_f64, false), (0.97, false), (0.85, true)] {
        let (server, base) = listener().await;
        let fixture = Fixture::new(&base, MANUAL);
        let manager = fixture.manager();
        consent(&manager);
        let chat = manager.create_chat("Phase thresholds").unwrap();
        seed(&manager, &server, &chat.chat_id).await;
        let measured = manager
            .get_context_usage(&chat.chat_id, None, None)
            .await
            .unwrap();
        let input = measured.budget.decision_input_tokens.unwrap();
        // Choose a capacity from the actual canonical estimate, not a bytes/4
        // assumption, with ample distance from each trigger boundary.
        let target_budget = (input as f64 / ratio).round() as u64;
        let window = (4097_u64..256_000)
            .find(|w| {
                let margin = 1024_u64.max(w.div_ceil(20));
                w.saturating_sub(4096 + margin) >= target_budget
            })
            .unwrap();
        fixture.write(&base, &format!("[request]\ncontext_auto_compact=true\ncontext_auto_mode='quick'\ncontext_window_tokens={window}\n"));
        manager.reload_config();
        consent(&manager);
        let started = manager.send_message(&chat.chat_id, "next").unwrap();
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), server.accept())
            .await
            .unwrap()
            .unwrap();
        let sent = body(&read_request(&mut socket).await);
        assert_eq!(sent["stream"], true);
        let active = manager
            .get_context_usage(&chat.chat_id, None, None)
            .await
            .unwrap();
        assert_eq!(active.turn_phase, TurnPhase::ActiveTurn);
        assert_eq!(active.budget.effective_trigger_ratio, 0.95);
        if ratio < 0.95 {
            assert!(active.budget.input_occupancy_ratio.unwrap() > 0.80);
            assert!(active.budget.input_occupancy_ratio.unwrap() < 0.95);
            assert!(manager
                .get_context_compaction(&chat.chat_id)
                .unwrap()
                .is_none());
            assert!(!sent
                .to_string()
                .contains("Historical working-memory summary"));
        } else {
            let job = compacted(&manager, &chat.chat_id).await;
            assert_eq!(job.state, CompactionState::Succeeded, "{:?}", job.error);
            assert!(sent
                .to_string()
                .contains("Historical working-memory summary"));
        }
        if cancel {
            manager
                .cancel_run(&chat.chat_id, started.active_run_id.as_deref().unwrap())
                .unwrap();
            reply(&mut socket, "200 OK", &sse_text("late", "stop")).await;
            tokio::time::sleep(Duration::from_millis(75)).await;
            assert!(manager
                .get_context_compaction(&chat.chat_id)
                .unwrap()
                .is_none());
        } else {
            reply(&mut socket, "200 OK", &sse_text("done", "stop")).await;
            finished(&manager, &chat.chat_id).await;
            let job = compacted(&manager, &chat.chat_id).await;
            assert_eq!(job.state, CompactionState::Succeeded, "{:?}", job.error);
            let ended = manager
                .get_context_usage(&chat.chat_id, None, None)
                .await
                .unwrap();
            assert_eq!(ended.turn_phase, TurnPhase::EndOfTurn);
            assert_eq!(ended.budget.effective_trigger_ratio, 0.80);
        }
        // Both active and after-turn automatic quick compaction stay local.
        assert!(
            tokio::time::timeout(Duration::from_millis(75), server.accept())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn new_user_turn_invalidates_inflight_summary_without_losing_new_messages() {
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, MANUAL);
    let manager = fixture.manager();
    consent(&manager);
    let chat = manager.create_chat("New turn fence").unwrap();
    seed(&manager, &server, &chat.chat_id).await;
    start_compact(&manager, &chat.chat_id, CompactionMode::Llm).await;
    let (mut summary_socket, _) = server.accept().await.unwrap();
    let summary_request = body(&read_request(&mut summary_socket).await);
    let sent = turn(
        &manager,
        &server,
        &chat.chat_id,
        "new instruction must survive",
        "fresh response complete.",
    )
    .await;
    assert!(!sent
        .to_string()
        .contains("Earlier assistant material summarized"));
    reply(
        &mut summary_socket,
        "200 OK",
        &sse_text(&summary(&summary_request), "stop"),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(75)).await;
    let job = compacted(&manager, &chat.chat_id).await;
    assert!(matches!(
        job.state,
        CompactionState::Cancelled | CompactionState::Stale
    ));
    let history = manager.get_chat(&chat.chat_id).unwrap();
    assert_eq!(history.messages.len(), 10);
    assert_eq!(history.messages[8].text, "new instruction must survive");
    assert_eq!(history.messages[9].text, "fresh response complete.");
    let sent = turn(&manager, &server, &chat.chat_id, "verify", "verified").await;
    assert!(sent.to_string().contains("new instruction must survive"));
    assert!(!sent
        .to_string()
        .contains("Earlier assistant material summarized"));
}

#[tokio::test]
async fn preview_after_destination_change_cannot_authorize_after_turn_summary_upload() {
    let (server_a, base_a) = listener().await;
    let (server_b, base_b) = listener().await;
    let fixture = Fixture::new(&base_a, MANUAL);
    let manager = fixture.manager();
    consent(&manager);
    let chat = manager.create_chat("Destination fence").unwrap();
    seed(&manager, &server_a, &chat.chat_id).await;
    let measured = manager
        .get_context_usage(&chat.chat_id, None, None)
        .await
        .unwrap();
    let target_budget = (measured.budget.decision_input_tokens.unwrap() as f64 / 0.85) as u64;
    let window = (4097_u64..256_000)
        .find(|w| w.saturating_sub(4096 + 1024_u64.max(w.div_ceil(20))) >= target_budget)
        .unwrap();
    let options = format!("[request]\ncontext_auto_compact=true\ncontext_auto_mode='llm'\ncontext_window_tokens={window}\n");
    fixture.write(&base_a, &options);
    manager.reload_config();
    consent(&manager);
    manager
        .send_message(&chat.chat_id, "authorized for A only")
        .unwrap();
    let (mut socket_a, _) = tokio::time::timeout(Duration::from_secs(5), server_a.accept())
        .await
        .unwrap()
        .unwrap();
    read_request(&mut socket_a).await;
    fixture.write(&base_b, &options);
    let changed = manager.reload_config();
    assert!(!changed.consented);
    let local = manager
        .get_context_usage(
            &chat.chat_id,
            None,
            Some("private draft never approved for B"),
        )
        .await
        .unwrap();
    assert_eq!(local.measurement_scope, MeasurementScope::DraftPreview);
    reply(
        &mut socket_a,
        "200 OK",
        &sse_text("final reply from A", "stop"),
    )
    .await;
    let done = finished(&manager, &chat.chat_id).await;
    assert_eq!(done.messages.last().unwrap().text, "final reply from A");
    assert!(!manager.config_status().consented);
    assert!(
        tokio::time::timeout(Duration::from_millis(250), server_b.accept())
            .await
            .is_err(),
        "local preview must never grant B permission to receive A's history"
    );
    assert_eq!(manager.get_chat(&chat.chat_id).unwrap().messages.len(), 10);
}

#[tokio::test]
async fn newly_registered_secret_invalidates_inflight_summary_even_when_sources_do_not_contain_it()
{
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, MANUAL);
    let redactor = xyz_core::observation::ObservationBuffer::default();
    let history_path = fixture.dir.join("history.sqlite");
    let history = xyz_core::history::HistoryService::new(
        Ok(history_path.clone()),
        redactor.clone(),
        Arc::new(|_| {}),
    );
    let manager = TaskManager::with_history(
        ModelConfigManager::from_path(Ok(fixture.path.clone())),
        Arc::new(|_| true),
        history,
    );
    consent(&manager);
    let chat = manager.create_chat("Redaction epoch").unwrap();
    seed(&manager, &server, &chat.chat_id).await;
    let before = manager
        .get_context_usage(&chat.chat_id, None, None)
        .await
        .unwrap();
    start_compact(&manager, &chat.chat_id, CompactionMode::Llm).await;
    let (mut socket, _) = server.accept().await.unwrap();
    let sent = body(&read_request(&mut socket).await);
    redactor
        .add_known_secret("newly-known-unrelated-secret-value")
        .unwrap();
    reply(&mut socket, "200 OK", &sse_text(&summary(&sent), "stop")).await;
    let result = compacted(&manager, &chat.chat_id).await;
    assert_eq!(result.state, CompactionState::Stale, "{:?}", result.error);
    let after = manager
        .get_context_usage(&chat.chat_id, None, None)
        .await
        .unwrap();
    assert_eq!(
        before.estimate.estimated_input_tokens,
        after.estimate.estimated_input_tokens
    );
    assert_eq!(manager.get_chat(&chat.chat_id).unwrap().messages.len(), 8);
    assert!(
        !history_path.exists(),
        "compaction must not enable persistent history"
    );
}

#[tokio::test]
async fn existing_llm_summary_redacts_newly_known_escaped_secret_before_next_actual_request() {
    const SECRET: &str = "new-credential-\"quoted\"\nsecond-line-sensitive";
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, MANUAL);
    let redactor = xyz_core::observation::ObservationBuffer::default();
    let history = xyz_core::history::HistoryService::new(
        Ok(fixture.dir.join("history.sqlite")),
        redactor.clone(),
        Arc::new(|_| {}),
    );
    let manager = TaskManager::with_history(
        ModelConfigManager::from_path(Ok(fixture.path.clone())),
        Arc::new(|_| true),
        history,
    );
    consent(&manager);
    let chat = manager.create_chat("Structured summary redaction").unwrap();
    seed(&manager, &server, &chat.chat_id).await;
    start_compact(&manager, &chat.chat_id, CompactionMode::Llm).await;
    let (mut socket, _) = server.accept().await.unwrap();
    let request = body(&read_request(&mut socket).await);
    let mut response: Value = serde_json::from_str(&summary(&request)).unwrap();
    response["summary_text"] =
        format!("Historical credential value {SECRET}; labels summary_text coverage end.").into();
    reply(
        &mut socket,
        "200 OK",
        &sse_text(&response.to_string(), "stop"),
    )
    .await;
    let job = compacted(&manager, &chat.chat_id).await;
    assert_eq!(job.state, CompactionState::Succeeded, "{:?}", job.error);
    redactor.add_known_secret(SECRET).unwrap();
    redactor.add_known_secret("summary_text").unwrap();
    redactor.add_known_secret("coverage").unwrap();
    // Preview refresh used to advance the redaction epoch while only scanning
    // the serialized source, missing quote/newline escapes inside its JSON.
    manager
        .get_context_usage(&chat.chat_id, None, None)
        .await
        .unwrap();
    let request = turn(&manager, &server, &chat.chat_id, "continue safely", "done.").await;
    let summary = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|m| {
            m["content"]
                .as_str()
                .filter(|s| s.starts_with("Historical working-memory summary"))
        })
        .unwrap();
    let decoded: Value = serde_json::from_str(summary.split_once('\n').unwrap().1).unwrap();
    assert!(
        decoded.get("summary_text").is_some(),
        "redaction corrupted the strict summary_text schema key: {decoded}"
    );
    assert!(
        decoded.get("coverage").is_some(),
        "redaction corrupted the strict coverage schema key: {decoded}"
    );
    serde_json::from_value::<xyz_core::context::StructuredSummary>(decoded.clone()).unwrap();
    let text = decoded["summary_text"].as_str().unwrap();
    assert!(!text.contains("summary_text"));
    assert!(!text.contains("coverage"));
    assert!(
        !text.contains(SECRET),
        "decoded outbound summary leaked a known credential"
    );
    assert!(!text.contains("second-line-sensitive"));
    assert!(text.contains("[REDACTED]"));
}

#[tokio::test]
async fn configuration_change_before_spawn_cannot_rebind_accepted_turn_or_grant_compaction_consent()
{
    let (server_a, base_a) = listener().await;
    let (server_b, base_b) = listener().await;
    let fixture = Fixture::new(&base_a, MANUAL);
    let manager = fixture.manager();
    consent(&manager);
    let chat = manager
        .create_chat("Before-spawn destination fence")
        .unwrap();
    manager
        .send_message(&chat.chat_id, "private A-only content")
        .unwrap();
    // No await: the accepted turn's spawned task has not begun preparation.
    fixture.write(&base_b, MANUAL);
    let status = manager.reload_config();
    assert!(!status.consented);
    let done = finished(&manager, &chat.chat_id).await;
    assert_eq!(done.messages[1].state, MessageState::Error);
    assert_eq!(
        done.messages[1].error.as_ref().unwrap().code,
        "MODEL_CONFIG_CHANGED"
    );
    let usage = manager
        .get_context_usage(&chat.chat_id, None, None)
        .await
        .unwrap();
    let error = manager
        .compact_context(
            &chat.chat_id,
            &usage.context_revision,
            CompactionMode::Llm,
            &uuid::Uuid::new_v4().to_string(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "MODEL_CONSENT_REQUIRED");
    assert!(
        tokio::time::timeout(Duration::from_millis(75), server_a.accept())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(75), server_b.accept())
            .await
            .is_err()
    );
    assert_eq!(
        manager.get_chat(&chat.chat_id).unwrap().messages[0].text,
        "private A-only content"
    );
}
