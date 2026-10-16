#![cfg(target_os = "linux")]
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use xyz_core::tasks::{ChatSnapshot, MessageState, TaskManager};

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
async fn sessionless_unified_conversation_replies_and_retains_context_without_tools() {
    use xyz_core::{
        agent::{AgentManager, ConversationTurn},
        model::ModelConfigManager,
        ssh::SessionManager,
    };
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, "");
    let config = ModelConfigManager::from_path(Ok(fixture.path.clone()));
    let chats = TaskManager::new(config.clone(), Arc::new(|_| true));
    consent(&chats);
    let agent = AgentManager::new(
        config.clone(),
        chats.clone(),
        SessionManager::new(Arc::new(|_| true)),
        Arc::new(|_| true),
    );
    let chat = chats.create_chat("Conversation").unwrap();
    let status = config.status();
    let model = tokio::spawn(async move {
        for (index, response) in ["Hello!", "We just greeted each other."].iter().enumerate() {
            let (mut socket, _) = server.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let body: serde_json::Value =
                serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert!(body
                .get("tools")
                .is_none_or(|tools| tools.as_array().is_some_and(Vec::is_empty)));
            assert!(request.contains("no command was sent or queued"));
            if index == 1 {
                assert!(request.contains("Hello!"));
            }
            reply(&mut socket, "200 OK", &sse_text(response, "stop")).await;
        }
    });
    for text in ["Hi", "What did we just say?"] {
        let response = agent
            .submit_conversation_turn(
                &chat.chat_id,
                text,
                status.revision.as_deref().unwrap(),
                status.destination.as_deref().unwrap(),
                None,
            )
            .unwrap();
        assert!(matches!(response, ConversationTurn::Chat { .. }));
        let result = finished(&chats, &chat.chat_id).await;
        assert_eq!(
            result.messages.last().unwrap().state,
            MessageState::Complete
        );
    }
    assert_eq!(chats.get_chat(&chat.chat_id).unwrap().messages.len(), 4);
    model.await.unwrap();
}

#[tokio::test]
async fn unified_submission_fences_stale_model_and_session_before_network() {
    use xyz_core::{
        agent::AgentManager,
        model::ModelConfigManager,
        ssh::{SessionManager, SessionRef},
    };
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, "");
    let config = ModelConfigManager::from_path(Ok(fixture.path.clone()));
    let chats = TaskManager::new(config.clone(), Arc::new(|_| true));
    consent(&chats);
    let agent = AgentManager::new(
        config.clone(),
        chats.clone(),
        SessionManager::new(Arc::new(|_| true)),
        Arc::new(|_| true),
    );
    let chat = chats.create_chat("Conversation").unwrap();
    let status = config.status();
    let error = agent
        .submit_conversation_turn(
            &chat.chat_id,
            "Hi",
            "stale",
            status.destination.as_deref().unwrap(),
            None,
        )
        .err()
        .unwrap();
    assert_eq!(error.code, "MODEL_CONFIG_CHANGED");
    let session = SessionRef {
        session_id: uuid::Uuid::new_v4().to_string(),
        generation: "1".into(),
    };
    let error = agent
        .submit_conversation_turn(
            &chat.chat_id,
            "Hi",
            status.revision.as_deref().unwrap(),
            status.destination.as_deref().unwrap(),
            Some(&session),
        )
        .err()
        .unwrap();
    assert_eq!(error.code, "SESSION_STALE");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), server.accept())
            .await
            .is_err()
    );
    assert!(chats.get_chat(&chat.chat_id).unwrap().messages.is_empty());
}

#[tokio::test]
async fn submission_uuid_reconciles_lost_response_without_duplicate_model_request() {
    use xyz_core::{
        agent::{AgentManager, ConversationTurn},
        model::ModelConfigManager,
        ssh::SessionManager,
    };
    let (server, base) = listener().await;
    let fixture = Fixture::new(&base, "");
    let config = ModelConfigManager::from_path(Ok(fixture.path.clone()));
    let chats = TaskManager::new(config.clone(), Arc::new(|_| true));
    consent(&chats);
    let agent = AgentManager::new(
        config.clone(),
        chats.clone(),
        SessionManager::new(Arc::new(|_| true)),
        Arc::new(|_| true),
    );
    let chat = chats.create_chat("Exactly one submission").unwrap();
    let status = config.status();
    let id = uuid::Uuid::new_v4().to_string();
    let submit = || {
        agent
            .submit_idempotent_conversation_turn(
                &id,
                &chat.chat_id,
                "Hello synthetic-private-key",
                status.revision.as_deref().unwrap(),
                status.destination.as_deref().unwrap(),
                None,
            )
            .unwrap()
    };
    let ConversationTurn::Chat { chat: first } = submit() else {
        panic!("expected chat")
    };
    let ConversationTurn::Chat { chat: duplicate } = submit() else {
        panic!("expected chat")
    };
    assert_eq!(first.active_run_id, duplicate.active_run_id);
    let ConversationTurn::Chat { chat: lookup } =
        agent.get_conversation_submission(&id).unwrap().unwrap()
    else {
        panic!("expected lookup")
    };
    assert_eq!(first.active_run_id, lookup.active_run_id);
    assert!(!lookup.messages[0].text.contains("synthetic-private-key"));
    let (mut socket, _) = server.accept().await.unwrap();
    let request = read_request(&mut socket).await;
    let body: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert!(!body.to_string().contains("synthetic-private-key"));
    reply(&mut socket, "200 OK", &sse_text("Hello!", "stop")).await;
    finished(&chats, &chat.chat_id).await;
    let after = submit();
    assert!(
        matches!(after,ConversationTurn::Chat{chat} if chat.active_run_id.is_none() && chat.messages.len()==2)
    );
    let conflict = agent
        .submit_idempotent_conversation_turn(
            &id,
            &chat.chat_id,
            "Different message",
            status.revision.as_deref().unwrap(),
            status.destination.as_deref().unwrap(),
            None,
        )
        .err()
        .unwrap();
    assert_eq!(conflict.code, "SUBMISSION_ID_CONFLICT");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), server.accept())
            .await
            .is_err()
    );
    assert!(agent
        .get_conversation_submission(&uuid::Uuid::new_v4().to_string())
        .unwrap()
        .is_none());
}
