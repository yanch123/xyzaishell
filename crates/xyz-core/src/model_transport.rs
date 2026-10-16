//! The only HTTP path used by model chat. Limits apply before Rig's SSE/JSON
//! parsers. Failures never retain provider diagnostics, URLs, or secrets; only
//! bounded request IDs and explicit usage counters survive successful frames.
use crate::ssh::ApiError;
use bytes::Bytes;
use futures::stream;
use rig_core::{
    completion::CompletionRequest,
    http_client::{
        BoxedStream, Error, HeaderMap, HttpClientExt, LazyBody, Method, MultipartForm, Request,
        Response, Result as HttpResult, StreamingResponse,
    },
    providers::openai::OpenAIConfig,
    wasm_compat::WasmCompatSend,
    wire::{Body, Mode, Operation, Wire},
};
use rig_reqwest::reqwest;
use serde_json::Value;
use std::{
    fmt,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::{timeout_at, Instant};

pub(crate) const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 256 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_FRAME_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy)]
struct Limits {
    request: usize,
    chunk: usize,
    line: usize,
    frame: usize,
    response: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            request: MAX_REQUEST_BYTES,
            chunk: MAX_CHUNK_BYTES,
            line: MAX_LINE_BYTES,
            frame: MAX_FRAME_BYTES,
            response: MAX_RESPONSE_BYTES,
        }
    }
}

#[derive(Clone)]
pub(crate) struct BoundedHttpClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    total: Duration,
    idle: Duration,
    limits: Limits,
    errors: SafeErrors,
    prepared_body: Option<Value>,
    preflight: Option<Arc<RequestPreflight>>,
    usage: UsageCapture,
}
type RequestPreflight = dyn Fn(&Value) -> Result<(), ApiError> + Send + Sync;
impl fmt::Debug for BoundedHttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BoundedHttpClient { destination: [redacted] }")
    }
}

/// `destination` comes only from the validated, explicitly consented model
/// snapshot. The configuration layer gates HTTP behind its loopback-test flag;
/// this layer independently rejects non-loopback cleartext and pins every POST.
pub(crate) fn build_transport(
    destination: &str,
    connect: Duration,
    total: Duration,
    idle: Duration,
) -> Result<BoundedHttpClient, ApiError> {
    let invalid = || {
        ApiError::new(
            "MODEL_TRANSPORT_INVALID",
            "The model transport configuration is invalid.",
        )
    };
    let mut endpoint = reqwest::Url::parse(destination).map_err(|_| invalid())?;
    let loopback = match endpoint.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain("localhost")) => true,
        _ => false,
    };
    if !(endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback)
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || endpoint.host().is_none()
        || connect.is_zero()
        || total.is_zero()
        || idle.is_zero()
    {
        return Err(invalid());
    }
    endpoint.set_path(&format!(
        "{}/chat/completions",
        endpoint.path().trim_end_matches('/')
    ));
    let client = reqwest::Client::builder()
        .no_proxy()
        // localhost is allowed only by explicit loopback-test configuration.
        // Pin its resolution as well, independent of hosts files or DNS.
        .resolve_to_addrs(
            "localhost",
            &[
                std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
                std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, 0)),
            ],
        )
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_gzip()
        .no_brotli()
        .no_zstd()
        .no_deflate()
        .connect_timeout(connect)
        .timeout(total)
        // Do not keep an unused connection alive after a cancelled model turn.
        .pool_max_idle_per_host(0)
        .connection_verbose(false)
        .build()
        .map_err(|_| {
            ApiError::new(
                "MODEL_TRANSPORT_UNAVAILABLE",
                "The secure model transport could not be initialized.",
            )
        })?;
    Ok(BoundedHttpClient {
        client,
        endpoint,
        total,
        idle,
        limits: Limits::default(),
        errors: SafeErrors::default(),
        prepared_body: None,
        preflight: None,
        usage: UsageCapture::default(),
    })
}

#[derive(Clone, Copy, Debug)]
enum Failure {
    Unsupported,
    Destination,
    RequestLimit,
    RequestInvalid,
    PreparedMismatch,
    ProviderContextOverflow,
    ResponseLimit,
    ChunkLimit,
    LineLimit,
    FrameLimit,
    ContentType,
    Encoding,
    InvalidUtf8,
    Truncated,
    Timeout,
    IdleTimeout,
    TotalTimeout,
    Network,
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "Only streaming model requests are supported.",
            Self::Destination => "The model request does not match the approved destination.",
            Self::RequestLimit => "The model request exceeded its byte limit.",
            Self::RequestInvalid => "The model request could not be serialized safely.",
            Self::PreparedMismatch => {
                "The prepared model request no longer matches its configuration."
            }
            Self::ResponseLimit => "The model response exceeded its byte limit.",
            Self::ChunkLimit => "The model response chunk exceeded its byte limit.",
            Self::LineLimit => "The model response line exceeded its byte limit.",
            Self::FrameLimit => "The model response event exceeded its byte limit.",
            Self::ProviderContextOverflow => "The provider rejected the context size. Adjust the configured capacity before explicitly retrying.",
            Self::ContentType => "The model response was not an event stream.",
            Self::Encoding => "The model response used an unsupported content encoding.",
            Self::InvalidUtf8 => "The model event stream was not valid UTF-8.",
            Self::Truncated => "The model event stream ended inside an event.",
            Self::Timeout | Self::TotalTimeout => "The model request timed out.",
            Self::IdleTimeout => "The model response became idle and timed out.",
            Self::Network => "The model connection failed.",
        })
    }
}
impl std::error::Error for Failure {}
fn failed(reason: Failure) -> Error {
    Error::instance(reason)
}
fn deadline_failure(deadline: Instant) -> Failure {
    if Instant::now() >= deadline {
        Failure::TotalTimeout
    } else {
        Failure::IdleTimeout
    }
}
fn deadline_error(deadline: Instant) -> Error {
    failed(deadline_failure(deadline))
}
fn network_failure(error: reqwest::Error) -> Failure {
    // Do not preserve reqwest's error as a source: its Display and Debug can
    // include the request URL and private provider diagnostics.
    if error.is_timeout() {
        Failure::Timeout
    } else {
        Failure::Network
    }
}
fn network(error: reqwest::Error) -> Error {
    failed(network_failure(error))
}

/// Inspect only a small, explicitly bounded JSON error, and only its known
/// machine-readable code. Never retain error messages or infer a real window.
/// Unannounced, encoded, oversized or stalled bodies keep the generic error.
async fn provider_context_overflow(mut response: reqwest::Response, deadline: Instant) -> bool {
    const ERROR_BYTES: usize = 16 * 1024;
    if !matches!(response.status().as_u16(), 400 | 413)
        || !response
            .content_length()
            .is_some_and(|n| n > 0 && n <= ERROR_BYTES as u64)
        || !response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
        || response
            .headers()
            .get("content-encoding")
            .is_some_and(|v| v.as_bytes() != b"identity")
    {
        return false;
    }
    #[derive(serde::Deserialize)]
    struct Envelope<'a> {
        #[serde(borrow)]
        error: Code<'a>,
    }
    #[derive(serde::Deserialize)]
    struct Code<'a> {
        #[serde(default, borrow)]
        code: Option<&'a str>,
        #[serde(default, rename = "type", borrow)]
        kind: Option<&'a str>,
    }
    timeout_at(
        deadline.min(Instant::now() + Duration::from_millis(250)),
        async move {
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if bytes.len().saturating_add(chunk.len()) > ERROR_BYTES {
                            return false;
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    Ok(None) => break,
                    Err(_) => return false,
                }
            }
            let Ok(envelope) = serde_json::from_slice::<Envelope<'_>>(&bytes) else {
                return false;
            };
            matches!(
                envelope.error.code,
                Some("context_length_exceeded" | "context_window_exceeded")
            ) || matches!(
                envelope.error.kind,
                Some("context_length_exceeded" | "context_window_exceeded")
            )
        },
    )
    .await
    .unwrap_or(false)
}

// Share only fixed application-safe classifications across the client clone
// moved into Rig. No network error object or provider material is retained.
#[derive(Clone, Default)]
struct SafeErrors(Arc<Mutex<Option<ApiError>>>);
impl SafeErrors {
    fn record(&self, error: &Error) {
        // These errors originate only in the host's preflight callback, never in
        // provider JSON or diagnostics. Preserve their actionable budget code.
        if let Error::Instance(error) = error {
            if let Some(error) = error.downcast_ref::<ApiError>() {
                *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(error.clone());
                return;
            }
        }
        let (code, message) = match error {
            Error::InvalidStatusCodeWithDetails { status, .. }
                if status.as_u16() == 401 || status.as_u16() == 403 =>
            {
                (
                    "MODEL_AUTH_FAILED",
                    "The model provider rejected the credentials or permissions.",
                )
            }
            Error::InvalidStatusCodeWithDetails { status, .. } if status.as_u16() == 429 => (
                "MODEL_RATE_LIMITED",
                "The model provider rate limited this request. No automatic retry occurs.",
            ),
            Error::InvalidStatusCodeWithDetails { .. } => (
                "MODEL_HTTP_ERROR",
                "The model provider rejected the request. Its actual context capacity or supported options may differ from this configuration. Review the capacity and model settings before explicitly retrying; no automatic retry occurs.",
            ),
            Error::Instance(error) => match error.downcast_ref::<Failure>() {
                Some(Failure::IdleTimeout) => (
                    "MODEL_IDLE_TIMEOUT",
                    "The model response exceeded the configured idle timeout.",
                ),
                Some(Failure::Timeout | Failure::TotalTimeout) => (
                    "MODEL_TIMEOUT",
                    "The model request exceeded a configured timeout.",
                ),
                Some(
                    Failure::ResponseLimit
                    | Failure::ChunkLimit
                    | Failure::LineLimit
                    | Failure::FrameLimit,
                ) => (
                    "MODEL_RESPONSE_LIMIT",
                    "The model response exceeded its safety byte limit.",
                ),
                Some(Failure::ProviderContextOverflow) => (
                    "CONTEXT_PROVIDER_OVERFLOW",
                    "The provider rejected the context size. Its capacity may differ from the configured default or override. Adjust the context-window setting and explicitly retry. The request and commands were not replayed.",
                ),
                Some(Failure::PreparedMismatch) => (
                    "CONTEXT_STALE",
                    "The prepared request changed. Prepare its current context again before sending.",
                ),
                Some(Failure::RequestInvalid) => (
                    "MODEL_REQUEST_INVALID",
                    "The model request could not be serialized safely.",
                ),
                Some(Failure::RequestLimit) => (
                    "MODEL_REQUEST_LIMIT",
                    "The model request exceeded its safety byte limit.",
                ),
                Some(
                    Failure::Truncated
                    | Failure::InvalidUtf8
                    | Failure::ContentType
                    | Failure::Encoding,
                ) => (
                    "MODEL_STREAM_INVALID",
                    "The model response was malformed or interrupted.",
                ),
                _ => (
                    "MODEL_REQUEST_FAILED",
                    "The model connection failed. No automatic retry occurs.",
                ),
            },
            _ => (
                "MODEL_REQUEST_FAILED",
                "The model request failed. No automatic retry occurs.",
            ),
        };
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) =
            Some(ApiError::new(code, message));
    }
}
/// Only explicitly reported fields are retained. Missing provider usage is
/// unknown, not a zero-filled Rig Usage value. Cache tokens remain an input
/// subset and reasoning tokens are never blindly added to output tokens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ReportedUsage {
    pub request_id: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}
impl ReportedUsage {
    pub(crate) fn complete(&self) -> bool {
        self.input_tokens.is_some() && self.output_tokens.is_some()
    }
}
#[derive(Clone, Default)]
struct UsageCapture(Arc<Mutex<UsageState>>);
#[derive(Default)]
struct UsageState {
    request_id: Option<String>,
    reported: Option<ReportedUsage>,
    // Provider metadata can echo credentials. This temporary in-memory filter
    // shares the request lifetime and is never serialized, logged, or Debug.
    credential_filter: Option<zeroize::Zeroizing<String>>,
}
fn safe_request_id(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b)))
    .then(|| value.to_owned())
}
impl UsageCapture {
    fn reset(&self, authorization: Option<&str>) {
        let credential_filter = authorization
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| !value.is_empty())
            .map(|value| zeroize::Zeroizing::new(value.to_owned()));
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = UsageState {
            credential_filter,
            ..UsageState::default()
        };
    }
    fn request_id(&self, value: &str) {
        if let Some(value) = safe_request_id(value) {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if state
                .credential_filter
                .as_ref()
                .is_some_and(|secret| value.contains(secret.as_str()))
            {
                return;
            }
            if state.request_id.is_none() {
                state.request_id = Some(value.clone());
                if let Some(usage) = &mut state.reported {
                    usage.request_id = Some(value);
                }
            }
        }
    }
    fn frame(&self, bytes: &[u8]) {
        // FrameGuard has already bounded and validated UTF-8, so this temporary
        // parse cannot accumulate an unbounded provider response.
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        let data = bytes
            .split(|b| *b == b'\n')
            .filter_map(|line| {
                line.strip_prefix(b"data:")
                    .map(|value| value.strip_prefix(b" ").unwrap_or(value))
            })
            .collect::<Vec<_>>()
            .join(&b'\n');
        let Ok(value) = serde_json::from_slice::<Value>(&data) else {
            return;
        };
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            self.request_id(id);
        }
        let Some(usage) = value.get("usage").filter(|v| v.is_object()) else {
            return;
        };
        let input = usage.get("prompt_tokens").and_then(Value::as_u64);
        let output = usage.get("completion_tokens").and_then(Value::as_u64);
        let cached = usage
            .pointer("/prompt_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .filter(|cached| input.is_none_or(|input| *cached <= input));
        let reasoning = usage
            .pointer("/completion_tokens_details/reasoning_tokens")
            .and_then(Value::as_u64);
        if input.is_none() && output.is_none() && cached.is_none() && reasoning.is_none() {
            return;
        }
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // A later explicit field supersedes an earlier field; fields omitted by
        // the provider do not create zeros or erase a previously reported count.
        let mut reported = state.reported.take().unwrap_or_default();
        reported.request_id = state.request_id.clone();
        if input.is_some() {
            reported.input_tokens = input;
        }
        if output.is_some() {
            reported.output_tokens = output;
        }
        if cached.is_some() {
            reported.cached_input_tokens = cached;
        }
        if reasoning.is_some() {
            reported.reasoning_tokens = reasoning;
        }
        if reported
            .cached_input_tokens
            .zip(reported.input_tokens)
            .is_some_and(|(cached, input)| cached > input)
        {
            reported.cached_input_tokens = None;
        }
        state.reported = Some(reported);
    }
}
/// Canonicalization is shared by local preparation and the actual HTTP path.
/// JSON escaping and key ordering therefore have one byte-accounting authority.
fn canonical_body(bytes: &[u8]) -> Result<Value, Failure> {
    let mut body: Value = serde_json::from_slice(bytes).map_err(|_| Failure::RequestInvalid)?;
    if !body.is_object() {
        return Err(Failure::RequestInvalid);
    }
    if body.get("stream") == Some(&Value::Bool(true)) {
        if body.get("stream_options").is_none_or(Value::is_null) {
            body["stream_options"] = serde_json::json!({});
        }
        if !body["stream_options"].is_object() {
            return Err(Failure::RequestInvalid);
        }
        body["stream_options"]["include_usage"] = Value::Bool(true);
    }
    Ok(body)
}
/// Runs the exact public Rig preparation and wire encoder used by its stream
/// driver. This is local data processing: no transport, socket, or real key is
/// created. Oversized candidates remain measurable before host compaction.
pub(crate) async fn canonical_request_body(
    config: &crate::model::ModelSnapshot,
    request: CompletionRequest,
) -> Result<Value, ApiError> {
    let error = || {
        ApiError::new(
            "MODEL_REQUEST_INVALID",
            "The local model request could not be prepared. No request was sent.",
        )
    };
    let wire = OpenAIConfig::new("local-preflight-placeholder")
        .with_base_url(&config.destination)
        .chat(&config.model);
    let request =
        rig_core::operation::Completion::prepare(request, &wire.describe()).map_err(|_| error())?;
    let encoded = wire.encode(request, Mode::Streaming).map_err(|_| error())?;
    let Body::Bytes(bytes) = encoded.request.into_body() else {
        return Err(error());
    };
    let body = canonical_body(&bytes).map_err(|_| error())?;
    if body.get("model").and_then(Value::as_str) != Some(config.model.as_str()) {
        return Err(error());
    }
    Ok(body)
}

impl BoundedHttpClient {
    /// Host-only projection override. Only messages may change; all model,
    /// output, tool-schema and protocol fields remain bound to the Rig request.
    pub(crate) fn with_prepared_body(mut self, body: Value) -> Self {
        self.prepared_body = Some(body);
        self
    }
    pub(crate) fn with_preflight<F>(mut self, preflight: F) -> Self
    where
        F: Fn(&Value) -> Result<(), ApiError> + Send + Sync + 'static,
    {
        self.preflight = Some(Arc::new(preflight));
        self
    }
    pub(crate) fn reported_usage(&self) -> Option<ReportedUsage> {
        self.usage
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reported
            .clone()
    }
    pub(crate) fn safe_error(&self) -> Option<ApiError> {
        self.errors
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

impl HttpClientExt for BoundedHttpClient {
    fn send<T, U>(
        &self,
        _request: Request<T>,
    ) -> impl Future<Output = HttpResult<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        T: Into<Bytes> + WasmCompatSend,
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        std::future::ready(Err(failed(Failure::Unsupported)))
    }
    fn send_multipart<U>(
        &self,
        _request: Request<MultipartForm>,
    ) -> impl Future<Output = HttpResult<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        std::future::ready(Err(failed(Failure::Unsupported)))
    }
    fn send_streaming<T>(
        &self,
        request: Request<T>,
    ) -> impl Future<Output = HttpResult<StreamingResponse>> + WasmCompatSend
    where
        T: Into<Bytes> + WasmCompatSend,
    {
        let transport = self.clone();
        // Convert before the future so it never retains a generic request.
        let (parts, body) = request.into_parts();
        let body = body.into();
        async move {
            let result = async {
                let url = reqwest::Url::parse(&parts.uri.to_string())
                    .map_err(|_| failed(Failure::Destination))?;
                if parts.method != Method::POST || url != transport.endpoint {
                    return Err(failed(Failure::Destination));
                }
                let mut selected = canonical_body(&body).map_err(failed)?;
                if let Some(prepared) = &transport.prepared_body {
                    if selected.get("stream") != Some(&Value::Bool(true))
                        || !selected.get("model").is_some_and(Value::is_string)
                        || !prepared.get("messages").is_some_and(Value::is_array)
                    {
                        return Err(failed(Failure::PreparedMismatch));
                    }
                    selected["messages"] = prepared["messages"].clone();
                    if &selected != prepared {
                        return Err(failed(Failure::PreparedMismatch));
                    }
                }
                if let Some(preflight) = &transport.preflight {
                    preflight(&selected).map_err(Error::instance)?;
                }
                let body =
                    serde_json::to_vec(&selected).map_err(|_| failed(Failure::RequestInvalid))?;
                if body.len() > transport.limits.request {
                    return Err(failed(Failure::RequestLimit));
                }
                transport.usage.reset(
                    parts
                        .headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                );
                let deadline = Instant::now() + transport.total;
                let response = timeout_at(
                    deadline.min(Instant::now() + transport.idle),
                    transport
                        .client
                        .post(url)
                        .headers(parts.headers)
                        .header("accept-encoding", "identity")
                        .header("content-length", body.len())
                        .body(body)
                        .send(),
                )
                .await
                .map_err(|_| deadline_error(deadline))?
                .map_err(network)?;
                if !response.status().is_success() {
                    let status = response.status();
                    if provider_context_overflow(response, deadline).await {
                        return Err(failed(Failure::ProviderContextOverflow));
                    }
                    // All other diagnostics stay private and are dropped. No
                    // error body or response headers enter downstream errors.
                    return Err(Error::non_success_with_details(
                        status,
                        HeaderMap::new(),
                        "The model provider rejected the request.".into(),
                    ));
                }
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.split(';').next())
                    .map(str::trim);
                if !content_type.is_some_and(|v| v.eq_ignore_ascii_case("text/event-stream")) {
                    return Err(failed(Failure::ContentType));
                }
                if response
                    .headers()
                    .get("content-encoding")
                    .is_some_and(|v| v.as_bytes() != b"identity")
                {
                    return Err(failed(Failure::Encoding));
                }
                if response
                    .content_length()
                    .is_some_and(|len| len > transport.limits.response as u64)
                {
                    return Err(failed(Failure::ResponseLimit));
                }
                if let Some(id) = response
                    .headers()
                    .get("x-request-id")
                    .and_then(|v| v.to_str().ok())
                {
                    transport.usage.request_id(id);
                }
                let state = BodyState {
                    response: Some(response),
                    pending: Bytes::new(),
                    parser: FrameGuard::new(transport.limits),
                    total_read: 0,
                    deadline,
                    idle: transport.idle,
                    ended: false,
                    errors: transport.errors.clone(),
                    usage: transport.usage.clone(),
                };
                let stream: BoxedStream = Box::pin(stream::unfold(state, |mut state| async move {
                    if state.ended {
                        return None;
                    }
                    match state.next_frame().await {
                        Ok(Some(bytes)) => Some((Ok(bytes), state)),
                        Ok(None) => None,
                        Err(reason) => {
                            let error = failed(reason);
                            state.errors.record(&error);
                            state.ended = true;
                            state.response.take();
                            state.pending = Bytes::new();
                            Some((Err(error), state))
                        }
                    }
                }));
                // Rig needs only the verified content type. Keep untrusted provider
                // headers out of later errors, observations, and application events.
                Response::builder()
                    .status(200)
                    .header("content-type", "text/event-stream")
                    .body(stream)
                    .map_err(|_| failed(Failure::Network))
            }
            .await;
            if let Err(error) = &result {
                transport.errors.record(error);
            }
            result
        }
    }
}

struct BodyState {
    response: Option<reqwest::Response>,
    pending: Bytes,
    parser: FrameGuard,
    total_read: usize,
    deadline: Instant,
    idle: Duration,
    ended: bool,
    errors: SafeErrors,
    usage: UsageCapture,
}
impl BodyState {
    async fn next_frame(&mut self) -> Result<Option<Bytes>, Failure> {
        loop {
            if Instant::now() >= self.deadline {
                return Err(Failure::TotalTimeout);
            }
            if !self.pending.is_empty() {
                let (used, frame) = self.parser.push(&self.pending)?;
                self.pending = self.pending.slice(used..);
                if let Some(frame) = frame {
                    self.usage.frame(&frame);
                    if self.parser.done {
                        // A complete [DONE] is the protocol terminator. Do not
                        // wait for EOF or consume data the provider sends later.
                        self.ended = true;
                        self.response.take();
                        self.pending = Bytes::new();
                    }
                    return Ok(Some(frame));
                }
            }
            let Some(response) = self.response.as_mut() else {
                return Ok(None);
            };
            let chunk = timeout_at(
                self.deadline.min(Instant::now() + self.idle),
                response.chunk(),
            )
            .await
            .map_err(|_| deadline_failure(self.deadline))?
            .map_err(network_failure)?;
            let Some(chunk) = chunk else {
                self.ended = true;
                self.response.take();
                self.parser.finish()?;
                // Rig's ChatDecoder accepts EOF after a valid finish_reason,
                // and rejects EOF before it. Do not synthesize [DONE].
                return Ok(None);
            };
            if chunk.len() > self.parser.limits.chunk {
                return Err(Failure::ChunkLimit);
            }
            self.total_read = self
                .total_read
                .checked_add(chunk.len())
                .ok_or(Failure::ResponseLimit)?;
            if self.total_read > self.parser.limits.response {
                return Err(Failure::ResponseLimit);
            }
            self.pending = chunk;
        }
    }
}

/// Normalizes CR, LF, and CRLF while checking each byte BEFORE appending it.
/// Rig receives only complete, validated events, never a growing partial line.
struct FrameGuard {
    limits: Limits,
    frame: Vec<u8>,
    line_bytes: usize,
    skip_lf: bool,
    first: bool,
    done: bool,
}
impl FrameGuard {
    fn new(limits: Limits) -> Self {
        Self {
            limits,
            frame: Vec::new(),
            line_bytes: 0,
            skip_lf: false,
            first: true,
            done: false,
        }
    }
    fn push(&mut self, chunk: &[u8]) -> Result<(usize, Option<Bytes>), Failure> {
        for (offset, &byte) in chunk.iter().enumerate() {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            let newline = byte == b'\n' || byte == b'\r';
            if self.frame.len() >= self.limits.frame {
                return Err(Failure::FrameLimit);
            }
            if !newline && self.line_bytes >= self.limits.line {
                return Err(Failure::LineLimit);
            }
            self.frame.push(if newline { b'\n' } else { byte });
            if newline {
                self.skip_lf = byte == b'\r';
                let empty_line = self.line_bytes == 0;
                self.line_bytes = 0;
                if empty_line {
                    let frame = std::mem::take(&mut self.frame);
                    std::str::from_utf8(&frame).map_err(|_| Failure::InvalidUtf8)?;
                    let data = if self.first {
                        frame.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&frame)
                    } else {
                        &frame
                    };
                    // Joined SSE data can equal [DONE] only with exactly one
                    // data field. Do not recognize a BOM except at stream start.
                    let mut fields = data.split(|byte| *byte == b'\n').filter_map(|line| {
                        if line == b"data" {
                            Some(&b""[..])
                        } else {
                            line.strip_prefix(b"data:")
                                .map(|value| value.strip_prefix(b" ").unwrap_or(value))
                        }
                    });
                    self.done = fields.next() == Some(&b"[DONE]"[..]) && fields.next().is_none();
                    self.first = false;
                    return Ok((offset + 1, Some(Bytes::from(frame))));
                }
            } else {
                self.line_bytes += 1;
            }
        }
        Ok((chunk.len(), None))
    }
    fn finish(&self) -> Result<(), Failure> {
        if self.frame.is_empty() {
            Ok(())
        } else {
            Err(Failure::Truncated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use rig_core::{
        completion::CompletionRequest,
        providers::openai::OpenAIConfig,
        streaming::{Item, StreamEvent},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        task::JoinHandle,
        time::{sleep, timeout},
    };

    fn client(destination: &str) -> BoundedHttpClient {
        build_transport(
            destination,
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(1),
        )
        .unwrap()
    }
    fn request(client: &BoundedHttpClient) -> Request<Bytes> {
        Request::builder()
            .method("POST")
            .uri(client.endpoint.as_str())
            .header("authorization", "Bearer mock-private-key")
            .body(Bytes::from_static(b"{}"))
            .unwrap()
    }
    async fn read_request(socket: &mut TcpStream) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(start) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..start]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map(|v| v.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                if bytes.len() >= start + 4 + length {
                    return bytes[start + 4..start + 4 + length].to_vec();
                }
            }
            assert!(bytes.len() < MAX_REQUEST_BYTES + 4096);
        }
    }
    async fn serve(body: Vec<u8>, headers: &str) -> (BoundedHttpClient, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = client(&format!("http://{}/v1", listener.local_addr().unwrap()));
        let headers = headers.to_owned();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            let response = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n{headers}\r\n");
            socket.write_all(response.as_bytes()).await.unwrap();
            let _ = socket.write_all(&body).await;
            let _ = socket.shutdown().await;
        });
        (transport, server)
    }
    const SSE: &str = "Content-Type: text/event-stream\r\n";

    #[test]
    fn validates_frames_before_allocating_and_preserves_split_grammar() {
        let body = b"\xef\xbb\xbfdata: one\r\n\r\n: ping\r\ndata: [DONE]\r\n\r\n";
        for split in 0..=body.len() {
            let mut guard = FrameGuard::new(Limits::default());
            let mut frames = Vec::new();
            for mut chunk in [&body[..split], &body[split..]] {
                while !chunk.is_empty() {
                    let (used, frame) = guard.push(chunk).unwrap();
                    chunk = &chunk[used..];
                    if let Some(frame) = frame {
                        frames.push(frame);
                    }
                }
            }
            guard.finish().unwrap();
            assert!(guard.done, "split {split}");
            assert_eq!(frames.len(), 2);
            assert_eq!(&frames[0][..], b"\xef\xbb\xbfdata: one\n\n");
            assert_eq!(&frames[1][..], b": ping\ndata: [DONE]\n\n");
        }
        let mut guard = FrameGuard::new(Limits {
            line: 8,
            frame: 20,
            ..Limits::default()
        });
        assert!(matches!(guard.push(b"123456789"), Err(Failure::LineLimit)));
        assert_eq!(guard.frame.len(), 8);
        let mut guard = FrameGuard::new(Limits {
            line: 8,
            frame: 20,
            ..Limits::default()
        });
        assert!(matches!(
            guard.push(b"data: a\ndata: b\ndata: c\n\n"),
            Err(Failure::FrameLimit)
        ));
        assert_eq!(guard.frame.len(), 20);
    }

    #[test]
    fn done_must_be_an_exact_complete_data_event() {
        for body in [
            b"data: [DONE]\ndata:\n\n".as_slice(),
            b": [DONE]\n\n",
            b"data: [DONE] \n\n",
        ] {
            let mut guard = FrameGuard::new(Limits::default());
            guard.push(body).unwrap();
            assert!(!guard.done);
        }
        let mut guard = FrameGuard::new(Limits::default());
        guard.push(b"data: [DONE]\n").unwrap();
        assert!(!guard.done);
        assert!(matches!(guard.finish(), Err(Failure::Truncated)));
        let mut guard = FrameGuard::new(Limits::default());
        guard.push(b"data: first\n\n").unwrap();
        guard.push(b"\xef\xbb\xbfdata: [DONE]\n\n").unwrap();
        assert!(!guard.done, "a BOM is special only at stream start");
        let mut guard = FrameGuard::new(Limits::default());
        assert!(matches!(
            guard.push(b"data: \xff\n\n"),
            Err(Failure::InvalidUtf8)
        ));
    }

    #[tokio::test]
    async fn refuses_oversized_requests_and_changed_destinations_before_connecting() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut transport = client(&format!("http://{}/v1", listener.local_addr().unwrap()));
        transport.limits.request = 1;
        let error = transport
            .send_streaming(request(&transport))
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("request exceeded"));
        let wrong = Request::builder()
            .method("POST")
            .uri(format!("{}/other", transport.endpoint))
            .body(Bytes::new())
            .unwrap();
        assert!(transport.send_streaming(wrong).await.is_err());
        let wrong = Request::builder()
            .method("GET")
            .uri(transport.endpoint.as_str())
            .body(Bytes::new())
            .unwrap();
        assert!(transport.send_streaming(wrong).await.is_err());
        assert!(timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err());
        assert!(!format!("{transport:?}").contains("127.0.0.1"));
    }

    #[tokio::test]
    async fn never_reads_error_bodies_or_follows_redirects() {
        for status in ["429 Too Many Requests", "307 Temporary Redirect"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let other = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let transport = client(&format!("http://{}/v1", listener.local_addr().unwrap()));
            let head = format!("HTTP/1.1 {status}\r\nLocation: http://{}/secret\r\nX-Private: mock-private-key\r\nContent-Length: 999999999\r\n\r\n", other.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                read_request(&mut socket).await;
                socket.write_all(head.as_bytes()).await.unwrap();
                // No body arrives. An error-body reader would block here.
                let mut byte = [0];
                assert!(matches!(
                    timeout(Duration::from_secs(1), socket.read(&mut byte)).await,
                    Ok(Ok(0)) | Ok(Err(_))
                ));
            });
            let error = timeout(
                Duration::from_millis(500),
                transport.send_streaming(request(&transport)),
            )
            .await
            .unwrap()
            .err()
            .unwrap();
            let displayed = format!("{error} {error:?}");
            assert!(!displayed.contains("mock-private-key"));
            assert!(!displayed.contains("/secret"));
            assert!(!displayed.contains("127.0.0.1"));
            assert!(error.non_success_headers().unwrap().is_empty());
            let safe = transport.clone().safe_error().unwrap();
            assert_eq!(
                safe.code,
                if status.starts_with("429") {
                    "MODEL_RATE_LIMITED"
                } else {
                    "MODEL_HTTP_ERROR"
                }
            );
            assert!(timeout(Duration::from_millis(50), other.accept())
                .await
                .is_err());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn bounds_streaming_response_even_without_content_length() {
        let (mut transport, server) =
            serve(b"data: a\n\ndata: b\n\ndata: c\n\n".to_vec(), SSE).await;
        transport.limits.response = 20;
        let mut stream = transport
            .send_streaming(request(&transport))
            .await
            .unwrap()
            .into_body();
        let mut limit_hit = false;
        while let Some(item) = stream.next().await {
            if let Err(error) = item {
                assert!(error.to_string().contains("response exceeded"));
                limit_hit = true;
                break;
            }
        }
        assert!(limit_hit);
        assert!(stream.next().await.is_none());
        server.await.unwrap();
        let (mut transport, server) = serve(b"data: 123456789\n\n".to_vec(), SSE).await;
        transport.limits.line = 8;
        let mut stream = transport
            .send_streaming(request(&transport))
            .await
            .unwrap()
            .into_body();
        assert!(stream
            .next()
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("line exceeded"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_non_sse_compression_and_oversized_announced_body() {
        for headers in [
            "Content-Type: application/json\r\n",
            "Content-Type: text/event-stream\r\nContent-Encoding: gzip\r\n",
            "Content-Type: text/event-stream\r\nContent-Length: 999999999\r\n",
        ] {
            let (transport, server) = serve(Vec::new(), headers).await;
            assert!(transport.send_streaming(request(&transport)).await.is_err());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn incomplete_event_eof_is_an_error_not_silent_truncation() {
        let (transport, server) = serve(b"data: {}\n".to_vec(), SSE).await;
        let mut stream = transport
            .send_streaming(request(&transport))
            .await
            .unwrap()
            .into_body();
        assert!(stream
            .next()
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("inside an event"));
        assert!(stream.next().await.is_none());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn dropping_stream_cancels_local_io() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = client(&format!("http://{}/v1", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: waiting\n\n",
                )
                .await
                .unwrap();
            let mut byte = [0];
            let result = timeout(Duration::from_secs(1), socket.read(&mut byte)).await;
            assert!(
                matches!(result, Ok(Ok(0)) | Ok(Err(_))),
                "socket remained open after drop"
            );
        });
        let mut stream = transport
            .send_streaming(request(&transport))
            .await
            .unwrap()
            .into_body();
        assert!(stream.next().await.unwrap().is_ok());
        drop(stream);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn done_ends_without_waiting_for_socket_eof() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = client(&format!("http://{}/v1", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: [DONE]\n\n",
                )
                .await
                .unwrap();
            let mut byte = [0];
            assert!(matches!(
                timeout(Duration::from_secs(1), socket.read(&mut byte)).await,
                Ok(Ok(0)) | Ok(Err(_))
            ));
        });
        let mut stream = transport
            .send_streaming(request(&transport))
            .await
            .unwrap()
            .into_body();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            &b"data: [DONE]\n\n"[..]
        );
        assert!(stream.next().await.is_none());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn idle_and_total_deadlines_bound_stalled_and_trickling_streams() {
        for trickle in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let transport = build_transport(
                &format!("http://{}/v1", listener.local_addr().unwrap()),
                Duration::from_secs(1),
                // Isolate each timer: a busy test runner must not turn the
                // total-deadline case into an earlier idle-deadline race.
                if trickle {
                    Duration::from_millis(120)
                } else {
                    Duration::from_secs(2)
                },
                if trickle {
                    Duration::from_secs(1)
                } else {
                    Duration::from_millis(50)
                },
            )
            .unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                read_request(&mut socket).await;
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                    .await
                    .unwrap();
                if trickle {
                    for _ in 0..40 {
                        if socket.write_all(b": ping\n\n").await.is_err() {
                            break;
                        }
                        sleep(Duration::from_millis(10)).await;
                    }
                } else {
                    sleep(Duration::from_millis(200)).await;
                }
            });
            let start = Instant::now();
            let mut stream = transport
                .send_streaming(request(&transport))
                .await
                .unwrap()
                .into_body();
            let mut timed_out = false;
            while let Some(item) = stream.next().await {
                if let Err(error) = item {
                    assert!(error.to_string().contains("timed out"));
                    timed_out = true;
                    break;
                }
            }
            assert!(timed_out);
            let safe = transport.safe_error().unwrap();
            assert_eq!(
                safe.code,
                if trickle {
                    "MODEL_TIMEOUT"
                } else {
                    "MODEL_IDLE_TIMEOUT"
                }
            );
            assert!(start.elapsed() < Duration::from_secs(1));
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_network_chunk_is_rejected_before_it_reaches_rig() {
        let (mut transport, server) = serve(b"data: [DONE]\n\n".to_vec(), SSE).await;
        // Zero makes this independent of TCP packet boundaries: the first
        // nonempty network chunk must be rejected before any SSE is returned.
        transport.limits.chunk = 0;
        let mut stream = transport
            .send_streaming(request(&transport))
            .await
            .unwrap()
            .into_body();
        let error = stream.next().await.unwrap().unwrap_err();
        assert!(error.to_string().contains("chunk exceeded"));
        assert_eq!(transport.safe_error().unwrap().code, "MODEL_RESPONSE_LIMIT");
        assert!(stream.next().await.is_none());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_while_awaiting_headers_closes_the_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = client(&format!("http://{}/v1", listener.local_addr().unwrap()));
        let (accepted, received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            accepted.send(()).unwrap();
            let mut byte = [0];
            assert!(matches!(
                timeout(Duration::from_secs(1), socket.read(&mut byte)).await,
                Ok(Ok(0)) | Ok(Err(_))
            ));
        });
        let operation =
            tokio::spawn(async move { transport.send_streaming(request(&transport)).await });
        received.await.unwrap();
        operation.abort();
        assert!(operation.await.err().unwrap().is_cancelled());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn explicit_localhost_destination_uses_the_loopback_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = client(&format!(
            "http://localhost:{}/v1",
            listener.local_addr().unwrap().port()
        ));
        let server = tokio::spawn(async move {
            let (mut socket, peer) = listener.accept().await.unwrap();
            assert!(peer.ip().is_loopback());
            read_request(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: [DONE]\n\n").await.unwrap();
        });
        let mut stream = transport
            .send_streaming(request(&transport))
            .await
            .unwrap()
            .into_body();
        assert!(stream.next().await.unwrap().is_ok());
        assert!(stream.next().await.is_none());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_connection_failure_is_not_retried() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = client(&format!("http://{}/v1", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            drop(socket);
            assert!(timeout(Duration::from_millis(150), listener.accept())
                .await
                .is_err());
        });
        let error = transport
            .send_streaming(request(&transport))
            .await
            .err()
            .unwrap();
        assert!(!format!("{error:?}").contains("127.0.0.1"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rig_requires_a_finish_reason_and_accepts_clean_eof_or_done() {
        let delta = "data: {\"id\":\"test\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n";
        let finish = "data: {\"id\":\"test\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n";
        for (suffix, success) in [
            (format!("{finish}data: [DONE]\n\n"), true),
            (finish.to_owned(), true),
            ("data: [DONE]\n\n".into(), false),
            (String::new(), false),
        ] {
            let (transport, server) = serve(format!("{delta}{suffix}").into_bytes(), SSE).await;
            let destination = transport
                .endpoint
                .as_str()
                .trim_end_matches("/chat/completions")
                .to_owned();
            let model = OpenAIConfig::new("mock-private-key")
                .with_base_url(destination)
                .connect(transport)
                .chat("mock-model");
            let mut stream = model.stream(CompletionRequest::new("hello")).unwrap();
            let mut text = String::new();
            while let Some(item) = stream.next().await {
                if let Ok(Item::Event(StreamEvent::Text { text: delta, .. })) = item {
                    text.push_str(&delta);
                }
            }
            assert_eq!(stream.finish().await.is_ok(), success);
            assert_eq!(text, "hello");
            server.await.unwrap();
        }
    }

    #[test]
    fn cleartext_is_limited_to_loopback_and_urls_are_not_in_errors() {
        for destination in [
            "http://example.com/v1",
            "http://localhost.attacker.example/v1",
            "https://user:private@example.com/v1",
            "https://example.com/v1?private-key=secret",
            "https://example.com/v1#secret",
        ] {
            let error = build_transport(
                destination,
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .unwrap_err();
            assert!(!format!("{error:?}").contains("secret"));
            assert!(!format!("{error:?}").contains("example.com"));
        }
    }

    fn snapshot(destination: &str) -> crate::model::ModelSnapshot {
        crate::model::ModelSnapshot {
            destination: destination.into(),
            model: "synthetic-model".into(),
            api_key: zeroize::Zeroizing::new("synthetic-private-key".into()),
            request: crate::model::RequestOptions::default(),
        }
    }

    #[tokio::test]
    async fn canonical_preparation_uses_rig_without_opening_a_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = snapshot(&format!("http://{}/v1", listener.local_addr().unwrap()));
        let body = canonical_request_body(
            &config,
            CompletionRequest::new("你好\n\"quoted\" 🦀")
                .preamble("Synthetic rules")
                .max_tokens(17),
        )
        .await
        .unwrap();
        assert_eq!(body["model"], "synthetic-model");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(canonical_body(&bytes).unwrap(), body);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Synthetic rules"));
        assert!(!text.contains("synthetic-private-key"));
        assert!(!text.contains("local-preflight-placeholder"));
        let oversized = canonical_request_body(
            &config,
            CompletionRequest::new("x".repeat(MAX_REQUEST_BYTES + 1)),
        )
        .await
        .unwrap();
        assert!(serde_json::to_vec(&oversized).unwrap().len() > MAX_REQUEST_BYTES);
        assert!(timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn exact_prepared_body_is_sent_and_unprepared_history_is_absent() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = snapshot(&format!("http://{}/v1", listener.local_addr().unwrap()));
        let mut body =
            canonical_request_body(&config, CompletionRequest::new("uncompressed old history"))
                .await
                .unwrap();
        body["messages"] = serde_json::json!([{"role":"user","content":"projected summary 中文"}]);
        let expected = serde_json::to_vec(&body).unwrap();
        let transport = client(&config.destination).with_prepared_body(body.clone());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let actual = read_request(&mut socket).await;
            assert_eq!(actual, expected);
            assert!(!String::from_utf8(actual)
                .unwrap()
                .contains("uncompressed old history"));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: [DONE]\n\n").await.unwrap();
        });
        let model = OpenAIConfig::new(config.api_key.as_str())
            .with_base_url(&config.destination)
            .connect(transport)
            .chat(&config.model);
        let mut stream = model
            .stream(CompletionRequest::new("uncompressed old history"))
            .unwrap();
        while stream.next().await.is_some() {}
        server.await.unwrap();
    }

    #[tokio::test]
    async fn prepared_body_cannot_change_model_tools_or_output_reserve() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = snapshot(&format!("http://{}/v1", listener.local_addr().unwrap()));
        let original =
            canonical_request_body(&config, CompletionRequest::new("hello").max_tokens(20))
                .await
                .unwrap();
        for field in ["model", "tools", "stream", "max_tokens"] {
            let mut changed = original.clone();
            changed[field] = serde_json::json!("changed");
            let transport = client(&config.destination).with_prepared_body(changed);
            let result = transport
                .send_streaming(
                    Request::builder()
                        .method("POST")
                        .uri(transport.endpoint.as_str())
                        .body(serde_json::to_vec(&original).unwrap())
                        .unwrap(),
                )
                .await;
            assert!(result.is_err(), "{field}");
            assert_eq!(transport.safe_error().unwrap().code, "CONTEXT_STALE");
        }
        assert!(timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn preflight_failure_preserves_host_error_and_never_connects() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = client(&format!("http://{}/v1", listener.local_addr().unwrap()))
            .with_preflight(|_| {
                Err(ApiError::new(
                    "CONTEXT_PROTECTED_OVERFLOW",
                    "Protected context cannot fit.",
                ))
            });
        assert!(transport.send_streaming(request(&transport)).await.is_err());
        assert_eq!(
            transport.safe_error().unwrap().code,
            "CONTEXT_PROTECTED_OVERFLOW"
        );
        assert!(timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn final_encoded_projection_has_the_actual_byte_guard() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = snapshot(&format!("http://{}/v1", listener.local_addr().unwrap()));
        let original = canonical_request_body(&config, CompletionRequest::new("original"))
            .await
            .unwrap();
        let mut projected = original.clone();
        projected["messages"] =
            serde_json::json!([{"role":"user","content":"\"".repeat(MAX_REQUEST_BYTES / 2)}]);
        assert!(serde_json::to_vec(&projected).unwrap().len() > MAX_REQUEST_BYTES);
        let transport = client(&config.destination).with_prepared_body(projected);
        let request = Request::builder()
            .method("POST")
            .uri(transport.endpoint.as_str())
            .body(serde_json::to_vec(&original).unwrap())
            .unwrap();
        assert!(transport.send_streaming(request).await.is_err());
        assert_eq!(transport.safe_error().unwrap().code, "MODEL_REQUEST_LIMIT");
        assert!(timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err());
    }

    #[test]
    fn usage_is_optional_partial_and_does_not_invent_zeros_or_add_caches() {
        let capture = UsageCapture::default();
        for frame in [
            b"data: {}\n\n".as_slice(),
            b"data: {\"usage\":null}\n\n",
            b"data: {\"usage\":{\"total_tokens\":10}}\n\n",
        ] {
            capture.frame(frame);
            assert!(capture.0.lock().unwrap().reported.is_none());
        }
        capture.frame(br#"data: {"id":"request-1","usage":{"prompt_tokens":70,"prompt_tokens_details":{"cached_tokens":40}}}

"#);
        let usage = capture.0.lock().unwrap().reported.clone().unwrap();
        assert_eq!(usage.request_id.as_deref(), Some("request-1"));
        assert_eq!(usage.input_tokens, Some(70));
        assert_eq!(usage.output_tokens, None);
        assert_eq!(usage.cached_input_tokens, Some(40));
        assert!(!usage.complete());
        capture.frame(br#"data: {"usage":{"completion_tokens":0,"completion_tokens_details":{"reasoning_tokens":0}}}

"#);
        let usage = capture.0.lock().unwrap().reported.clone().unwrap();
        assert_eq!(usage.input_tokens, Some(70));
        assert_eq!(usage.output_tokens, Some(0));
        assert_eq!(usage.reasoning_tokens, Some(0));
        assert!(usage.complete());
    }

    #[tokio::test]
    async fn streamed_usage_captures_reported_fields_and_bounded_request_id() {
        let body = b"data: {\"id\":\"completion-id\",\"usage\":{\"prompt_tokens\":23,\"completion_tokens\":7,\"prompt_tokens_details\":{\"cached_tokens\":10},\"completion_tokens_details\":{\"reasoning_tokens\":4}}}\n\ndata: [DONE]\n\n";
        let (transport, server) = serve(
            body.to_vec(),
            "Content-Type: text/event-stream\r\nX-Request-Id: req-123\r\n",
        )
        .await;
        let mut stream = transport
            .send_streaming(request(&transport))
            .await
            .unwrap()
            .into_body();
        while stream.next().await.is_some() {}
        let usage = transport.reported_usage().unwrap();
        assert_eq!(usage.request_id.as_deref(), Some("req-123"));
        assert_eq!(usage.input_tokens, Some(23));
        assert_eq!(usage.output_tokens, Some(7));
        assert_eq!(usage.cached_input_tokens, Some(10));
        assert_eq!(usage.reasoning_tokens, Some(4));
        server.await.unwrap();
        assert!(safe_request_id("https://private.example/token").is_none());
        assert!(safe_request_id(&"x".repeat(129)).is_none());
        let capture = UsageCapture::default();
        capture.reset(Some("Bearer synthetic-key"));
        capture.request_id("echo-synthetic-key");
        assert!(capture.0.lock().unwrap().request_id.is_none());
    }

    #[test]
    fn invalid_or_unknown_usage_fields_are_never_guessed() {
        let capture = UsageCapture::default();
        capture.frame(br#"data: {"usage":{"input_tokens":10,"output_tokens":5,"prompt_tokens":-1,"completion_tokens":"7"}}

"#);
        assert!(capture.0.lock().unwrap().reported.is_none());
        capture.frame(
            br#"data: {"usage":{"prompt_tokens":10,"prompt_tokens_details":{"cached_tokens":20}}}

"#,
        );
        let usage = capture.0.lock().unwrap().reported.clone().unwrap();
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.cached_input_tokens, None);
        assert_eq!(usage.output_tokens, None);
    }
    #[tokio::test]
    async fn a_body_exactly_at_the_byte_limit_is_allowed() {
        let (mut transport, server) = serve(b"data: [DONE]\n\n".to_vec(), SSE).await;
        transport.limits.request = 2; // The canonical encoding of {} is 2 bytes.
        let mut stream = transport
            .send_streaming(request(&transport))
            .await
            .unwrap()
            .into_body();
        assert!(stream.next().await.unwrap().is_ok());
        assert!(stream.next().await.is_none());
        server.await.unwrap();
    }
    #[tokio::test]
    async fn bounded_provider_overflow_is_actionable_without_echoing_error_text() {
        for (code, expected) in [
            ("context_length_exceeded", "CONTEXT_PROVIDER_OVERFLOW"),
            ("invalid_request", "MODEL_HTTP_ERROR"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let transport = client(&format!("http://{}/v1", listener.local_addr().unwrap()));
            let body = serde_json::json!({"error":{"code":code,"message":"context_length_exceeded synthetic-private-key confidential echoed prompt"}}).to_string();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                read_request(&mut socket).await;
                let response = format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            });
            let error = transport
                .send_streaming(request(&transport))
                .await
                .err()
                .unwrap();
            assert_eq!(transport.safe_error().unwrap().code, expected);
            assert!(!format!("{error:?} {error}").contains("synthetic-private-key"));
            assert!(!format!("{:?}", transport.safe_error()).contains("echoed prompt"));
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn overflow_classification_never_scans_messages_or_unbounded_bodies() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = client(&format!("http://{}/v1", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: 999999999\r\n\r\n").await.unwrap();
            let mut byte = [0];
            assert!(matches!(
                timeout(Duration::from_secs(1), socket.read(&mut byte)).await,
                Ok(Ok(0)) | Ok(Err(_))
            ));
        });
        assert!(transport.send_streaming(request(&transport)).await.is_err());
        assert_eq!(transport.safe_error().unwrap().code, "MODEL_HTTP_ERROR");
        server.await.unwrap();
    }
    #[tokio::test]
    async fn tiny_stalled_error_bodies_have_a_short_classification_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = client(&format!("http://{}/v1", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n").await.unwrap();
            let mut byte = [0];
            assert!(matches!(
                timeout(Duration::from_secs(1), socket.read(&mut byte)).await,
                Ok(Ok(0)) | Ok(Err(_))
            ));
        });
        let start = Instant::now();
        assert!(transport.send_streaming(request(&transport)).await.is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(transport.safe_error().unwrap().code, "MODEL_HTTP_ERROR");
        server.await.unwrap();
    }
}
