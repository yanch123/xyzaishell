//! Explicit, file-backed model configuration. No SDK environment fallbacks.
//! API keys never implement Debug/Serialize and never enter public status/errors.
use crate::ssh::ApiError;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

pub const MAX_CONFIG_BYTES: usize = 64 * 1024;

#[path = "model_settings.rs"]
mod settings;
pub use settings::{
    ModelKeyUpdate, ModelTestKind, ModelTestOutcome, ModelTestRequest, ModelTestResult,
    SaveModelConfigRequest,
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    schema_version: u32,
    model: ModelFields,
    #[serde(default)]
    capabilities: Capabilities,
    #[serde(default)]
    request: RequestOptions,
    #[serde(default)]
    transport: TransportOptions,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelFields {
    protocol: String,
    base_url: String,
    api_key: String,
    name: String,
}
impl Drop for ModelFields {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.api_key.zeroize();
    }
}
#[derive(Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct Capabilities {
    streaming: String,
    tool_calling: String,
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            streaming: "required".into(),
            tool_calling: "required".into(),
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RequestOptions {
    pub connect_timeout_seconds: u64,
    pub request_timeout_seconds: u64,
    pub stream_idle_timeout_seconds: u64,
    pub max_output_tokens: u64,
    /// An explicit user setting, never the resolved product default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    pub context_auto_compact: bool,
    pub context_auto_mode: crate::context::CompactionMode,
}
impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            connect_timeout_seconds: 10,
            request_timeout_seconds: 120,
            stream_idle_timeout_seconds: 30,
            max_output_tokens: 4096,
            context_window_tokens: None,
            context_auto_compact: true,
            context_auto_mode: crate::context::CompactionMode::Llm,
        }
    }
}
impl RequestOptions {
    pub(crate) fn context_budget(&self) -> crate::context::ContextBudgetConfig {
        crate::context::ContextBudgetConfig {
            context_window_override: self.context_window_tokens,
            verified_context_window: None,
            output_reserve_tokens: self.max_output_tokens,
            safety_margin_override: None,
        }
    }
}
#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct TransportOptions {
    allow_loopback_http: bool,
}

pub(crate) struct ModelSnapshot {
    pub destination: String,
    pub model: String,
    pub api_key: Zeroizing<String>,
    pub request: RequestOptions,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelConfigStatus {
    pub path: Option<String>,
    pub revision: Option<String>,
    /// Opaque file-CAS identity, never a digest of a credential.
    pub file_revision: Option<String>,
    pub base_url: Option<String>,
    pub key_present: bool,
    pub path_overridden: bool,
    pub ready: bool,
    pub destination: Option<String>,
    pub model: Option<String>,
    pub consented: bool,
    pub error: Option<ApiError>,
    pub warning: Option<String>,
    pub context_policy: String,
    pub persistence: String,
    /// Resolved budget and persisted override are deliberately separate.
    pub context_window_tokens: Option<u64>,
    pub context_window_override_tokens: Option<u64>,
    pub capacity_source: crate::context::CapacitySource,
    pub output_reserve_tokens: Option<u64>,
    pub safety_margin_tokens: Option<u64>,
    pub input_budget_tokens: Option<u64>,
    pub context_auto_compact: bool,
    pub context_auto_mode: crate::context::CompactionMode,
}
struct ConfigState {
    status: ModelConfigStatus,
    snapshot: Option<Arc<ModelSnapshot>>,
    revision: u64,
    file_bytes: Option<Zeroizing<Vec<u8>>>,
    active_test: Option<(String, tokio::sync::watch::Sender<bool>)>,
    cancelled_tests: std::collections::VecDeque<String>,
}
/// The path is fixed for the life of the process. Reload never picks another
/// environment variable or silently falls back after an explicit path error.
#[derive(Clone)]
pub struct ModelConfigManager {
    path: Result<PathBuf, ApiError>,
    state: Arc<Mutex<ConfigState>>,
}
impl Default for ModelConfigManager {
    fn default() -> Self {
        Self::new()
    }
}
impl ModelConfigManager {
    pub fn new() -> Self {
        let manager = Self::from_path(resolve_config_path());
        #[cfg(not(windows))]
        if std::env::var_os("XYZAI_MODEL_CONFIG").is_none()
            && std::env::var_os("XDG_CONFIG_HOME")
                .is_some_and(|p| !p.is_empty() && !Path::new(&p).is_absolute())
        {
            manager.state.lock().unwrap_or_else(|e| e.into_inner()).status.warning = Some("Relative XDG_CONFIG_HOME was ignored; using the default HOME configuration directory.".into());
        }
        manager
    }
    pub fn from_path(path: Result<PathBuf, ApiError>) -> Self {
        let status = ModelConfigStatus {
            path: path.as_ref().ok().map(|p| p.display().to_string()),
            revision: Some("0".into()),
            file_revision: None,
            base_url: None,
            key_present: false,
            path_overridden: std::env::var_os("XYZAI_MODEL_CONFIG").is_some(),
            ready: false,
            destination: None,
            model: None,
            consented: false,
            error: None,
            warning: None,
            context_policy: "explicit_chat_only".into(),
            persistence: "memory_only".into(),
            context_window_tokens: None,
            context_window_override_tokens: None,
            capacity_source: crate::context::CapacitySource::Unavailable,
            output_reserve_tokens: None,
            safety_margin_tokens: None,
            input_budget_tokens: None,
            context_auto_compact: true,
            context_auto_mode: crate::context::CompactionMode::Llm,
        };
        let this = Self {
            path,
            state: Arc::new(Mutex::new(ConfigState {
                status,
                snapshot: None,
                revision: 0,
                file_bytes: None,
                active_test: None,
                cancelled_tests: std::collections::VecDeque::new(),
            })),
        };
        this.reload();
        this
    }
    /// Host-only filtering registration, not model-destination authorization.
    /// The key never leaves the shared in-memory redactor through this API.
    pub(crate) fn register_history_secrets(
        &self,
        redactor: &crate::observation::ObservationBuffer,
    ) -> Result<(), ApiError> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(snapshot) = &state.snapshot {
            redactor.add_known_secret(snapshot.api_key.as_str())?;
        }
        Ok(())
    }
    /// Host-only snapshot for local budget previews and deterministic quick
    /// compaction. Possession does not grant permission to transmit; callers
    /// must still use authorized_snapshot_for/terminal_snapshot before network.
    pub(crate) fn local_snapshot(&self) -> Option<Arc<ModelSnapshot>> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot
            .clone()
    }
    pub fn status(&self) -> ModelConfigStatus {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
            .clone()
    }
    pub fn reload(&self) -> ModelConfigStatus {
        // Serialize disk reads and commits so a slower older reload cannot win.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let result = self
            .path
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|path| settings::read_configuration(path));
        state.revision += 1;
        state.status.revision = Some(state.revision.to_string());
        state.status.consented = false;
        state.status.file_revision = None;
        state.status.base_url = None;
        state.status.key_present = false;
        state.status.ready = false;
        state.status.destination = None;
        state.status.model = None;
        state.status.context_window_tokens = None;
        state.status.context_window_override_tokens = None;
        state.status.capacity_source = crate::context::CapacitySource::Unavailable;
        state.status.output_reserve_tokens = None;
        state.status.safety_margin_tokens = None;
        state.status.input_budget_tokens = None;
        state.status.context_auto_compact = true;
        state.status.context_auto_mode = crate::context::CompactionMode::Llm;
        state.snapshot = None;
        state.file_bytes = None;
        match result {
            Ok((bytes, snapshot)) => {
                state.status.file_revision = Some(uuid::Uuid::new_v4().to_string());
                state.file_bytes = bytes;
                state.status.error = None;
                if let Some(snapshot) = snapshot {
                    settings::apply_snapshot(&mut state, snapshot);
                }
            }
            Err(error) => state.status.error = Some(error),
        }
        state.status.clone()
    }
    pub fn confirm_destination(
        &self,
        revision: &str,
        destination: &str,
    ) -> Result<ModelConfigStatus, ApiError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.status.ready
            || state.status.revision.as_deref() != Some(revision)
            || state.status.destination.as_deref() != Some(destination)
        {
            return Err(ApiError::new("MODEL_CONFIG_CHANGED", "The model configuration changed. Review its current destination before consenting."));
        }
        state.revision += 1;
        state.status.revision = Some(state.revision.to_string());
        state.status.consented = true;
        Ok(state.status.clone())
    }
    /// Per-run terminal-context consent, independent of text-only chat consent.
    pub(crate) fn terminal_snapshot(
        &self,
        revision: &str,
        destination: &str,
    ) -> Result<Arc<ModelSnapshot>, ApiError> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.status.ready
            || state.status.revision.as_deref() != Some(revision)
            || state.status.destination.as_deref() != Some(destination)
        {
            return Err(ApiError::new("MODEL_CONFIG_CHANGED", "Review the current model destination before allowing this task's terminal context."));
        }
        state.snapshot.clone().ok_or_else(|| {
            ApiError::new(
                "MODEL_CONFIG_UNAVAILABLE",
                "Load a valid model configuration first.",
            )
        })
    }
    pub(crate) fn authorized_snapshot_for(
        &self,
        revision: &str,
        destination: &str,
    ) -> Result<Arc<ModelSnapshot>, ApiError> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.status.revision.as_deref() != Some(revision)
            || state.status.destination.as_deref() != Some(destination)
        {
            return Err(ApiError::new(
                "MODEL_CONFIG_CHANGED",
                "The model destination changed. Review it before sending again.",
            ));
        }
        if !state.status.ready || !state.status.consented {
            return Err(ApiError::new(
                "MODEL_CONSENT_REQUIRED",
                "Review and allow the model destination first.",
            ));
        }
        state.snapshot.clone().ok_or_else(|| {
            ApiError::new(
                "MODEL_CONFIG_UNAVAILABLE",
                "Load a valid model configuration first.",
            )
        })
    }
    pub(crate) fn authorized_snapshot(&self) -> Result<Arc<ModelSnapshot>, ApiError> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.status.ready || !state.status.consented {
            return Err(ApiError::new("MODEL_CONSENT_REQUIRED", "Review and allow the configured model destination before sending chat history or a connection test."));
        }
        state.snapshot.clone().ok_or_else(|| {
            ApiError::new(
                "MODEL_CONFIG_UNAVAILABLE",
                "Load a valid model configuration first.",
            )
        })
    }
}
fn invalid(message: &str) -> ApiError {
    ApiError::new("MODEL_CONFIG_INVALID", message)
}
fn resolve_config_path() -> Result<PathBuf, ApiError> {
    if let Some(path) = std::env::var_os("XYZAI_MODEL_CONFIG") {
        let path = PathBuf::from(path);
        if path.as_os_str().is_empty() || !path.is_absolute() {
            return Err(invalid("XYZAI_MODEL_CONFIG must contain an absolute local file path. Empty or relative overrides do not fall back."));
        }
        return Ok(path);
    }
    #[cfg(windows)]
    let root = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty() && Path::new(p).is_absolute())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|p| p.join(".config"))
        });
    root.filter(|p| p.is_absolute()).map(|p| p.join("xyzaishell/model.toml")).ok_or_else(|| invalid("A valid absolute operating-system configuration directory is unavailable. Set XYZAI_MODEL_CONFIG explicitly."))
}
fn parse_config(text: &str) -> Result<ModelSnapshot, ApiError> {
    // Do not display TOML parser diagnostics: they may include the credential line.
    let raw: ConfigFile = toml::from_str(text).map_err(|_| invalid("The model TOML has invalid syntax, unknown fields, or wrong field types. Details are hidden to protect credentials."))?;
    if raw.schema_version != 1 || raw.model.protocol != "openai_chat_completions" {
        return Err(invalid(
            "Use schema_version 1 and the openai_chat_completions protocol.",
        ));
    }
    if raw.capabilities.streaming != "required" || raw.capabilities.tool_calling != "required" {
        return Err(invalid("Streaming and tool_calling capability declarations must be required. M2 implements chat only; tool compatibility is not certified."));
    }
    let key = &raw.model.api_key;
    // An explicitly empty key is the persisted incomplete/cleared state.
    if key.len() > 16 * 1024
        || key.trim() != key
        || key.chars().any(char::is_control)
        || key.contains("REPLACE_WITH")
    {
        return Err(invalid("Provide a non-placeholder API key without surrounding whitespace or control characters."));
    }
    if raw.model.name.trim().is_empty()
        || raw.model.name.len() > 256
        || raw.model.name.chars().any(char::is_control)
        || raw.model.name.contains("REPLACE_WITH")
    {
        return Err(invalid(
            "Provide a non-placeholder model name of at most 256 bytes without control characters.",
        ));
    }
    let mut url = url::Url::parse(&raw.model.base_url)
        .map_err(|_| invalid("The model base URL must be an absolute HTTPS URL."))?;
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
        || url.host_str().is_some_and(|h| h.ends_with(".invalid"))
        || !(url.scheme() == "https"
            || (url.scheme() == "http" && loopback && raw.transport.allow_loopback_http))
    {
        return Err(invalid("Use an HTTPS base URL without credentials, query, or fragment. Plain HTTP requires explicit loopback permission and localhost, 127.0.0.1, or ::1."));
    }
    let path = url.path().trim_end_matches('/').to_owned();
    if path.ends_with("/chat/completions") || path.ends_with("/responses") {
        return Err(invalid(
            "Configure an API base URL, not a full completion or responses endpoint.",
        ));
    }
    url.set_path(&path);
    let destination = url.as_str().trim_end_matches('/').to_owned();
    let r = &raw.request;
    if !(1..=120).contains(&r.connect_timeout_seconds)
        || !(1..=3600).contains(&r.request_timeout_seconds)
        || !(1..=300).contains(&r.stream_idle_timeout_seconds)
        || !(1..=131072).contains(&r.max_output_tokens)
    {
        return Err(invalid(
            "Model request limits are outside their documented ranges.",
        ));
    }
    r.context_budget()
        .resolve(crate::context::TurnPhase::ActiveTurn, 0)
        .map_err(|_| invalid("The context window must exceed the output reserve plus the safety margin. Correct the explicit capacity setting; it cannot fall back to the default."))?;
    Ok(ModelSnapshot {
        destination,
        model: raw.model.name.clone(),
        api_key: Zeroizing::new(key.clone()),
        request: raw.request,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(base: &str) -> String {
        format!("schema_version=1\n[model]\nprotocol='openai_chat_completions'\nbase_url='{base}'\napi_key='synthetic-key'\nname='mock-model'\n")
    }
    #[test]
    fn validates_strict_schema_and_redacts_parser_errors() {
        let good = config("https://example.com/v1/");
        assert_eq!(
            parse_config(&good).unwrap().destination,
            "https://example.com/v1"
        );
        for value in [
            good.replace("schema_version=1", "schema_version=2"),
            good.replace("api_key", "api_kye"),
            good.replace("synthetic-key", "REPLACE_WITH_LOCAL_API_KEY"),
            good.replace("synthetic-key", " synthetic-key"),
            good + "bad = 'synthetic-private-value'\n",
        ] {
            let error = parse_config(&value).err().unwrap();
            assert!(!error.to_string().contains("synthetic"));
        }
    }
    #[test]
    fn destination_validation() {
        for url in [
            "http://example.com/v1",
            "https://user:pass@example.com/v1",
            "https://example.com/v1?key=secret",
            "https://example.com/v1#fragment",
            "https://example.invalid/v1",
            "https://example.com/v1/chat/completions",
        ] {
            assert!(parse_config(&config(url)).is_err(), "{url}");
        }
        assert!(parse_config(
            &(config("http://127.0.0.1:123/v1") + "[transport]\nallow_loopback_http=true\n")
        )
        .is_ok());
        assert!(parse_config(&config("http://localhost/v1")).is_err());
    }
}
