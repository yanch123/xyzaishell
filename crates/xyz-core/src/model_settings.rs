//! Host-only model settings. No model tool can read or invoke this boundary.
//! The UI receives an opaque file version, never old key material or its hash.
use super::*;
use crate::storage::{private_files::PrivateDirectory, StorageError};
use futures::StreamExt;
use rig_core::{
    completion::{CompletionRequest, FinishReason, ToolDefinition},
    message::{AssistantContent, ToolName},
    providers::openai::OpenAIConfig,
    streaming::{Item, PartKind, StreamEvent},
};
use std::{collections::BTreeMap, time::Duration};
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
    /// Omitted by old clients: preserve. JSON null: remove explicit override.
    #[serde(default, deserialize_with = "optional_context_override")]
    pub context_window_tokens: Option<Option<u64>>,
    #[serde(default)]
    pub context_auto_compact: Option<bool>,
    #[serde(default)]
    pub context_auto_mode: Option<crate::context::CompactionMode>,
}
fn optional_context_override<'de, D>(deserializer: D) -> Result<Option<Option<u64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<u64>::deserialize(deserializer).map(Some)
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelTestKind {
    #[default]
    ChatConnectivity,
    TaskCapability,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelTestOutcome {
    Supported,
    Unsupported,
    Inconclusive,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelTestRequest {
    pub test_id: String,
    pub file_revision: String,
    pub destination: String,
    pub model: String,
    /// Older clients remain limited to the existing text-only test. Consent is
    /// for this exact kind, saved destination, model and configuration revision.
    #[serde(default)]
    pub kind: ModelTestKind,
    /// Authorizes only authentication and this kind's fixed synthetic test data.
    pub consent: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct ModelTestResult {
    pub test_id: String,
    pub file_revision: String,
    pub destination: String,
    pub model: String,
    pub kind: ModelTestKind,
    /// Evidence from this test attempt, not a guarantee about future requests.
    /// Transport errors and a model declining the probe are inconclusive.
    pub outcome: ModelTestOutcome,
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
    state.status.context_window_override_tokens = snapshot.request.context_window_tokens;
    state.status.context_auto_compact = snapshot.request.context_auto_compact;
    state.status.context_auto_mode = snapshot.request.context_auto_mode;
    // parse_config already validated this exact immutable request configuration.
    if let Ok(budget) = snapshot
        .request
        .context_budget()
        .resolve(crate::context::TurnPhase::ActiveTurn, 0)
    {
        state.status.context_window_tokens = budget.context_window_tokens;
        state.status.capacity_source = budget.capacity_source;
        state.status.output_reserve_tokens = Some(budget.output_reserve_tokens);
        state.status.safety_margin_tokens = budget.safety_margin_tokens;
        state.status.input_budget_tokens = budget.input_budget_tokens;
    }
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
        // Capacity overrides are scoped to one destination/model identity. Old
        // clients keep an override only while that identity stays unchanged.
        let identity_changed = raw.model.base_url.trim_end_matches('/')
            != request.base_url.trim_end_matches('/')
            || raw.model.name != request.model;
        raw.request.context_window_tokens = match request.context_window_tokens {
            Some(value) => value,
            None if identity_changed => None,
            None => raw.request.context_window_tokens,
        };
        if let Some(enabled) = request.context_auto_compact {
            raw.request.context_auto_compact = enabled;
        }
        if let Some(mode) = request.context_auto_mode {
            raw.request.context_auto_mode = mode;
        }
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
                state.status.context_window_tokens = None;
                state.status.context_window_override_tokens = None;
                state.status.capacity_source = crate::context::CapacitySource::Unavailable;
                state.status.output_reserve_tokens = None;
                state.status.safety_margin_tokens = None;
                state.status.input_budget_tokens = None;
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
                return Err(ApiError::new("MODEL_TEST_CONFIRMATION_REQUIRED", "Review and confirm the exact saved destination, model, test kind, and synthetic test data before testing."));
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
                    kind: request.kind,
                    outcome: ModelTestOutcome::Inconclusive,
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
            result = tokio::time::timeout(Duration::from_secs(snapshot.request.request_timeout_seconds), stream_test(snapshot, request.kind)) => result.unwrap_or_else(|_| Err(ApiError::new("MODEL_TIMEOUT", "The model settings test timed out without retry."))),
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
            kind: request.kind,
            outcome: match &outcome {
                Ok(()) => ModelTestOutcome::Supported,
                Err(error) if error.code == "MODEL_TASK_CAPABILITY_UNSUPPORTED" => {
                    ModelTestOutcome::Unsupported
                }
                Err(_) => ModelTestOutcome::Inconclusive,
            },
            success: outcome.is_ok(),
            error: outcome.err(),
        })
    }
}
const MOCK_TOOL_NAME: &str = "xyz_capability_probe";
const MOCK_PROBE_VALUE: &str = "xyz-task-capability";
const MOCK_TOOL_PROMPT: &str = "This is a synthetic compatibility test. Call xyz_capability_probe exactly once with the JSON argument {\"probe\":\"xyz-task-capability\"}. Do not call any other tool. The tool is inert and will not be executed.";
fn incompatible_tool_response() -> ApiError {
    ApiError::new("MODEL_TASK_CAPABILITY_UNSUPPORTED", "This completed test response is incompatible with the task adapter: it requires exactly one streamed mock-tool call with the requested name, exact JSON arguments, and a complete normalized tool-call finish reason. No tool was executed. Review the provider's streaming tool-call support or choose a compatible model, then explicitly test again.")
}
async fn stream_test(config: Arc<ModelSnapshot>, kind: ModelTestKind) -> Result<(), ApiError> {
    let transport = crate::model_transport::build_transport(
        &config.destination,
        Duration::from_secs(config.request.connect_timeout_seconds),
        Duration::from_secs(config.request.request_timeout_seconds),
        Duration::from_secs(config.request.stream_idle_timeout_seconds),
    )?;
    let failures = transport.clone();
    let safe_failure = || {
        failures.safe_error().unwrap_or_else(|| ApiError::new("MODEL_TEST_FAILED", "The provider did not complete a valid test stream, so compatibility is inconclusive. Check this saved configuration and service availability. No automatic retry occurs."))
    };
    let model = OpenAIConfig::new(config.api_key.as_str())
        .with_base_url(&config.destination)
        .connect(transport)
        .chat(&config.model);
    // No history, terminal context, user draft, execution adapter or SSH handle.
    // This inert declaration is serialized only; no Tool implementation exists.
    let request = match kind {
        ModelTestKind::ChatConnectivity => CompletionRequest::new("Reply with OK.").max_tokens(16),
        ModelTestKind::TaskCapability => {
            let tool = ToolDefinition::new(
                ToolName::new(MOCK_TOOL_NAME).map_err(|_| safe_failure())?,
                "Inert compatibility probe. The application validates the synthetic arguments only; it never executes this tool.",
                serde_json::json!({"type":"object","additionalProperties":false,"properties":{"probe":{"type":"string","enum":[MOCK_PROBE_VALUE]}},"required":["probe"]}),
            );
            CompletionRequest::new(MOCK_TOOL_PROMPT)
                .tools(vec![tool])
                .max_tokens(128)
        }
    };
    let mut stream = model.stream(request).map_err(|_| safe_failure())?;
    let mut output_bytes = 0usize;
    let mut raw_arguments: BTreeMap<usize, String> = BTreeMap::new();
    while let Some(item) = stream.next().await {
        match item.map_err(|_| safe_failure())? {
            Item::Event(StreamEvent::Text { text, .. }) => {
                output_bytes += text.len();
            }
            Item::Event(StreamEvent::Start { kind: PartKind::ToolCall, .. })
                if kind == ModelTestKind::ChatConnectivity => return Err(ApiError::new("MODEL_TOOLS_UNAVAILABLE", "The chat connectivity test returned a tool call. Only streaming text was requested; no tool was executed.")),
            Item::Event(StreamEvent::Arguments { part, json }) => {
                if kind == ModelTestKind::ChatConnectivity {
                    return Err(ApiError::new("MODEL_TOOLS_UNAVAILABLE", "The chat connectivity test returned a tool call. Only streaming text was requested; no tool was executed."));
                }
                output_bytes += json.len();
                if output_bytes <= 4096 && raw_arguments.len() < 16 {
                    raw_arguments.entry(part.index()).or_default().push_str(&json);
                } else {
                    return Err(ApiError::new("MODEL_OUTPUT_LIMIT", "The synthetic model test exceeded its response limit; compatibility is inconclusive."));
                }
            }
            _ => {},
        }
        if output_bytes > 4096 {
            return Err(ApiError::new("MODEL_OUTPUT_LIMIT", "The synthetic model test exceeded its response limit; compatibility is inconclusive."));
        }
    }
    let response = stream.finish().await.map_err(|_| safe_failure())?;
    if kind == ModelTestKind::TaskCapability {
        if !matches!(
            response.finish_reason(),
            Some(FinishReason::Stop | FinishReason::ToolCalls)
        ) {
            return Err(ApiError::new("MODEL_TASK_CAPABILITY_INCONCLUSIVE", "The synthetic response did not reach a complete stop or tool-call boundary. Task compatibility is inconclusive; no tool was executed and no automatic retry occurs."));
        }
        let calls: Vec<_> = response
            .choice
            .iter()
            .filter_map(|part| match part {
                AssistantContent::ToolCall(call) => Some(call),
                _ => None,
            })
            .collect();
        if calls.is_empty() && raw_arguments.is_empty() {
            return Err(ApiError::new("MODEL_TASK_CAPABILITY_INCONCLUSIVE", "The model did not return the requested mock tool call. A text reply does not establish task compatibility or prove that tool calling is unsupported. Check the provider's streaming tool-call support, then explicitly test again."));
        }
        if calls.len() != 1
            || raw_arguments.len() != 1
            // Match the real task adapter: Rig reconciles stop + a completed
            // tool call to ToolCalls. This is not raw-wire certification.
            || !matches!(response.finish_reason(), Some(FinishReason::ToolCalls))
            || response.choice.iter().any(|part| {
                !matches!(
                    part,
                    AssistantContent::Text(_) | AssistantContent::ToolCall(_)
                )
            })
        {
            return Err(incompatible_tool_response());
        }
        let call = calls[0];
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ProbeArguments {
            probe: String,
        }
        let arguments: ProbeArguments = serde_json::from_str(
            raw_arguments
                .values()
                .next()
                .ok_or_else(incompatible_tool_response)?,
        )
        .map_err(|_| incompatible_tool_response())?;
        if call.function.name.as_str() != MOCK_TOOL_NAME
            || call.function.invalid_arguments.is_some()
            || arguments.probe != MOCK_PROBE_VALUE
            || serde_json::to_value(&call.function.arguments)
                .map_err(|_| incompatible_tool_response())?
                != serde_json::json!({"probe":MOCK_PROBE_VALUE})
        {
            return Err(incompatible_tool_response());
        }
        return Ok(());
    }
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
