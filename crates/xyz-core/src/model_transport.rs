//! The only HTTP path used by model chat. Limits apply before Rig's SSE/JSON
//! parsers, and failures never retain a provider body, header, URL, or secret.
use crate::ssh::ApiError;
use bytes::Bytes;
use futures::stream;
use rig_core::{
    http_client::{
        BoxedStream, Error, HeaderMap, HttpClientExt, LazyBody, Method, MultipartForm, Request,
        Response, Result as HttpResult, StreamingResponse,
    },
    wasm_compat::WasmCompatSend,
};
use rig_reqwest::reqwest;
use std::{
    fmt,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::{timeout_at, Instant};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
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
}
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
    })
}

#[derive(Clone, Copy, Debug)]
enum Failure {
    Unsupported,
    Destination,
    RequestLimit,
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
            Self::ResponseLimit => "The model response exceeded its byte limit.",
            Self::ChunkLimit => "The model response chunk exceeded its byte limit.",
            Self::LineLimit => "The model response line exceeded its byte limit.",
            Self::FrameLimit => "The model response event exceeded its byte limit.",
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

// Share only fixed application-safe classifications across the client clone
// moved into Rig. No network error object or provider material is retained.
#[derive(Clone, Default)]
struct SafeErrors(Arc<Mutex<Option<ApiError>>>);
impl SafeErrors {
    fn record(&self, error: &Error) {
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
                "The model provider rejected the request. No automatic retry occurs.",
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
impl BoundedHttpClient {
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
                if body.len() > transport.limits.request {
                    return Err(failed(Failure::RequestLimit));
                }
                let deadline = Instant::now() + transport.total;
                let response = timeout_at(
                    deadline.min(Instant::now() + transport.idle),
                    transport
                        .client
                        .post(url)
                        .headers(parts.headers)
                        .header("accept-encoding", "identity")
                        .body(body)
                        .send(),
                )
                .await
                .map_err(|_| deadline_error(deadline))?
                .map_err(network)?;
                if !response.status().is_success() {
                    // Never read any error body, even when Content-Length is absent
                    // or claims a tiny size. Likewise do not retain response headers.
                    return Err(Error::non_success_with_details(
                        response.status(),
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
                let state = BodyState {
                    response: Some(response),
                    pending: Bytes::new(),
                    parser: FrameGuard::new(transport.limits),
                    total_read: 0,
                    deadline,
                    idle: transport.idle,
                    ended: false,
                    errors: transport.errors.clone(),
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
    async fn read_request(socket: &mut TcpStream) {
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
                    break;
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
                Duration::from_millis(120),
                Duration::from_millis(50),
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
            assert!(start.elapsed() < Duration::from_millis(500));
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
}
