//! Real loopback SSE + SSH protocol tests; no operating-system command shell is
//! invoked. The fixture records bytes on the same visible PTY channel.
#![cfg(target_os = "linux")]
#[allow(dead_code)]
mod support;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use support::{Behavior, LoopbackServer, PASSWORD, USER, WAIT};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
};
use xyz_core::{
    agent::{
        AckTerminalAction, AgentManager, ContinueTerminalTask, DecideTerminalAction,
        StartTerminalTask, TaskRunRequest, TerminalContextConsent, TerminalTaskSnapshot,
    },
    model::ModelConfigManager,
    ssh::{
        Authentication, ConnectRequest, HostKeyDecision, Secret, SessionManager, SessionRef,
        SessionState, SshEvent, TerminalSize, TrustDecision,
    },
    tasks::TaskManager,
};
struct Fixture {
    directory: PathBuf,
    config: ModelConfigManager,
    chats: TaskManager,
    ssh: SessionManager,
    agent: AgentManager,
    session: SessionRef,
    auto_ack: Arc<AtomicBool>,
    output_seq: Arc<AtomicU64>,
    output_bytes: Arc<AtomicUsize>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.ssh.disconnect(&self.session);
        let _ = fs::remove_dir_all(&self.directory);
    }
}
impl Fixture {
    async fn new(server: &LoopbackServer, base: &str) -> Self {
        let directory = std::env::temp_dir().join(format!("xyz-agent-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("model.toml");
        fs::write(&path,format!("schema_version=1\n[model]\nprotocol='openai_chat_completions'\nbase_url='{base}'\napi_key='synthetic-model-credential'\nname='fixture'\n[transport]\nallow_loopback_http=true\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let config = ModelConfigManager::from_path(Ok(path));
        let chats = TaskManager::new(config.clone(), Arc::new(|_| true));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let ssh = SessionManager::new(Arc::new(move |e| tx.send(e).is_ok()));
        let s = ssh.clone();
        let auto_ack = Arc::new(AtomicBool::new(true));
        let output_seq = Arc::new(AtomicU64::new(0));
        let output_bytes = Arc::new(AtomicUsize::new(0));
        let ack = auto_ack.clone();
        let seq_cursor = output_seq.clone();
        let bytes_cursor = output_bytes.clone();
        let (ready, done) = oneshot::channel();
        tokio::spawn(async move {
            let mut ready = Some(ready);
            while let Some(e) = rx.recv().await {
                match e {
                    SshEvent::HostKey { challenge } => s
                        .confirm_host_key(HostKeyDecision {
                            challenge_id: challenge.challenge_id,
                            expected_fingerprint: challenge.fingerprint,
                            decision: TrustDecision::Accept,
                        })
                        .unwrap(),
                    SshEvent::Output {
                        session,
                        seq,
                        data_base64,
                        ..
                    } => {
                        seq_cursor.store(seq.parse().unwrap(), Ordering::SeqCst);
                        bytes_cursor.fetch_add(
                            STANDARD.decode(data_base64).unwrap().len(),
                            Ordering::SeqCst,
                        );
                        if ack.load(Ordering::SeqCst) {
                            s.ack_output(&session, &seq).unwrap();
                        }
                    }
                    SshEvent::State { snapshot } if snapshot.state == SessionState::Connected => {
                        if let Some(ready) = ready.take() {
                            let _ = ready.send(());
                        }
                    }
                    _ => {}
                }
            }
        });
        let session = ssh
            .connect(ConnectRequest {
                host: "127.0.0.1".into(),
                port: server.addr.port(),
                username: USER.into(),
                auth: Authentication::Password {
                    password: Secret::new(PASSWORD.into()),
                },
                session_only: true,
                size: TerminalSize { cols: 80, rows: 24 },
            })
            .unwrap()
            .session
            .unwrap();
        tokio::time::timeout(WAIT, done).await.unwrap().unwrap();
        let agent = AgentManager::new(
            config.clone(),
            chats.clone(),
            ssh.clone(),
            Arc::new(|_| true),
        );
        Self {
            directory,
            config,
            chats,
            ssh,
            agent,
            session,
            auto_ack,
            output_seq,
            output_bytes,
        }
    }
    fn request(&self, chat_id: &str) -> StartTerminalTask {
        let s = self.config.status();
        StartTerminalTask {
            chat_id: chat_id.into(),
            session: self.session.clone(),
            instruction: "Inspect the host; never claim unknown results succeeded.".into(),
            readiness_confirmed: true,
            context_consent: TerminalContextConsent {
                revision: s.revision.unwrap(),
                destination: s.destination.unwrap(),
            },
        }
    }
    fn start(&self) -> TerminalTaskSnapshot {
        let c = self.chats.create_chat("Terminal test").unwrap();
        self.agent.start(self.request(&c.chat_id)).unwrap()
    }
}
async fn listener() -> (TcpListener, String) {
    let s = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", s.local_addr().unwrap());
    (s, base)
}
async fn read_request(socket: &mut TcpStream) -> String {
    let mut all = vec![];
    loop {
        let mut buf = [0; 4096];
        let n = socket.read(&mut buf).await.unwrap();
        assert!(n > 0);
        all.extend_from_slice(&buf[..n]);
        if let Some(i) = all.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&all[..i]);
            let count: usize = head
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .map(str::to_owned)
                })
                .unwrap()
                .parse()
                .unwrap();
            if all.len() >= i + 4 + count {
                return String::from_utf8(all).unwrap();
            }
        }
        assert!(all.len() < 256 * 1024);
    }
}
fn chunk(delta: serde_json::Value, finish: serde_json::Value) -> String {
    format!(
        "data: {}\n\n",
        serde_json::json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    )
}
fn tool_body(name: &str, args: &str, finish: &str) -> String {
    chunk(
        serde_json::json!({"role":"assistant","tool_calls":[{"index":0,"id":"fixture-call","type":"function","function":{"name":name,"arguments":args}}]}),
        serde_json::Value::Null,
    ) + &chunk(serde_json::json!({}), serde_json::json!(finish))
        + "data: [DONE]\n\n"
}
async fn reply(socket: &mut TcpStream, body: &str) {
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
}
async fn phase(agent: &AgentManager, id: &str, wanted: &str) -> TerminalTaskSnapshot {
    tokio::time::timeout(WAIT, async {
        loop {
            let t = agent.get(id).unwrap();
            if t.phase == wanted {
                return t;
            }
            assert!(
                !["failed", "completed"].contains(&t.phase.as_str()),
                "unexpected task: {t:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
fn ack(agent: &AgentManager, t: &TerminalTaskSnapshot) {
    let a = t.action.as_ref().unwrap();
    agent
        .acknowledge(AckTerminalAction {
            task_id: t.task_id.clone(),
            run_id: t.run_id.clone(),
            action_id: a.action_id.clone(),
            display_revision: a.display_revision.clone(),
        })
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_model_command_waits_for_finished_stream_and_visible_preview_then_same_pty() {
    let ssh_server = LoopbackServer::start(Behavior::default()).await;
    let (http, base) = listener().await;
    let f = Fixture::new(&ssh_server, &base).await;
    // Historical manual input predates terminal-context consent; do not upload it.
    f.ssh
        .write(&f.session, &STANDARD.encode(b"PRIVATE-HISTORICAL-INPUT\r"))
        .await
        .unwrap();
    ssh_server
        .wait_for("manual input", |t| t.input.len() == 1)
        .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let (partial_tx, partial_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let responder = tokio::spawn(async move {
        let (mut socket, _) = http.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let first = chunk(
            serde_json::json!({"role":"assistant","content":"I will inspect the current host.","tool_calls":[{"index":0,"id":"one","type":"function","function":{"name":"terminal_action","arguments":"{\"command_text\":\"pwd\"}"}}]}),
            serde_json::Value::Null,
        );
        let last =
            chunk(serde_json::json!({}), serde_json::json!("tool_calls")) + "data: [DONE]\n\n";
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",first.len()+last.len(),first).as_bytes()).await.unwrap();
        partial_tx.send(()).unwrap();
        release_rx.await.unwrap();
        socket.write_all(last.as_bytes()).await.unwrap();
        request
    });
    let t = f.start();
    partial_rx.await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(f.agent.get(&t.task_id).unwrap().action.is_none());
    assert_eq!(
        f.agent.get(&t.task_id).unwrap().assistant_text,
        "I will inspect the current host."
    );
    assert_eq!(ssh_server.trace().input.len(), 1);
    release_tx.send(()).unwrap();
    let prepared = phase(&f.agent, &t.task_id, "awaiting_display").await;
    assert_eq!(ssh_server.trace().input.len(), 1);
    assert_eq!(prepared.action.as_ref().unwrap().command_text, "pwd");
    assert!(!prepared.action.as_ref().unwrap().displayed);
    ack(&f.agent, &prepared);
    let sent = phase(&f.agent, &t.task_id, "needs_user").await;
    assert_eq!(sent.remote_outcome, "unknown");
    assert!(!sent.terminal_ready);
    assert_eq!(sent.action.unwrap().delivery, "write_accepted");
    let trace = ssh_server
        .wait_for("AI input", |t| t.input.len() == 2)
        .await;
    assert_eq!(trace.input[1].1, b"pwd\r");
    assert_eq!(trace.input[0].0, trace.input[1].0);
    assert_eq!(trace.channels.len(), 1);
    assert_eq!(trace.exec_requests, 0);
    let request = responder.await.unwrap();
    let body = request.split("\r\n\r\n").nth(1).unwrap();
    assert!(!body.contains("PRIVATE-HISTORICAL-INPUT"));
    assert!(!body.contains(PASSWORD));
    assert!(!body.contains("synthetic-model-credential"));
    assert!(body.contains("terminal_observation_untrusted"));
    let c = f.chats.create_chat("Second task").unwrap();
    assert_eq!(
        f.agent.start(f.request(&c.chat_id)).unwrap_err().code,
        "AI_OWNER_BUSY"
    );
    f.agent
        .cancel(TaskRunRequest {
            task_id: t.task_id.clone(),
            run_id: t.run_id.clone(),
        })
        .unwrap();
    assert_eq!(
        f.agent.start(f.request(&c.chat_id)).unwrap_err().code,
        "TERMINAL_OUTCOME_UNKNOWN"
    );
    let resolved = f
        .agent
        .continue_task(ContinueTerminalTask {
            task_id: t.task_id,
            run_id: t.run_id,
            readiness_confirmed: true,
        })
        .unwrap();
    assert!(resolved.terminal_ready);
    assert_eq!(resolved.remote_outcome, "unknown");
    assert_eq!(resolved.phase, "cancelled");
    assert_eq!(
        trace.input.len(),
        2,
        "cancel/readiness must never inject Ctrl+C or Enter"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn destructive_command_requires_exact_single_use_approval_and_same_chat_is_reserved() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let (http, base) = listener().await;
    let f = Fixture::new(&server, &base).await;
    tokio::spawn(async move {
        let (mut socket, _) = http.accept().await.unwrap();
        read_request(&mut socket).await;
        reply(
            &mut socket,
            &tool_body(
                "terminal_action",
                r#"{"command_text":"rm -rf /tmp/fixture"}"#,
                "tool_calls",
            ),
        )
        .await;
    });
    let t = f.start();
    let a = phase(&f.agent, &t.task_id, "awaiting_display").await;
    assert_eq!(a.action.as_ref().unwrap().policy, "require_approval");
    ack(&f.agent, &a);
    let t = phase(&f.agent, &t.task_id, "waiting_approval").await;
    assert!(server.trace().input.is_empty());
    let config = f.config.status();
    f.chats
        .confirm_destination(
            config.revision.as_ref().unwrap(),
            config.destination.as_ref().unwrap(),
        )
        .unwrap();
    assert_eq!(
        f.chats
            .send_message(&t.chat_id, "race text")
            .unwrap_err()
            .code,
        "CHAT_BUSY"
    );
    let a = t.action.as_ref().unwrap();
    let request = || DecideTerminalAction {
        task_id: t.task_id.clone(),
        run_id: t.run_id.clone(),
        action_id: a.action_id.clone(),
        approval_id: a.approval_id.clone().unwrap(),
        decision: "approve".into(),
    };
    let mut bad = request();
    bad.approval_id = "model-cannot-grant-permission".into();
    assert!(f.agent.decide(bad).is_err());
    assert!(server.trace().input.is_empty());
    f.agent.decide(request()).unwrap();
    assert!(f.agent.decide(request()).is_err());
    phase(&f.agent, &t.task_id, "needs_user").await;
    let trace = server
        .wait_for("approved command", |t| !t.input.is_empty())
        .await;
    assert_eq!(trace.input[0].1, b"rm -rf /tmp/fixture\r");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_and_unknown_model_calls_never_prepare_or_write() {
    for (name, args, finish) in [
        ("terminal_action", r#"{"command_text":"pwd""#, "tool_calls"),
        (
            "terminal_action",
            r#"{"command_text":"pwd","command_text":"id"}"#,
            "tool_calls",
        ),
        ("send_user_input", r#"{"command_text":"pwd"}"#, "tool_calls"),
        ("terminal_action", r#"{"command_text":"pwd"}"#, "length"),
    ] {
        let server = LoopbackServer::start(Behavior::default()).await;
        let (http, base) = listener().await;
        let f = Fixture::new(&server, &base).await;
        tokio::spawn(async move {
            let (mut socket, _) = http.accept().await.unwrap();
            read_request(&mut socket).await;
            reply(&mut socket, &tool_body(name, args, finish)).await;
        });
        let t = f.start();
        let failed = phase(&f.agent, &t.task_id, "failed").await;
        assert!(failed.action.is_none());
        assert!(!failed.owner);
        assert!(server.trace().input.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_ai_cancellation_is_revalidated_after_real_ssh_backpressure() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let (http, base) = listener().await;
    let f = Fixture::new(&server, &base).await;
    tokio::spawn(async move {
        for command in ["pwd", "id"] {
            let (mut socket, _) = http.accept().await.unwrap();
            read_request(&mut socket).await;
            reply(
                &mut socket,
                &tool_body(
                    "terminal_action",
                    &serde_json::json!({"command_text":command}).to_string(),
                    "tool_calls",
                ),
            )
            .await;
        }
    });
    let first = f.start();
    let first = phase(&f.agent, &first.task_id, "awaiting_display").await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    f.auto_ack.store(false, Ordering::SeqCst);
    let before = f.output_bytes.load(Ordering::SeqCst);
    server.send_output(vec![b'x'; 256 * 1024 + 100]).await;
    tokio::time::timeout(WAIT, async {
        while f.output_bytes.load(Ordering::SeqCst) < before + 256 * 1024 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    ack(&f.agent, &first);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        f.agent
            .get(&first.task_id)
            .unwrap()
            .action
            .unwrap()
            .delivery,
        "queued"
    );
    let stopped = f
        .agent
        .cancel(TaskRunRequest {
            task_id: first.task_id.clone(),
            run_id: first.run_id.clone(),
        })
        .unwrap();
    assert_eq!(stopped.remote_outcome, "not_sent");
    assert!(!stopped.owner);
    let second = f.start();
    let second = phase(&f.agent, &second.task_id, "awaiting_display").await;
    ack(&f.agent, &second);
    f.auto_ack.store(true, Ordering::SeqCst);
    f.ssh
        .ack_output(&f.session, &f.output_seq.load(Ordering::SeqCst).to_string())
        .unwrap();
    phase(&f.agent, &second.task_id, "needs_user").await;
    let trace = server
        .wait_for("second queued action", |t| !t.input.is_empty())
        .await;
    assert_eq!(trace.input.len(), 1);
    assert_eq!(trace.input[0].1, b"id\r");
    tokio::time::timeout(WAIT, async {
        loop {
            let t = f.agent.get(&first.task_id).unwrap();
            if t.action.as_ref().unwrap().delivery == "not_sent" {
                assert_eq!(t.phase, "cancelled");
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_continue_sends_only_sanitized_new_output_and_preserves_unknown_result() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let (http, base) = listener().await;
    let f = Fixture::new(&server, &base).await;
    let responder = tokio::spawn(async move {
        let (mut socket, _) = http.accept().await.unwrap();
        read_request(&mut socket).await;
        reply(
            &mut socket,
            &tool_body("terminal_action", r#"{"command_text":"pwd"}"#, "tool_calls"),
        )
        .await;
        let (mut socket, _) = http.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let body = chunk(
            serde_json::json!({"role":"assistant","content":"Inspect the terminal to verify the result."}),
            serde_json::Value::Null,
        ) + &chunk(serde_json::json!({}), serde_json::json!("stop"))
            + "data: [DONE]\n\n";
        reply(&mut socket, &body).await;
        request
    });
    let t = f.start();
    let p = phase(&f.agent, &t.task_id, "awaiting_display").await;
    ack(&f.agent, &p);
    phase(&f.agent, &t.task_id, "needs_user").await;
    let before = f.output_bytes.load(Ordering::SeqCst);
    let output = format!(
        "\x1b]52;c;HIDDEN-CLIPBOARD\x07visible {} synthetic-model-credential \x1b[31mred\x1b[0m\n",
        PASSWORD
    );
    server.send_output(output.as_bytes().to_vec()).await;
    tokio::time::timeout(WAIT, async {
        while f.output_bytes.load(Ordering::SeqCst) < before + output.len() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    f.agent
        .continue_task(ContinueTerminalTask {
            task_id: t.task_id.clone(),
            run_id: t.run_id.clone(),
            readiness_confirmed: true,
        })
        .unwrap();
    let done = phase(&f.agent, &t.task_id, "completed").await;
    assert_eq!(done.remote_outcome, "unknown");
    assert_eq!(done.actions.len(), 1);
    assert!(done.terminal_ready);
    assert!(!done.owner);
    let request = responder.await.unwrap();
    let body = request.split("\r\n\r\n").nth(1).unwrap();
    assert!(!body.contains(PASSWORD));
    assert!(!body.contains("synthetic-model-credential"));
    assert!(!body.contains("HIDDEN-CLIPBOARD"));
    assert!(body.contains("[REDACTED]"));
    assert!(body.contains("visible"));
    assert!(body.contains("unknown"));
    assert_eq!(server.trace().input.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_during_model_response_keeps_old_run_secret_redactor() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let (http, base) = listener().await;
    let f = Fixture::new(&server, &base).await;
    let (received_tx, received_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let response = tokio::spawn(async move {
        let (mut socket, _) = http.accept().await.unwrap();
        read_request(&mut socket).await;
        received_tx.send(()).unwrap();
        release_rx.await.unwrap();
        let text = format!("echo {} synthetic-model-credential done", PASSWORD);
        let body = chunk(
            serde_json::json!({"role":"assistant","content":text}),
            serde_json::Value::Null,
        ) + &chunk(serde_json::json!({}), serde_json::json!("stop"))
            + "data: [DONE]\n\n";
        reply(&mut socket, &body).await;
    });
    let t = f.start();
    received_rx.await.unwrap();
    f.ssh.disconnect(&f.session).unwrap();
    tokio::time::timeout(WAIT, async {
        while f.ssh.snapshot().state != SessionState::Disconnected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let new = f
        .ssh
        .connect(ConnectRequest {
            host: "127.0.0.1".into(),
            port: server.addr.port(),
            username: USER.into(),
            auth: Authentication::Password {
                password: Secret::new(PASSWORD.into()),
            },
            session_only: true,
            size: TerminalSize { cols: 80, rows: 24 },
        })
        .unwrap()
        .session
        .unwrap();
    assert_ne!(new, f.session);
    tokio::time::timeout(WAIT, async {
        while f.ssh.snapshot().state != SessionState::Connected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    release_tx.send(()).unwrap();
    let done = phase(&f.agent, &t.task_id, "completed").await;
    assert!(!done.assistant_text.contains(PASSWORD));
    assert!(!done.assistant_text.contains("synthetic-model-credential"));
    assert!(done.assistant_text.contains("[REDACTED]"));
    assert!(server.trace().input.is_empty());
    response.await.unwrap();
    f.ssh.disconnect(&new).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_secret_file_value_only_reaches_approved_visible_pty_and_never_model() {
    use xyz_core::{
        agent::{EnsureTaskSecrets, PrepareSecretInput},
        storage::task_credentials::SecretFieldRequest,
        task_secrets::TaskSecrets,
    };
    let ssh_server = LoopbackServer::start(Behavior::default()).await;
    let (http, base) = listener().await;
    let mut f = Fixture::new(&ssh_server, &base).await;
    let secrets = Arc::new(TaskSecrets::open(&f.directory.join("task-storage")).unwrap());
    f.agent = f.agent.clone().with_task_secrets(secrets);
    let responder = tokio::spawn(async move {
        let (mut socket, _) = http.accept().await.unwrap();
        let first = read_request(&mut socket).await;
        reply(&mut socket, &tool_body("task_secret_request", r#"{"fields":[{"name":"service_password","required":true}],"purpose":"Synthetic remote password prompt"}"#, "tool_calls")).await;
        let (mut socket, _) = http.accept().await.unwrap();
        let second = read_request(&mut socket).await;
        let body = chunk(
            serde_json::json!({"role":"assistant","content":"The remote outcome remains unknown."}),
            serde_json::Value::Null,
        ) + &chunk(serde_json::json!({}), serde_json::json!("stop"))
            + "data: [DONE]\n\n";
        reply(&mut socket, &body).await;
        (first, second)
    });
    let started = f.start();
    let requested = phase(&f.agent, &started.task_id, "needs_secrets").await;
    assert_eq!(
        requested.secret_request.as_ref().unwrap().fields[0].name,
        "service_password"
    );
    let view = f
        .agent
        .ensure_task_secrets(EnsureTaskSecrets {
            task_id: started.task_id.clone(),
            run_id: started.run_id.clone(),
            fields: vec![SecretFieldRequest {
                name: "service_password".into(),
                required: true,
            }],
            expected_snapshot_id: None,
        })
        .unwrap();
    let synthetic = "only-fixture-secret-83927";
    fs::write(&view.local_edit_path, format!("schema_version = 1\n[task]\nid = \"{}\"\n[secrets.service_password]\nrequired = true\nvalue = \"{synthetic}\"\n", started.task_id)).unwrap();
    let status = f
        .agent
        .reload_task_secrets(TaskRunRequest {
            task_id: started.task_id.clone(),
            run_id: started.run_id.clone(),
        })
        .unwrap();
    assert!(status.fields[0].present);
    assert!(!serde_json::to_string(&status).unwrap().contains(synthetic));
    let action = f
        .agent
        .prepare_secret_input(PrepareSecretInput {
            task_id: started.task_id.clone(),
            run_id: started.run_id.clone(),
            field_name: "service_password".into(),
            purpose: "Synthetic remote password prompt".into(),
            prompt_confirmed: true,
        })
        .unwrap();
    assert!(!serde_json::to_string(&action).unwrap().contains(synthetic));
    assert!(ssh_server.trace().input.is_empty());
    ack(&f.agent, &action);
    let waiting = phase(&f.agent, &started.task_id, "waiting_approval").await;
    let a = waiting.action.as_ref().unwrap();
    f.agent
        .decide(DecideTerminalAction {
            task_id: started.task_id.clone(),
            run_id: started.run_id.clone(),
            action_id: a.action_id.clone(),
            approval_id: a.approval_id.clone().unwrap(),
            decision: "approve".into(),
        })
        .unwrap();
    ssh_server
        .wait_for("approved secret on visible PTY", |trace| {
            trace.input.len() == 1
        })
        .await;
    assert_eq!(
        ssh_server.trace().input[0].1,
        format!("{synthetic}\r").as_bytes()
    );
    phase(&f.agent, &started.task_id, "needs_user").await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    f.agent
        .continue_task(ContinueTerminalTask {
            task_id: started.task_id.clone(),
            run_id: started.run_id.clone(),
            readiness_confirmed: true,
        })
        .unwrap();
    let (first, second) = responder.await.unwrap();
    assert!(!first.contains(synthetic));
    assert!(!second.contains(synthetic));
    assert!(!second.contains(&view.local_edit_path));
    assert!(second.contains("service_password"));
    assert!(second.contains("[REDACTED]"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edited_secret_after_preview_fails_not_sent_and_releases_the_task() {
    use xyz_core::{
        agent::{EnsureTaskSecrets, PrepareSecretInput},
        storage::task_credentials::SecretFieldRequest,
        task_secrets::TaskSecrets,
    };
    let server = LoopbackServer::start(Behavior::default()).await;
    let (http, base) = listener().await;
    let mut f = Fixture::new(&server, &base).await;
    let secrets = Arc::new(TaskSecrets::open(&f.directory.join("task-storage")).unwrap());
    f.agent = f.agent.clone().with_task_secrets(secrets);
    let responder = tokio::spawn(async move {
        let (mut socket, _) = http.accept().await.unwrap();
        read_request(&mut socket).await;
        reply(
            &mut socket,
            &tool_body(
                "task_secret_request",
                r#"{"fields":[{"name":"password","required":true}],"purpose":"Fixture prompt"}"#,
                "tool_calls",
            ),
        )
        .await;
    });
    let task = f.start();
    phase(&f.agent, &task.task_id, "needs_secrets").await;
    let view = f
        .agent
        .ensure_task_secrets(EnsureTaskSecrets {
            task_id: task.task_id.clone(),
            run_id: task.run_id.clone(),
            fields: vec![SecretFieldRequest {
                name: "password".into(),
                required: true,
            }],
            expected_snapshot_id: None,
        })
        .unwrap();
    let content = |value: &str| {
        format!("schema_version=1\n[task]\nid='{}'\n[secrets.password]\nrequired=true\nvalue='{value}'\n",task.task_id)
    };
    fs::write(&view.local_edit_path, content("synthetic-approved")).unwrap();
    f.agent
        .reload_task_secrets(TaskRunRequest {
            task_id: task.task_id.clone(),
            run_id: task.run_id.clone(),
        })
        .unwrap();
    let preview = f
        .agent
        .prepare_secret_input(PrepareSecretInput {
            task_id: task.task_id.clone(),
            run_id: task.run_id.clone(),
            field_name: "password".into(),
            purpose: "Fixture prompt".into(),
            prompt_confirmed: true,
        })
        .unwrap();
    fs::write(
        &view.local_edit_path,
        content("synthetic-edited-after-preview"),
    )
    .unwrap();
    ack(&f.agent, &preview);
    let action = preview.action.unwrap();
    f.agent
        .decide(DecideTerminalAction {
            task_id: task.task_id.clone(),
            run_id: task.run_id.clone(),
            action_id: action.action_id,
            approval_id: action.approval_id.unwrap(),
            decision: "approve".into(),
        })
        .unwrap();
    let failed = phase(&f.agent, &task.task_id, "failed").await;
    assert!(!failed.owner);
    assert_eq!(failed.action.unwrap().delivery, "not_sent");
    assert_eq!(failed.remote_outcome, "not_sent");
    assert!(server.trace().input.is_empty());
    // A fresh run can reserve the same task; the failed approval is not stranded.
    let new_run = f.agent.start(f.request(&task.chat_id)).unwrap();
    f.agent
        .cancel(TaskRunRequest {
            task_id: new_run.task_id,
            run_id: new_run.run_id,
        })
        .unwrap();
    responder.await.unwrap();
}
