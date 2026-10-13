//! Explicit, file-backed model configuration. No SDK environment fallbacks.
//! API keys never implement Debug/Serialize and never enter public status/errors.
use crate::ssh::ApiError;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::io::Read;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

pub const MAX_CONFIG_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
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
#[derive(Deserialize)]
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
#[derive(Deserialize)]
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
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RequestOptions {
    pub connect_timeout_seconds: u64,
    pub request_timeout_seconds: u64,
    pub stream_idle_timeout_seconds: u64,
    pub max_output_tokens: u64,
}
impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            connect_timeout_seconds: 10,
            request_timeout_seconds: 120,
            stream_idle_timeout_seconds: 30,
            max_output_tokens: 4096,
        }
    }
}
#[derive(Default, Deserialize)]
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
    pub ready: bool,
    pub destination: Option<String>,
    pub model: Option<String>,
    pub consented: bool,
    pub error: Option<ApiError>,
    pub warning: Option<String>,
    pub context_policy: String,
    pub persistence: String,
}
struct ConfigState {
    status: ModelConfigStatus,
    snapshot: Option<Arc<ModelSnapshot>>,
    revision: u64,
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
            ready: false,
            destination: None,
            model: None,
            consented: false,
            error: None,
            warning: None,
            context_policy: "explicit_chat_only".into(),
            persistence: "memory_only".into(),
        };
        let this = Self {
            path,
            state: Arc::new(Mutex::new(ConfigState {
                status,
                snapshot: None,
                revision: 0,
            })),
        };
        this.reload();
        this
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
            .and_then(|path| load_config(path));
        state.revision += 1;
        state.status.revision = Some(state.revision.to_string());
        state.status.consented = false;
        match result {
            Ok(snapshot) => {
                state.status.ready = true;
                state.status.destination = Some(snapshot.destination.clone());
                state.status.model = Some(snapshot.model.clone());
                state.status.error = None;
                state.snapshot = Some(Arc::new(snapshot));
            }
            Err(error) => {
                state.status.ready = false;
                state.status.destination = None;
                state.status.model = None;
                state.status.error = Some(error);
                state.snapshot = None;
            }
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
    pub(crate) fn authorized_snapshot(&self) -> Result<Arc<ModelSnapshot>, ApiError> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.status.consented {
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
fn load_config(path: &Path) -> Result<ModelSnapshot, ApiError> {
    let text = read_secure_config(path)?;
    parse_config(&text)
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
    if key.is_empty()
        || key.len() > 16 * 1024
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
    Ok(ModelSnapshot {
        destination,
        model: raw.model.name.clone(),
        api_key: Zeroizing::new(key.clone()),
        request: raw.request,
    })
}

/// Component-by-component opening prevents symlink substitution; descriptor
/// metadata checks the file actually read rather than a raced path lookup.
#[cfg(target_os = "linux")]
fn read_secure_config(path: &Path) -> Result<Zeroizing<String>, ApiError> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
        path::Component,
    };
    let denied = || {
        ApiError::new("MODEL_FILE_SECURITY", "Use a private local model file owned by this user (0600), inside a private directory (0700), without symlinks or hard links.")
    };
    if !path.is_absolute() {
        return Err(denied());
    }
    let components: Vec<_> = path.components().collect();
    if components.len() < 2
        || components[1..]
            .iter()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(denied());
    }
    let mut directory = std::fs::File::open("/").map_err(|_| denied())?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    for component in &components[1..components.len() - 1] {
        let name = CString::new(component.as_os_str().as_bytes()).map_err(|_| denied())?;
        // SAFETY: valid live directory descriptor and NUL-terminated component.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(denied());
        }
        // SAFETY: openat returned a new owned descriptor.
        directory = unsafe { std::fs::File::from_raw_fd(fd) };
        let meta = directory.metadata().map_err(|_| denied())?;
        if (meta.uid() != uid && meta.uid() != 0)
            || (meta.mode() & 0o022 != 0 && !(meta.uid() == 0 && meta.mode() & 0o1000 != 0))
        {
            return Err(denied());
        }
    }
    let parent = directory.metadata().map_err(|_| denied())?;
    if parent.uid() != uid || parent.mode() & 0o077 != 0 {
        return Err(denied());
    }
    let name =
        CString::new(components.last().unwrap().as_os_str().as_bytes()).map_err(|_| denied())?;
    // SAFETY: same descriptor/name contract; NONBLOCK avoids blocking on a FIFO.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(ApiError::new("MODEL_FILE_UNAVAILABLE", "The selected model configuration is missing, unreadable, or unsafe. Check the displayed path."));
    }
    // SAFETY: openat returned a new owned descriptor.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = file.metadata().map_err(|_| denied())?;
    if !meta.is_file()
        || meta.uid() != uid
        || meta.mode() & 0o077 != 0
        || meta.nlink() != 1
        || meta.len() > MAX_CONFIG_BYTES as u64
    {
        return Err(denied());
    }
    let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: writable statfs storage and live fd; initialized only on success.
    if unsafe { libc::fstatfs(file.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
        return Err(denied());
    }
    // SAFETY: successful fstatfs initialized the structure.
    let filesystem = unsafe { filesystem.assume_init() };
    // Explicit local filesystem allowlist: ext*, XFS, Btrfs, tmpfs, overlayfs.
    if !matches!(
        filesystem.f_type as u64,
        0xef53 | 0x58465342 | 0x9123683e | 0x01021994 | 0x794c7630
    ) {
        return Err(denied());
    }
    let mut text = Zeroizing::new(String::new());
    (&file)
        .take(MAX_CONFIG_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|_| denied())?;
    if text.len() > MAX_CONFIG_BYTES {
        return Err(denied());
    }
    let after = file.metadata().map_err(|_| denied())?;
    let parent_after = directory.metadata().map_err(|_| denied())?;
    if after.uid() != uid
        || after.mode() & 0o077 != 0
        || after.nlink() != 1
        || after.len() != meta.len()
        || after.mtime() != meta.mtime()
        || after.mtime_nsec() != meta.mtime_nsec()
        || after.ctime() != meta.ctime()
        || after.ctime_nsec() != meta.ctime_nsec()
        || parent_after.uid() != uid
        || parent_after.mode() & 0o077 != 0
    {
        return Err(ApiError::new("MODEL_FILE_CHANGED", "The model file or its permissions changed while it was read. Finish editing and reload it."));
    }
    Ok(text)
}
#[cfg(windows)]
fn read_secure_config(path: &Path) -> Result<Zeroizing<String>, ApiError> {
    let bytes = Zeroizing::new(crate::windows_private_file::read_private_file(path, MAX_CONFIG_BYTES).map_err(|_| ApiError::new("MODEL_FILE_SECURITY", "Use a private local model configuration and immediate parent directory owned by this Windows user. Their ACLs may grant access only to this user, SYSTEM, and Administrators; links and network paths are refused."))?);
    std::str::from_utf8(&bytes)
        .map(|text| Zeroizing::new(text.to_owned()))
        .map_err(|_| invalid("The model configuration must be valid UTF-8."))
}
#[cfg(not(any(target_os = "linux", windows)))]
fn read_secure_config(_path: &Path) -> Result<Zeroizing<String>, ApiError> {
    Err(ApiError::new("MODEL_FILE_PLATFORM_UNSUPPORTED", "Model configuration is unavailable on this platform until ownership, local-filesystem and ACL validation is implemented."))
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
