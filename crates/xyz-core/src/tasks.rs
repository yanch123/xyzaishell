//! Ephemeral, backend-owned chat transcripts. M2 exposes no executable tools,
//! terminal context, local files, SSH credentials or cross-chat history.
use crate::{
    model::{ModelConfigManager, ModelConfigStatus, ModelSnapshot},
    ssh::ApiError,
};
use futures::StreamExt;
use rig_core::{
    completion::{CompletionRequest, FinishReason},
    message::Message,
    providers::openai::OpenAIConfig,
    streaming::{Item, PartKind, StreamEvent},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{watch, Semaphore};
use uuid::Uuid;

pub const MAX_CHATS: usize = 32;
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_ASSISTANT_BYTES: usize = 64 * 1024;
pub const MAX_TRANSCRIPT_BYTES: usize = 256 * 1024;
pub const MAX_MESSAGES: usize = 128;
const MAX_ACTIVE_RUNS: usize = 4;
const MAX_STREAM_SNAPSHOTS: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    Complete,
    Streaming,
    Cancelled,
    Error,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: MessageRole,
    pub text: String,
    pub run_id: String,
    pub state: MessageState,
    pub error: Option<ApiError>,
    pub destination: Option<String>,
    pub model: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatSnapshot {
    pub chat_id: String,
    pub task_id: String,
    pub title: String,
    pub messages: Vec<ChatMessage>,
    pub active_run_id: Option<String>,
    pub revision: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelEvent {
    ChatUpdated { chat: ChatSnapshot },
    ConfigUpdated { config: ModelConfigStatus },
}
pub type ModelEventSink = Arc<dyn Fn(ModelEvent) -> bool + Send + Sync>;
struct ChatRecord {
    snapshot: ChatSnapshot,
    revision: u64,
    cancel: Option<watch::Sender<bool>>,
    terminal_run: Option<String>,
}
impl ChatRecord {
    fn changed(&mut self) {
        self.revision += 1;
        self.snapshot.revision = self.revision.to_string();
    }
}
#[derive(Clone)]
pub struct TaskManager {
    config: ModelConfigManager,
    chats: Arc<Mutex<BTreeMap<String, ChatRecord>>>,
    permits: Arc<Semaphore>,
    events: ModelEventSink,
}
impl TaskManager {
    pub fn new(config: ModelConfigManager, events: ModelEventSink) -> Self {
        Self {
            config,
            chats: Arc::new(Mutex::new(BTreeMap::new())),
            permits: Arc::new(Semaphore::new(MAX_ACTIVE_RUNS)),
            events,
        }
    }
    pub(crate) fn reserve_terminal(
        &self,
        chat_id: &str,
        run_id: &str,
    ) -> Result<ChatSnapshot, ApiError> {
        let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        let record = chats.get_mut(chat_id).ok_or_else(chat_missing)?;
        if record.snapshot.active_run_id.is_some() || record.terminal_run.is_some() {
            return Err(ApiError::new(
                "CHAT_BUSY",
                "Finish or stop this chat's current run before starting another.",
            ));
        }
        record.terminal_run = Some(run_id.into());
        Ok(record.snapshot.clone())
    }
    pub(crate) fn release_terminal(&self, chat_id: &str, run_id: &str) {
        let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(record) = chats.get_mut(chat_id) {
            if record.terminal_run.as_deref() == Some(run_id) {
                record.terminal_run = None;
            }
        }
    }
    pub fn config_status(&self) -> ModelConfigStatus {
        self.config.status()
    }
    pub fn reload_config(&self) -> ModelConfigStatus {
        let config = self.config.reload();
        (self.events)(ModelEvent::ConfigUpdated {
            config: config.clone(),
        });
        config
    }
    pub fn confirm_destination(
        &self,
        revision: &str,
        destination: &str,
    ) -> Result<ModelConfigStatus, ApiError> {
        let config = self.config.confirm_destination(revision, destination)?;
        (self.events)(ModelEvent::ConfigUpdated {
            config: config.clone(),
        });
        Ok(config)
    }
    pub fn list_chats(&self) -> Vec<ChatSnapshot> {
        self.chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|c| c.snapshot.clone())
            .collect()
    }
    pub fn get_chat(&self, chat_id: &str) -> Result<ChatSnapshot, ApiError> {
        self.chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(chat_id)
            .map(|c| c.snapshot.clone())
            .ok_or_else(chat_missing)
    }
    pub fn create_chat(&self, title: &str) -> Result<ChatSnapshot, ApiError> {
        self.create_chat_inner(title, true)
    }
    fn create_chat_inner(&self, title: &str, announce: bool) -> Result<ChatSnapshot, ApiError> {
        if title.trim().is_empty() || title.len() > 256 || title.chars().any(char::is_control) {
            return Err(ApiError::new(
                "CHAT_TITLE_INVALID",
                "Use a nonempty chat title of at most 256 bytes without control characters.",
            ));
        }
        let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        if chats.len() >= MAX_CHATS {
            return Err(ApiError::new(
                "CHAT_LIMIT",
                "This process supports at most 32 chats. Histories are memory-only in M2.",
            ));
        }
        let id = Uuid::new_v4().to_string();
        let chat = ChatSnapshot {
            chat_id: id.clone(),
            task_id: id.clone(),
            title: title.to_owned(),
            messages: vec![],
            active_run_id: None,
            revision: "1".into(),
        };
        chats.insert(
            id,
            ChatRecord {
                snapshot: chat.clone(),
                revision: 1,
                cancel: None,
                terminal_run: None,
            },
        );
        drop(chats);
        if announce {
            (self.events)(ModelEvent::ChatUpdated { chat: chat.clone() });
        }
        Ok(chat)
    }
    pub fn probe(&self) -> Result<ChatSnapshot, ApiError> {
        // Check consent before even creating the local diagnostic chat.
        self.config.authorized_snapshot()?;
        // Do not announce a diagnostic chat until a run was accepted. If a
        // capacity/consent race rejects it before sending, release the empty
        // slot. Never remove a chat with an accepted or ambiguous request.
        let chat = self.create_chat_inner("Connection test", false)?;
        let result = self.send_message(&chat.chat_id, "Reply with OK.");
        if result.is_err() {
            let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
            if chats.get(&chat.chat_id).is_some_and(|c| {
                c.snapshot.messages.is_empty() && c.snapshot.active_run_id.is_none()
            }) {
                chats.remove(&chat.chat_id);
            }
        }
        result
    }
    pub fn send_message(&self, chat_id: &str, text: &str) -> Result<ChatSnapshot, ApiError> {
        if text.trim().is_empty() || text.len() > MAX_MESSAGE_BYTES || text.contains('\0') {
            return Err(ApiError::new(
                "CHAT_INPUT_INVALID",
                "Enter 1 to 16384 UTF-8 bytes of chat text without NUL characters.",
            ));
        }
        // The immutable snapshot and destination consent are resolved before
        // any client/connection is constructed. Reload cannot redirect a run.
        let config = self.config.authorized_snapshot()?;
        let redactor = crate::observation::ObservationBuffer::new();
        redactor.add_known_secret(config.api_key.as_str())?;
        let safe_text = redactor.redact_prose(text);
        let text = safe_text.as_str();
        let permit = self.permits.clone().try_acquire_owned().map_err(|_| {
            ApiError::new(
                "MODEL_BUSY",
                "At most four chat requests may stream at once. Wait or cancel an active request.",
            )
        })?;
        let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        let record = chats.get_mut(chat_id).ok_or_else(chat_missing)?;
        if record.snapshot.active_run_id.is_some() || record.terminal_run.is_some() {
            return Err(ApiError::new(
                "CHAT_BUSY",
                "Wait for this chat response or cancel it before sending another message.",
            ));
        }
        let bytes: usize = record.snapshot.messages.iter().map(|m| m.text.len()).sum();
        if record.snapshot.messages.len() + 2 > MAX_MESSAGES
            || bytes + text.len() + MAX_ASSISTANT_BYTES > MAX_TRANSCRIPT_BYTES
        {
            return Err(ApiError::new("CHAT_CONTEXT_FULL", "This chat reached its bounded context limit. Create a new chat; history is never silently trimmed."));
        }
        let run_id = Uuid::new_v4().to_string();
        let history = record
            .snapshot
            .messages
            .iter()
            .filter(|m| !m.text.is_empty())
            .map(|m| match m.role {
                MessageRole::User => Message::user(m.text.clone()),
                MessageRole::Assistant => Message::assistant(m.text.clone()),
            })
            .collect::<Vec<_>>();
        record.snapshot.messages.push(ChatMessage {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::User,
            text: text.to_owned(),
            run_id: run_id.clone(),
            state: MessageState::Complete,
            error: None,
            destination: None,
            model: None,
        });
        record.snapshot.messages.push(ChatMessage {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::Assistant,
            text: String::new(),
            run_id: run_id.clone(),
            state: MessageState::Streaming,
            error: None,
            destination: Some(config.destination.clone()),
            model: Some(config.model.clone()),
        });
        let (cancel, receiver) = watch::channel(false);
        record.cancel = Some(cancel);
        record.snapshot.active_run_id = Some(run_id.clone());
        record.changed();
        let snapshot = record.snapshot.clone();
        drop(chats);
        if !(self.events)(ModelEvent::ChatUpdated {
            chat: snapshot.clone(),
        }) {
            self.finish_run(
                chat_id,
                &run_id,
                Err(ApiError::new(
                    "MODEL_DELIVERY_FAILED",
                    "The chat view is unavailable. The request was not sent.",
                )),
            );
            return self.get_chat(chat_id);
        }
        let this = self.clone();
        let chat_id = chat_id.to_owned();
        let text = text.to_owned();
        tokio::spawn(async move {
            let _permit = permit;
            let timeout = Duration::from_secs(config.request.request_timeout_seconds);
            let mut receiver = receiver;
            let result = tokio::select! {
                biased;
                _ = wait_cancelled(&mut receiver) => Err(cancelled()),
                result = tokio::time::timeout(timeout, this.stream_reply(&chat_id, &run_id, &text, history, config)) => result.unwrap_or_else(|_| Err(ApiError::new("MODEL_TIMEOUT", "The model request reached its deadline. It was stopped and will not be retried automatically."))),
            };
            this.finish_run(&chat_id, &run_id, result);
        });
        Ok(snapshot)
    }
    pub fn cancel_run(&self, chat_id: &str, run_id: &str) -> Result<ChatSnapshot, ApiError> {
        let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        let record = chats.get_mut(chat_id).ok_or_else(chat_missing)?;
        if record.snapshot.active_run_id.as_deref() != Some(run_id) {
            return Err(ApiError::new(
                "MODEL_RUN_STALE",
                "That chat run is no longer active. Refresh the chat before cancelling.",
            ));
        }
        if let Some(cancel) = record.cancel.take() {
            let _ = cancel.send(true);
        }
        record.snapshot.active_run_id = None;
        if let Some(message) = record.snapshot.messages.last_mut() {
            message.state = MessageState::Cancelled;
            message.error = None;
        }
        record.changed();
        let chat = record.snapshot.clone();
        drop(chats);
        (self.events)(ModelEvent::ChatUpdated { chat: chat.clone() });
        Ok(chat)
    }
    fn append_text(
        &self,
        chat_id: &str,
        run_id: &str,
        text: &str,
        emit: bool,
    ) -> Result<(), ApiError> {
        let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        let record = chats.get_mut(chat_id).ok_or_else(chat_missing)?;
        if record.snapshot.active_run_id.as_deref() != Some(run_id) {
            return Err(cancelled());
        }
        let message = record
            .snapshot
            .messages
            .last_mut()
            .ok_or_else(chat_missing)?;
        if text.len() > MAX_ASSISTANT_BYTES {
            return Err(ApiError::new("MODEL_OUTPUT_LIMIT", "The model response exceeded the 65536-byte text limit. Partial output is retained; nothing was executed."));
        }
        message.text.clear();
        message.text.push_str(text);
        record.changed();
        let snapshot = emit.then(|| record.snapshot.clone());
        drop(chats);
        if let Some(chat) = snapshot {
            if !(self.events)(ModelEvent::ChatUpdated { chat }) {
                return Err(ApiError::new(
                    "MODEL_DELIVERY_FAILED",
                    "The chat view stopped accepting updates. The model request was stopped.",
                ));
            }
        }
        Ok(())
    }
    fn finish_run(&self, chat_id: &str, run_id: &str, result: Result<(), ApiError>) {
        let mut chats = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        let Some(record) = chats.get_mut(chat_id) else {
            return;
        };
        // Completion/queued deltas for cancelled or replaced runs have no power.
        if record.snapshot.active_run_id.as_deref() != Some(run_id) {
            return;
        }
        record.snapshot.active_run_id = None;
        record.cancel = None;
        if let Some(message) = record.snapshot.messages.last_mut() {
            match result {
                Ok(()) => message.state = MessageState::Complete,
                Err(error) if error.code == "MODEL_CANCELLED" => {
                    message.state = MessageState::Cancelled;
                    message.error = None;
                }
                Err(error) => {
                    message.state = MessageState::Error;
                    message.error = Some(error);
                }
            }
        }
        record.changed();
        let chat = record.snapshot.clone();
        drop(chats);
        (self.events)(ModelEvent::ChatUpdated { chat });
    }
    async fn stream_reply(
        &self,
        chat_id: &str,
        run_id: &str,
        text: &str,
        history: Vec<Message>,
        config: Arc<ModelSnapshot>,
    ) -> Result<(), ApiError> {
        let transport = crate::model_transport::build_transport(
            &config.destination,
            Duration::from_secs(config.request.connect_timeout_seconds),
            Duration::from_secs(config.request.request_timeout_seconds),
            Duration::from_secs(config.request.stream_idle_timeout_seconds),
        )?;
        let failures = transport.clone();
        let safe_provider_error = || failures.safe_error().unwrap_or_else(provider_error);
        let model = OpenAIConfig::new(config.api_key.as_str())
            .with_base_url(&config.destination)
            .connect(transport)
            .chat(&config.model);
        let request = CompletionRequest::new(text).messages(history).preamble("You are xyzaishell's text-chat assistant. You have no terminal, file, credential, or execution tools in this milestone. Never claim to have run commands. Previous assistant text may be partial if its response was interrupted.").max_tokens(config.request.max_output_tokens);
        let mut stream = model.stream(request).map_err(|_| safe_provider_error())?;
        let redactor = crate::observation::ObservationBuffer::new();
        redactor.add_known_secret(config.api_key.as_str())?;
        let mut raw_output = zeroize::Zeroizing::new(String::new());
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let mut emitted = 0;
        let mut output_bytes = 0;
        loop {
            let item = tokio::time::timeout(Duration::from_secs(config.request.stream_idle_timeout_seconds), stream.next()).await.map_err(|_| ApiError::new("MODEL_IDLE_TIMEOUT", "The model stopped providing valid streaming data. The request was stopped without retry."))?;
            let Some(item) = item else {
                break;
            };
            match item.map_err(|_| safe_provider_error())? {
                Item::Event(StreamEvent::Text { text, .. }) => {
                    let emit = emitted < MAX_STREAM_SNAPSHOTS && last_emit.elapsed() >= Duration::from_millis(100);
                    output_bytes += text.len();
                    if output_bytes > MAX_ASSISTANT_BYTES { return Err(ApiError::new("MODEL_OUTPUT_LIMIT", "The model response exceeded the 65536-byte text limit. Partial output is retained; nothing was executed.")); }
                    raw_output.push_str(&text);
                    self.append_text(chat_id, run_id, &redactor.redact_prose(&raw_output), emit)?;
                    if emit { emitted += 1; last_emit = Instant::now(); }
                }
                Item::Event(StreamEvent::Start { kind: PartKind::ToolCall, .. }) | Item::Event(StreamEvent::Arguments { .. }) => return Err(ApiError::new("MODEL_TOOLS_UNAVAILABLE", "The provider returned a tool call. M2 is text-only; no command or tool was executed.")),
                _ => {}
            }
        }
        let response = stream.finish().await.map_err(|_| safe_provider_error())?;
        if response
            .choice
            .iter()
            .any(|part| matches!(part, rig_core::message::AssistantContent::ToolCall(_)))
        {
            return Err(ApiError::new(
                "MODEL_TOOLS_UNAVAILABLE",
                "The provider returned a tool call. No command or tool was executed.",
            ));
        }
        match response.finish_reason() {
            Some(FinishReason::Stop) if output_bytes > 0 => Ok(()),
            Some(FinishReason::Length) => Err(ApiError::new(
                "MODEL_OUTPUT_TRUNCATED",
                "The provider stopped at its output-token limit. The response is incomplete.",
            )),
            Some(FinishReason::ContentFilter) => Err(ApiError::new(
                "MODEL_CONTENT_FILTERED",
                "The provider filtered the response.",
            )),
            _ => Err(ApiError::new(
                "MODEL_RESPONSE_INCOMPLETE",
                "The provider did not return a complete, nonempty text response.",
            )),
        }
    }
}
async fn wait_cancelled(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}
fn cancelled() -> ApiError {
    ApiError::new("MODEL_CANCELLED", "The model request was cancelled.")
}
fn chat_missing() -> ApiError {
    ApiError::new(
        "CHAT_NOT_FOUND",
        "That chat is not available in this process.",
    )
}
fn provider_error() -> ApiError {
    ApiError::new("MODEL_REQUEST_FAILED", "The model request failed or the provider returned invalid, oversized, or interrupted data. Check the destination, credentials, and service availability. Provider diagnostics are hidden to protect private data; no automatic retry occurs.")
}
