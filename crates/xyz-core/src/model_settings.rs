//! Host-only model settings. No model tool can read or invoke this boundary.
//! The UI receives an opaque file version, never old key material or its hash.
use super::*;
use crate::storage::{private_files::PrivateDirectory, StorageError};
use futures::StreamExt;
use rig_core::{
    completion::{CompletionRequest, FinishReason},
    providers::openai::OpenAIConfig,
    streaming::{Item, PartKind, StreamEvent},
};
use std::time::Duration;
use zeroize::Zeroize;

#[derive(Deserialize)]
#[serde(transparent)]
pub struct ModelKeyText(String);
impl Drop for ModelKeyText {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelKeyUpdate {
    Keep,
    Replace { value: ModelKeyText },
    Clear,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveModelConfigRequest {
    pub expected_file_revision: String,
    pub base_url: String,
    pub model: String,
    pub key_update: ModelKeyUpdate,
    /// Explicit reuse decision bound to the exact normalized new destination.
    pub reuse_key_for_destination: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelTestRequest {
    pub test_id: String,
    pub file_revision: String,
    pub destination: String,
    pub model: String,
    /// Authorizes only authentication and the fixed minimal test prompt.
    pub consent: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct ModelTestResult {
    pub test_id: String,
    pub file_revision: String,
    pub destination: String,
    pub model: String,
    pub success: bool,
    pub error: Option<ApiError>,
}
fn storage_error(error: StorageError) -> ApiError {
    match error {
        StorageError::Conflict => ApiError::new("CONFIG_CONFLICT", "The selected model file changed. Reload it and review your edits before saving again."),
        StorageError::SaveUncertain => ApiError::new("CONFIG_SAVE_UNCERTAIN", "The model save result is uncertain. Reload and check the selected file; do not retry this save or test automatically."),
        StorageError::Busy => ApiError::new("CONFIG_BUSY", "Another instance is using this model file. Wait, then reload before saving."),
        StorageError::InvalidPath => ApiError::new("MODEL_FILE_SECURITY", "Use an absolute local configuration path with a simple filename, in a private directory. The selected path was not changed."),
        StorageError::UnsafeFile | StorageError::UnsupportedFilesystem => ApiError::new("MODEL_FILE_SECURITY", "Use a private local model file and directory owned by this user, without symbolic links or hard links. On Linux use file mode 0600 and directory mode 0700."),
        StorageError::TooLarge => ApiError::new("MODEL_FILE_SECURITY", "The model configuration exceeds the 65536-byte limit."),
        _ => ApiError::new("MODEL_FILE_UNAVAILABLE", "The selected model configuration cannot be accessed safely. Check its path and private-file permissions; no fallback path was used."),
    }
}
fn location(path: &Path) -> Result<(PrivateDirectory, &str), ApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| storage_error(StorageError::InvalidPath))?;
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| storage_error(StorageError::InvalidPath))?;
    let directory = PrivateDirectory::open_or_create(parent).map_err(storage_error)?;
    Ok((directory, name))
}
type Loaded = (Option<Zeroizing<Vec<u8>>>, Option<ModelSnapshot>);
pub(super) fn read_configuration(path: &Path) -> Result<Loaded, ApiError> {
    let (directory, name) = location(path)?;
    let bytes = directory
        .read(name, MAX_CONFIG_BYTES)
        .map_err(storage_error)?;
    let snapshot = bytes
        .as_ref()
        .map(|bytes| {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| invalid("The model configuration must be valid UTF-8."))?;
            parse_config(text)
        })
        .transpose()?;
    Ok((bytes, snapshot))
}
pub(super) fn apply_snapshot(state: &mut ConfigState, snapshot: ModelSnapshot) {
    state.status.key_present = !snapshot.api_key.is_empty();
    state.status.ready = state.status.key_present;
    state.status.base_url = Some(snapshot.destination.clone());
    state.status.destination = Some(snapshot.destination.clone());
    state.status.model = Some(snapshot.model.clone());
    state.status.error = None;
    state.snapshot = Some(Arc::new(snapshot));
}
fn empty_file() -> ConfigFile {
    ConfigFile {
        schema_version: 1,
        model: ModelFields {
            protocol: "openai_chat_completions".into(),
            base_url: String::new(),
            api_key: String::new(),
            name: String::new(),
        },
        capabilities: Capabilities::default(),
        request: RequestOptions::default(),
        transport: TransportOptions::default(),
    }
}
impl ModelConfigManager {
    /// Serialized with reload and confirmed against the exact loaded bytes under
    /// the stable cross-process private-file lock. Unknown data is never reset.
    pub fn save(&self, request: SaveModelConfigRequest) -> Result<ModelConfigStatus, ApiError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.status.file_revision.as_deref() != Some(request.expected_file_revision.as_str()) {
            return Err(storage_error(StorageError::Conflict));
        }
        let path = self.path.as_ref().map_err(Clone::clone)?;
        let (directory, filename) = location(path)?;
        let mut raw = match state.file_bytes.as_ref() {
            Some(bytes) => {
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| invalid("The model configuration must be valid UTF-8."))?;
                // Validate the whole existing schema before replacing any field.
                parse_config(text)?;
                toml::from_str::<ConfigFile>(text).map_err(|_| {
                    invalid("The model configuration is invalid; repair it before saving.")
                })?
            }
            None => empty_file(),
        };
        raw.model.base_url = request.base_url;
        raw.model.name = request.model;
        let keeping = matches!(request.key_update, ModelKeyUpdate::Keep);
        match request.key_update {
            ModelKeyUpdate::Keep => {}
            ModelKeyUpdate::Clear => raw.model.api_key.zeroize(),
            ModelKeyUpdate::Replace { mut value } => {
                if value.0.is_empty() {
                    return Err(invalid(
                        "Enter the new key, or use the separate clear-key action.",
                    ));
                }
                raw.model.api_key.zeroize();
                raw.model.api_key = std::mem::take(&mut value.0);
            }
        }
        // The serialized buffer is secret-bearing and cleared on every exit.
        let encoded = Zeroizing::new(
            toml::to_string(&raw)
                .map_err(|_| invalid("The model configuration could not be encoded."))?,
        );
        if encoded.len() > MAX_CONFIG_BYTES {
            return Err(storage_error(StorageError::TooLarge));
        }
        let snapshot = parse_config(&encoded)?;
        if keeping
            && state.status.key_present
            && state.status.destination.as_deref() != Some(snapshot.destination.as_str())
            && request.reuse_key_for_destination.as_deref() != Some(snapshot.destination.as_str())
        {
            return Err(ApiError::new("MODEL_KEY_REUSE_REQUIRED", "This destination changed. Enter a new key, clear the key, or explicitly allow the existing key for this exact destination before saving."));
        }
        let bytes = Zeroizing::new(encoded.as_bytes().to_vec());
        let result = directory.update(filename, MAX_CONFIG_BYTES, |current| {
            if current != state.file_bytes.as_ref().map(|v| v.as_slice()) {
                return Err(StorageError::Conflict);
            }
            Ok(((), Some(Zeroizing::new(bytes.to_vec()))))
        });
        if let Err(error) = result {
            let error = storage_error(error);
            if matches!(
                error.code.as_str(),
                "CONFIG_CONFLICT" | "CONFIG_SAVE_UNCERTAIN"
            ) {
                // No further save/test/send until a definite reread. Existing
                // requests already own immutable snapshots and continue safely.
                state.revision += 1;
                state.status.revision = Some(state.revision.to_string());
                state.status.ready = false;
                state.status.consented = false;
                state.status.file_revision = None;
                state.status.error = Some(error.clone());
                state.snapshot = None;
                state.file_bytes = None;
            }
            return Err(error);
        }
        state.revision += 1;
        state.status.revision = Some(state.revision.to_string());
        state.status.file_revision = Some(uuid::Uuid::new_v4().to_string());
        state.status.consented = false;
        state.file_bytes = Some(bytes);
        apply_snapshot(&mut state, snapshot);
        Ok(state.status.clone())
    }
    pub fn cancel_test(&self, test_id: &str) -> Result<(), ApiError> {
        if uuid::Uuid::parse_str(test_id).is_err() {
            return Err(invalid("The model test identifier is invalid."));
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((id, sender)) = &state.active_test {
            if id == test_id {
                let _ = sender.send(true);
                return Ok(());
            }
        }
        // IPC cancellation may arrive before its start. Keep a small bounded
        // tombstone so that a cancelled attempt cannot subsequently transmit.
        if !state.cancelled_tests.iter().any(|id| id == test_id) {
            if state.cancelled_tests.len() >= 32 {
                state.cancelled_tests.pop_front();
            }
            state.cancelled_tests.push_back(test_id.into());
        }
        Ok(())
    }
    pub async fn test_saved(&self, request: ModelTestRequest) -> Result<ModelTestResult, ApiError> {
        let (snapshot, mut receiver) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !request.consent
                || !state.status.ready
                || state.status.file_revision.as_deref() != Some(request.file_revision.as_str())
                || state.status.destination.as_deref() != Some(request.destination.as_str())
                || state.status.model.as_deref() != Some(request.model.as_str())
                || uuid::Uuid::parse_str(&request.test_id).is_err()
            {
                return Err(ApiError::new("MODEL_TEST_CONFIRMATION_REQUIRED", "Review and confirm the exact saved destination, model, and minimal test data before testing."));
            }
            if let Some(index) = state
                .cancelled_tests
                .iter()
                .position(|id| id == &request.test_id)
            {
                state.cancelled_tests.remove(index);
                return Ok(ModelTestResult {
                    test_id: request.test_id,
                    file_revision: request.file_revision,
                    destination: request.destination,
                    model: request.model,
                    success: false,
                    error: Some(ApiError::new(
                        "MODEL_CANCELLED",
                        "The model settings test was cancelled before sending.",
                    )),
                });
            }
            if state.active_test.is_some() {
                return Err(ApiError::new(
                    "MODEL_TEST_BUSY",
                    "A model settings test is already running. Wait or cancel it first.",
                ));
            }
            let path = self.path.as_ref().map_err(Clone::clone)?;
            let (directory, filename) = location(path)?;
            let current = directory
                .read(filename, MAX_CONFIG_BYTES)
                .map_err(storage_error)?;
            if current.as_deref() != state.file_bytes.as_deref() {
                return Err(storage_error(StorageError::Conflict));
            }
            let snapshot = state
                .snapshot
                .clone()
                .ok_or_else(|| invalid("Save a complete model configuration before testing."))?;
            let (sender, receiver) = tokio::sync::watch::channel(false);
            state.active_test = Some((request.test_id.clone(), sender));
            (snapshot, receiver)
        };
        let outcome = tokio::select! {
            biased;
            _ = receiver.changed() => Err(ApiError::new("MODEL_CANCELLED", "The model settings test was cancelled without retry.")),
            result = tokio::time::timeout(Duration::from_secs(snapshot.request.request_timeout_seconds), stream_test(snapshot)) => result.unwrap_or_else(|_| Err(ApiError::new("MODEL_TIMEOUT", "The model settings test timed out without retry."))),
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .active_test
            .as_ref()
            .is_some_and(|(id, _)| id == &request.test_id)
        {
            state.active_test = None;
        }
        Ok(ModelTestResult {
            test_id: request.test_id,
            file_revision: request.file_revision,
            destination: request.destination,
            model: request.model,
            success: outcome.is_ok(),
            error: outcome.err(),
        })
    }
}
async fn stream_test(config: Arc<ModelSnapshot>) -> Result<(), ApiError> {
    let transport = crate::model_transport::build_transport(
        &config.destination,
        Duration::from_secs(config.request.connect_timeout_seconds),
        Duration::from_secs(config.request.request_timeout_seconds),
        Duration::from_secs(config.request.stream_idle_timeout_seconds),
    )?;
    let failures = transport.clone();
    let safe_failure = || {
        failures.safe_error().unwrap_or_else(|| ApiError::new("MODEL_TEST_FAILED", "The provider did not complete the streaming chat test. Check this saved configuration and service availability. No automatic retry occurs."))
    };
    let model = OpenAIConfig::new(config.api_key.as_str())
        .with_base_url(&config.destination)
        .connect(transport)
        .chat(&config.model);
    // No history, terminal context, preamble, tool declarations, or user draft.
    let mut stream = model
        .stream(CompletionRequest::new("Reply with OK.").max_tokens(16))
        .map_err(|_| safe_failure())?;
    let mut output_bytes = 0usize;
    while let Some(item) = stream.next().await {
        match item.map_err(|_| safe_failure())? {
            Item::Event(StreamEvent::Text { text, .. }) => {
                output_bytes += text.len();
                if output_bytes > 4096 { return Err(ApiError::new("MODEL_OUTPUT_LIMIT", "The minimal model test exceeded its response limit.")); }
            }
            Item::Event(StreamEvent::Start { kind: PartKind::ToolCall, .. }) | Item::Event(StreamEvent::Arguments { .. }) => return Err(ApiError::new("MODEL_TOOLS_UNAVAILABLE", "The test returned a tool call. Only streaming text is tested; no tool was executed.")),
            _ => {},
        }
    }
    let response = stream.finish().await.map_err(|_| safe_failure())?;
    if response
        .choice
        .iter()
        .any(|part| matches!(part, rig_core::message::AssistantContent::ToolCall(_)))
    {
        return Err(safe_failure());
    }
    match response.finish_reason() {
        Some(FinishReason::Stop) if output_bytes > 0 => Ok(()),
        _ => Err(safe_failure()),
    }
}

#[cfg(test)]
#[path = "model_settings_tests.rs"]
mod tests;
