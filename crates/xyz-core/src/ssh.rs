//! One SSH connection and one visible shell PTY per application instance.
//! Credentials are accepted only by the dedicated connection request. They are
//! never serialized, logged, persisted, or returned through an event.
use base64::{engine::general_purpose::STANDARD, Engine};
use russh::{
    client,
    keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate},
    ChannelMsg, Disconnect,
};
use serde::{Deserialize, Serialize};
#[cfg(not(windows))]
use std::io::Read;
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

pub const MAX_INPUT_BYTES: usize = 16 * 1024;
pub const MAX_OUTPUT_CHUNK: usize = 16 * 1024;
const INPUT_QUEUE_FRAMES: usize = 16;
const OUTPUT_WINDOW_BYTES: usize = 256 * 1024;
const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);
// The remote receiver waits longer, so a late/absent M marker cannot release a
// body after the receiver could have returned to the interactive shell.
const RECEIVER_TIMEOUT: Duration = Duration::from_secs(5);
const RECEIVER_AUTHORITY_POLL: Duration = Duration::from_millis(25);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiError {
    pub code: String,
    pub safe_message: String,
    pub retry: String,
}
impl ApiError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            safe_message: message.into(),
            retry: "after_user_action".into(),
        }
    }
    fn receiver_unknown() -> Self {
        Self::new(
            "DELIVERY_UNKNOWN",
            "Shell 命令接收过程未完成或无法确认，这不代表原命令已执行。自动输入已停止，请检查终端输出并重新连接；不会自动重发。",
        )
    }
    fn unknown() -> Self {
        Self::new(
            "DELIVERY_UNKNOWN",
            "The write may have reached the server. Do not automatically resend it.",
        )
    }
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.safe_message)
    }
}
impl std::error::Error for ApiError {}

#[derive(Deserialize)]
#[serde(transparent)]
pub struct Secret(String);
impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Authentication {
    Password {
        password: Secret,
    },
    PrivateKey {
        private_key_path: String,
        passphrase: Option<Secret>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: Authentication,
    pub session_only: bool,
    pub size: TerminalSize,
}
impl ConnectRequest {
    pub fn validate(&self) -> Result<(), ApiError> {
        self.size.validate()?;
        if !self.session_only {
            return Err(ApiError::new(
                "CREDENTIAL_MODE_REQUIRED",
                "Explicit session-only credential storage is required for this milestone.",
            ));
        }
        if self.host.is_empty()
            || self.host.len() > 253
            || self
                .host
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '@'))
            || self.port == 0
        {
            return Err(ApiError::new(
                "SSH_CONFIG_INVALID",
                "Enter a hostname or IP address and a port from 1 to 65535.",
            ));
        }
        if self.username.is_empty()
            || self.username.len() > 256
            || self.username.chars().any(char::is_control)
        {
            return Err(ApiError::new(
                "SSH_CONFIG_INVALID",
                "Enter a valid SSH username.",
            ));
        }
        match &self.auth {
            Authentication::Password { password } if password.0.len() > 16384 => {
                return Err(ApiError::new(
                    "SSH_CREDENTIAL_INVALID",
                    "The credential is too large.",
                ))
            }
            Authentication::PrivateKey {
                private_key_path,
                passphrase,
            } => {
                if !Path::new(private_key_path).is_absolute() || private_key_path.len() > 4096 {
                    return Err(ApiError::new(
                        "LOCAL_PATH_INVALID",
                        "Choose an absolute private-key path.",
                    ));
                }
                if passphrase.as_ref().is_some_and(|p| p.0.len() > 16384) {
                    return Err(ApiError::new(
                        "SSH_CREDENTIAL_INVALID",
                        "The credential is too large.",
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalSize {
    pub cols: u32,
    pub rows: u32,
}
impl TerminalSize {
    pub fn validate(self) -> Result<(), ApiError> {
        if (1..=1000).contains(&self.cols) && (1..=1000).contains(&self.rows) {
            Ok(())
        } else {
            Err(ApiError::new(
                "INPUT_INVALID",
                "Terminal dimensions must be from 1 to 1000.",
            ))
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionRef {
    pub session_id: String,
    pub generation: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Disconnected,
    Connecting,
    AwaitingHostKey,
    Authenticating,
    Connected,
    Closing,
    Error,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSnapshot {
    #[serde(default)]
    pub profile_id: Option<String>,
    pub revision: String,
    pub session: Option<SessionRef>,
    pub state: SessionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_key: Option<HostKeyChallenge>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostKeyChallenge {
    pub challenge_id: String,
    pub session: SessionRef,
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_fingerprint: Option<String>,
    pub changed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostKeyDecision {
    pub challenge_id: String,
    pub expected_fingerprint: String,
    pub decision: TrustDecision,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustDecision {
    Accept,
    Reject,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WriteReceipt {
    pub transport_seq: String,
    pub delivery: String,
    pub remote_outcome: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
// Public event variants retain their stable Rust shape as well as JSON shape.
// The bounded terminal-output window limits the dominant queued payload.
#[allow(clippy::large_enum_variant)]
pub enum SshEvent {
    #[serde(rename = "session.state")]
    State { snapshot: SessionSnapshot },
    #[serde(rename = "host_key.challenge")]
    HostKey { challenge: HostKeyChallenge },
    #[serde(rename = "terminal.output")]
    Output {
        session: SessionRef,
        seq: String,
        data_base64: String,
        stream: String,
    },
    #[serde(rename = "session.exit")]
    Exit {
        session: SessionRef,
        exit_status: u32,
    },
}
pub type EventSink = Arc<dyn Fn(SshEvent) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct SessionManager {
    inner: Arc<Inner>,
}
struct Inner {
    state: Mutex<ManagerState>,
    sink: EventSink,
    observation: crate::observation::ObservationBuffer,
    shell_log: Mutex<Option<crate::shell_log::ShellLogService>>,
    shell_integration: crate::shell_integration::ShellIntegrationService,
    trust: Result<Option<Arc<crate::storage::trust::KnownHostsStore>>, ApiError>,
}
struct ManagerState {
    snapshot: SessionSnapshot,
    revision: u64,
    generation: u64,
    live: Option<LiveSession>,
    trusted: HashMap<(String, u16), String>,
    challenge: Option<(String, String, oneshot::Sender<bool>)>,
}
struct LiveSession {
    session: SessionRef,
    commands: mpsc::Sender<Command>,
    cancel: watch::Sender<bool>,
    output_window: Arc<Semaphore>,
    unacked: VecDeque<(u64, OwnedSemaphorePermit)>,
}
enum Command {
    AiWrite {
        permit: Box<crate::execution::DispatchPermit>,
        reply: oneshot::Sender<Result<WriteReceipt, ApiError>>,
    },
    Write {
        data: Zeroizing<Vec<u8>>,
        reply: oneshot::Sender<Result<WriteReceipt, ApiError>>,
    },
    Resize(TerminalSize),
}
impl SessionManager {
    pub fn new(sink: EventSink) -> Self {
        Self::with_trust(sink, Ok(None))
    }
    pub fn with_persistent_trust(
        sink: EventSink,
        root: Result<std::path::PathBuf, ApiError>,
    ) -> Self {
        let trust = root.and_then(|root| {
            crate::storage::trust::KnownHostsStore::open(&root)
                .map(Arc::new)
                .map(Some)
                .map_err(crate::host_storage::storage_error)
        });
        Self::with_trust(sink, trust)
    }
    fn with_trust(
        sink: EventSink,
        trust: Result<Option<Arc<crate::storage::trust::KnownHostsStore>>, ApiError>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                sink,
                trust,
                observation: crate::observation::ObservationBuffer::default(),
                shell_log: Mutex::new(None),
                shell_integration: crate::shell_integration::ShellIntegrationService::new(),
                state: Mutex::new(ManagerState {
                    snapshot: SessionSnapshot {
                        profile_id: None,
                        revision: "0".into(),
                        session: None,
                        state: SessionState::Disconnected,
                        host: None,
                        port: None,
                        username: None,
                        error: None,
                        host_key: None,
                    },
                    revision: 0,
                    generation: 0,
                    live: None,
                    trusted: HashMap::new(),
                    challenge: None,
                }),
            }),
        }
    }
    fn lock(&self) -> MutexGuard<'_, ManagerState> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Attach once during host initialization, before a connection is opened.
    pub fn attach_shell_log(&self, service: crate::shell_log::ShellLogService) {
        self.inner.shell_integration.attach_log(service.clone());
        *self
            .inner
            .shell_log
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(service);
    }
    fn shell_log(&self) -> Option<crate::shell_log::ShellLogService> {
        self.inner
            .shell_log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn shell_integration(&self) -> crate::shell_integration::ShellIntegrationService {
        self.inner.shell_integration.clone()
    }
    /// Compatibility endpoint: preparation belongs to connection startup now.
    /// Never inject setup into an existing foreground, even if a stale client
    /// submits its old readiness checkbox.
    pub async fn install_shell_integration(
        &self,
        session: &SessionRef,
        _readiness_confirmed: bool,
    ) -> Result<crate::shell_integration::IntegrationStatus, ApiError> {
        {
            let state = self.lock();
            require_connected(&state, session)?;
        }
        Ok(self.inner.shell_integration.status())
    }
    pub fn observation(&self) -> crate::observation::ObservationBuffer {
        self.inner.observation.clone()
    }
    pub(crate) fn begin_task_context(
        &self,
        session: &SessionRef,
        model_secret: String,
    ) -> Result<(SessionSnapshot, crate::observation::ObservationBuffer), ApiError> {
        let state = self.lock();
        require_connected(&state, session)?;
        self.inner
            .observation
            .begin(session.clone(), vec![model_secret]);
        Ok((
            state.snapshot.clone(),
            self.inner.observation.detached_redactor(),
        ))
    }
    pub fn snapshot(&self) -> SessionSnapshot {
        self.lock().snapshot.clone()
    }
    fn emit(&self, event: SshEvent) -> Result<(), ApiError> {
        if (self.inner.sink)(event) {
            Ok(())
        } else {
            Err(ApiError::new(
                "UI_UNAVAILABLE",
                "The terminal display is unavailable; the session is being closed.",
            ))
        }
    }
    pub fn connect(&self, request: ConnectRequest) -> Result<SessionSnapshot, ApiError> {
        self.connect_with_profile(request, None)
    }
    /// The host calls this only after resolving a saved profile; raw connect IPC
    /// cannot assign the immutable connection/profile association.
    pub fn connect_profile(
        &self,
        request: ConnectRequest,
        profile_id: String,
    ) -> Result<SessionSnapshot, ApiError> {
        self.connect_with_profile(request, Some(profile_id))
    }
    fn connect_with_profile(
        &self,
        request: ConnectRequest,
        profile_id: Option<String>,
    ) -> Result<SessionSnapshot, ApiError> {
        request.validate()?;
        if let Err(error) = &self.inner.trust {
            return Err(error.clone());
        }
        let (commands, receiver) = mpsc::channel(INPUT_QUEUE_FRAMES);
        let (cancel, cancelled) = watch::channel(false);
        let snapshot = {
            let mut state = self.lock();
            if state.live.is_some() {
                return Err(ApiError::new(
                    "SESSION_BUSY",
                    "Disconnect the current server before opening another connection.",
                ));
            }
            self.inner.observation.reset_capture_preserving_secrets();
            match &request.auth {
                Authentication::Password { password } => {
                    self.inner.observation.add_known_secret(&password.0)?
                }
                Authentication::PrivateKey {
                    passphrase: Some(p),
                    ..
                } => self.inner.observation.add_known_secret(&p.0)?,
                _ => {}
            }
            state.generation += 1;
            state.revision += 1;
            let session = SessionRef {
                session_id: Uuid::new_v4().to_string(),
                generation: state.generation.to_string(),
            };
            state.live = Some(LiveSession {
                session: session.clone(),
                commands,
                cancel,
                output_window: Arc::new(Semaphore::new(OUTPUT_WINDOW_BYTES)),
                unacked: VecDeque::new(),
            });
            state.snapshot = SessionSnapshot {
                profile_id,
                revision: state.revision.to_string(),
                session: Some(session),
                state: SessionState::Connecting,
                host: Some(request.host.clone()),
                port: Some(request.port),
                username: Some(request.username.clone()),
                error: None,
                host_key: None,
            };
            state.snapshot.clone()
        };
        let session = snapshot
            .session
            .clone()
            .ok_or_else(|| ApiError::new("INTERNAL_STATE", "No session identity was created."))?;
        let manager = self.clone();
        tokio::spawn(async move {
            manager.run(session, request, receiver, cancelled).await;
        });
        // A snapshot revision prevents late command responses overwriting newer events.
        let _ = self.emit(SshEvent::State {
            snapshot: snapshot.clone(),
        });
        Ok(snapshot)
    }
    fn change_state(
        &self,
        session: &SessionRef,
        next: SessionState,
        error: Option<ApiError>,
    ) -> Result<(), ApiError> {
        let snapshot = {
            let mut state = self.lock();
            let live = require_live(&state, session)?;
            if *live.cancel.borrow() {
                return Err(ApiError::new(
                    "SESSION_CLOSING",
                    "The SSH connection is closing.",
                ));
            }
            state.revision += 1;
            state.snapshot.revision = state.revision.to_string();
            state.snapshot.state = next;
            state.snapshot.error = error;
            if next != SessionState::AwaitingHostKey {
                state.snapshot.host_key = None;
            }
            state.snapshot.clone()
        };
        self.emit(SshEvent::State { snapshot })
    }
    pub fn confirm_host_key(&self, request: HostKeyDecision) -> Result<(), ApiError> {
        let sender = {
            let mut state = self.lock();
            let Some((id, fingerprint, _)) = &state.challenge else {
                return Err(ApiError::new(
                    "HOST_KEY_STALE",
                    "This host-key request has expired.",
                ));
            };
            if id != &request.challenge_id || fingerprint != &request.expected_fingerprint {
                return Err(ApiError::new(
                    "HOST_KEY_STALE",
                    "The challenge or fingerprint does not match the current server.",
                ));
            }
            state.challenge.take().map(|(_, _, sender)| sender)
        };
        sender
            .ok_or_else(|| ApiError::new("HOST_KEY_STALE", "This host-key request has expired."))?
            .send(matches!(request.decision, TrustDecision::Accept))
            .map_err(|_| {
                ApiError::new(
                    "HOST_KEY_STALE",
                    "The connection no longer needs this confirmation.",
                )
            })
    }
    pub fn disconnect(&self, session: &SessionRef) -> Result<(), ApiError> {
        let snapshot = {
            let mut state = self.lock();
            let live = require_live(&state, session)?;
            let cancel = live.cancel.clone();
            state.revision += 1;
            state.snapshot.revision = state.revision.to_string();
            state.snapshot.state = SessionState::Closing;
            state.snapshot.host_key = None;
            let snapshot = state.snapshot.clone();
            let _ = cancel.send(true);
            snapshot
        };
        self.emit(SshEvent::State { snapshot })
    }
    pub fn resize(&self, session: &SessionRef, size: TerminalSize) -> Result<(), ApiError> {
        size.validate()?;
        let state = self.lock();
        let live = require_connected(&state, session)?;
        live.commands
            .try_send(Command::Resize(size))
            .map_err(queue_error)
    }
    pub async fn write(
        &self,
        session: &SessionRef,
        encoded: &str,
    ) -> Result<WriteReceipt, ApiError> {
        let data = decode_input(encoded)?;
        let (sender, response) = oneshot::channel();
        {
            let state = self.lock();
            let live = require_connected(&state, session)?;
            live.commands
                .try_send(Command::Write {
                    data,
                    reply: sender,
                })
                .map_err(queue_error)?;
        }
        // Never retry after queue acceptance, including if this caller is cancelled.
        response.await.unwrap_or_else(|_| Err(ApiError::unknown()))
    }
    pub(crate) async fn write_ai(
        &self,
        session: &SessionRef,
        permit: crate::execution::DispatchPermit,
    ) -> Result<WriteReceipt, ApiError> {
        let (sender, response) = oneshot::channel();
        let started = permit.started.clone();
        {
            let state = self.lock();
            let live = require_connected(&state, session)?;
            live.commands
                .try_send(Command::AiWrite {
                    permit: Box::new(permit),
                    reply: sender,
                })
                .map_err(queue_error)?;
        }
        response.await.unwrap_or_else(|_| {
            if started.load(std::sync::atomic::Ordering::Acquire) {
                Err(ApiError::unknown())
            } else {
                Err(ApiError::new(
                    "NOT_SENT",
                    "The SSH actor closed before this AI frame started. Nothing was sent.",
                ))
            }
        })
    }
    pub fn ack_output(&self, session: &SessionRef, seq: &str) -> Result<(), ApiError> {
        let seq: u64 = seq
            .parse()
            .map_err(|_| ApiError::new("INPUT_INVALID", "Invalid output acknowledgement."))?;
        let mut state = self.lock();
        // A delayed renderer callback may outlive disconnect or reconnect. It
        // must not release credits for another generation or surface a false
        // connection failure after successful final output rendering.
        if state
            .live
            .as_ref()
            .is_none_or(|live| &live.session != session)
        {
            return Ok(());
        }
        if let Some(live) = state.live.as_mut() {
            // Future acknowledgements must not release bytes that are not displayed.
            if live.unacked.back().is_some_and(|(last, _)| seq > *last) {
                return Err(ApiError::new(
                    "INPUT_INVALID",
                    "Output acknowledgement exceeds the published cursor.",
                ));
            }
            while live
                .unacked
                .front()
                .is_some_and(|(pending, _)| *pending <= seq)
            {
                live.unacked.pop_front();
            }
        }
        Ok(())
    }
    async fn publish_output(
        &self,
        session: &SessionRef,
        seq: &mut u64,
        bytes: &[u8],
        stream: &str,
    ) -> Result<(), ApiError> {
        for chunk in bytes.chunks(MAX_OUTPUT_CHUNK) {
            let semaphore = { require_live(&self.lock(), session)?.output_window.clone() };
            let permit = semaphore
                .acquire_many_owned(chunk.len() as u32)
                .await
                .map_err(|_| ApiError::new("SESSION_NOT_CONNECTED", "The terminal is closed."))?;
            *seq += 1;
            {
                let mut state = self.lock();
                require_live(&state, session)?;
                if let Some(live) = state.live.as_mut() {
                    live.unacked.push_back((*seq, permit));
                }
            }
            // Metadata is passive and must not become an AI observation. Keep the
            // original bytes/seq for the visible terminal and its backpressure ACK.
            self.inner
                .shell_integration
                .ingest_with_observer(session, chunk, |visible| {
                    self.inner.observation.push(session, visible);
                });
            self.emit(SshEvent::Output {
                session: session.clone(),
                seq: seq.to_string(),
                data_base64: STANDARD.encode(chunk),
                stream: stream.into(),
            })?;
        }
        Ok(())
    }
    async fn run(
        &self,
        session: SessionRef,
        request: ConnectRequest,
        mut receiver: mpsc::Receiver<Command>,
        mut cancelled: watch::Receiver<bool>,
    ) {
        let mut output_seq = 0;
        let operation = self.run_connected(
            &session,
            request,
            &mut receiver,
            cancelled.clone(),
            &mut output_seq,
        );
        let result = tokio::select! { biased; _ = cancelled.changed() => Ok(()), result = operation => result };
        // Anything still in the queue is provably not sent. An in-flight write is
        // reported unknown by its dropped response, never silently replayed.
        receiver.close();
        while let Some(command) = receiver.recv().await {
            if let Command::Write { reply, .. } | Command::AiWrite { reply, .. } = command {
                let _ = reply.send(Err(ApiError::new(
                    "NOT_SENT",
                    "The session closed before this frame was written.",
                )));
            }
        }
        let snapshot = {
            let mut state = self.lock();
            if state
                .live
                .as_ref()
                .is_none_or(|live| live.session != session)
            {
                return;
            }
            state.live = None;
            state.challenge = None;
            state.revision += 1;
            state.snapshot.revision = state.revision.to_string();
            state.snapshot.state = if result.is_ok() {
                SessionState::Disconnected
            } else {
                SessionState::Error
            };
            state.snapshot.error = result.err();
            state.snapshot.host_key = None;
            state.snapshot.clone()
        };
        self.inner.shell_integration.disconnect(&session);
        if let Some(log) = self.shell_log() {
            log.disconnect(&session);
        }
        let _ = self.emit(SshEvent::State { snapshot });
    }
    async fn run_connected(
        &self,
        session_ref: &SessionRef,
        request: ConnectRequest,
        receiver: &mut mpsc::Receiver<Command>,
        cancelled: watch::Receiver<bool>,
        output_seq: &mut u64,
    ) -> Result<(), ApiError> {
        let handler = ClientHandler {
            manager: self.clone(),
            session: session_ref.clone(),
            host: request.host.clone(),
            port: request.port,
            cancelled,
        };
        let config = Arc::new(client::Config {
            inactivity_timeout: None,
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 3,
            channel_buffer_size: 16,
            ..Default::default()
        });
        // TCP establishment is bounded separately; human host-key review gets 120s.
        let socket = tokio::time::timeout(
            NETWORK_TIMEOUT,
            tokio::net::TcpStream::connect((request.host.as_str(), request.port)),
        )
        .await
        .map_err(|_| {
            ApiError::new(
                "SSH_CONNECT_TIMEOUT",
                "The SSH server did not respond in time.",
            )
        })?
        .map_err(|_| {
            ApiError::new(
                "SSH_CONNECT_FAILED",
                "Unable to connect to the SSH server. Check the host, port, and network.",
            )
        })?;
        // A duplicate socket handle guarantees cancellation closes the actual
        // transport, even while russh owns an independently spawned event loop.
        let socket = socket.into_std().map_err(|_| {
            ApiError::new("SSH_CONNECT_FAILED", "Unable to prepare the SSH transport.")
        })?;
        let _shutdown = ShutdownOnDrop(socket.try_clone().map_err(|_| {
            ApiError::new(
                "SSH_CONNECT_FAILED",
                "Unable to prepare transport cancellation.",
            )
        })?);
        let socket = tokio::net::TcpStream::from_std(socket).map_err(|_| {
            ApiError::new("SSH_CONNECT_FAILED", "Unable to prepare the SSH transport.")
        })?;
        let mut session = tokio::time::timeout(
            Duration::from_secs(150),
            client::connect_stream(config, socket, handler),
        )
        .await
        .map_err(|_| {
            ApiError::new(
                "SSH_HANDSHAKE_TIMEOUT",
                "SSH negotiation or host-key confirmation timed out.",
            )
        })?
        .map_err(|_| {
            ApiError::new(
                "SSH_HANDSHAKE_FAILED",
                "SSH negotiation failed or the host key was not trusted.",
            )
        })?;
        self.change_state(session_ref, SessionState::Authenticating, None)?;
        let auth = async {
            match request.auth {
                Authentication::Password { mut password } => session
                    .authenticate_password(request.username, std::mem::take(&mut password.0))
                    .await
                    .map_err(|_| ApiError::new("SSH_AUTH_FAILED", "SSH authentication failed.")),
                Authentication::PrivateKey {
                    private_key_path,
                    passphrase,
                } => {
                    let key = tokio::task::spawn_blocking(move || {
                        read_private_key(
                            &private_key_path,
                            passphrase.as_ref().map(|p| p.0.as_str()),
                        )
                    })
                    .await
                    .map_err(|_| {
                        ApiError::new("SSH_KEY_FAILED", "Unable to load the selected private key.")
                    })??;
                    let hash = session
                        .best_supported_rsa_hash()
                        .await
                        .map_err(|_| {
                            ApiError::new(
                                "SSH_AUTH_FAILED",
                                "Unable to negotiate the key signature algorithm.",
                            )
                        })?
                        .flatten();
                    session
                        .authenticate_publickey(
                            request.username,
                            PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                        )
                        .await
                        .map_err(|_| {
                            ApiError::new("SSH_AUTH_FAILED", "SSH key authentication failed.")
                        })
                }
            }
        };
        let auth_result = tokio::time::timeout(NETWORK_TIMEOUT, auth)
            .await
            .map_err(|_| ApiError::new("SSH_AUTH_TIMEOUT", "SSH authentication timed out."))??;
        if !auth_result.success() {
            return Err(ApiError::new("SSH_AUTH_FAILED", "SSH authentication was rejected. Check the credentials or required authentication method."));
        }
        let mut channel = tokio::time::timeout(NETWORK_TIMEOUT, session.channel_open_session())
            .await
            .map_err(|_| ApiError::new("SSH_PTY_FAILED", "Opening the SSH session timed out."))?
            .map_err(|_| {
                ApiError::new("SSH_PTY_FAILED", "The server refused a session channel.")
            })?;
        channel
            .request_pty(
                true,
                "xterm-256color",
                request.size.cols,
                request.size.rows,
                0,
                0,
                &[],
            )
            .await
            .map_err(|_| ApiError::new("SSH_PTY_FAILED", "Unable to request a terminal."))?;
        self.wait_request_success(session_ref, &mut channel, output_seq, "SSH_PTY_FAILED")
            .await?;
        // Register the protocol BEFORE any startup output can arrive. The code
        // is the command of this visible PTY, never data written to its stdin.
        let startup = self.inner.shell_integration.prepare_startup(session_ref)?;
        channel
            .exec(true, startup.as_bytes())
            .await
            .map_err(|_| ApiError::new("SSH_SHELL_FAILED", "无法启动当前终端的交互 Shell。"))?;
        if !self
            .wait_exec_success(session_ref, &mut channel, output_seq)
            .await?
        {
            // An explicit refusal proves that this exec request did not start.
            // Only then may we request the ordinary manual shell on SAME channel.
            self.inner.shell_integration.startup_unavailable(
                session_ref,
                "SHELL_STARTUP_REFUSED",
                "服务器不支持自动准备 Shell，已切换为手动终端。不会向终端输入启动脚本。",
            );
            channel
                .request_shell(true)
                .await
                .map_err(|_| ApiError::new("SSH_SHELL_FAILED", "服务器拒绝打开手动终端。"))?;
            self.wait_request_success(session_ref, &mut channel, output_seq, "SSH_SHELL_FAILED")
                .await?;
        }
        self.change_state(session_ref, SessionState::Connected, None)?;
        let mut transport_seq = 0u64;
        // A silent startup/password prompt must still publish a terminal
        // preparation timeout to watchers without sending any terminal input.
        let preparation_deadline = tokio::time::sleep(Duration::from_secs(15));
        tokio::pin!(preparation_deadline);
        let mut preparation_pending = true;
        loop {
            tokio::select! {
                _ = &mut preparation_deadline, if preparation_pending => {
                    preparation_pending = false;
                    self.inner.shell_integration.status();
                }

                command = receiver.recv() => match command {
                    Some(Command::Write { data, reply }) => {
                        // The sole writer serializes frames; it does not arbitrate human/AI intent.
                        transport_seq += 1;
                        let write = tokio::time::timeout(NETWORK_TIMEOUT, channel.data(&data[..])).await;
                        let success = matches!(write, Ok(Ok(())));
                        let receipt = if success { Ok(WriteReceipt { transport_seq: transport_seq.to_string(), delivery: "write_accepted".into(), remote_outcome: "unknown".into() }) } else { Err(ApiError::unknown()) };
                        let _ = reply.send(receipt);
                        if !success { return Err(ApiError::unknown()); }
                    }
                    Some(Command::AiWrite { permit, reply }) => {
                        // Final gate is in this single SSH actor, at dequeue time.
                        // Cancellation/session/expiry/approval are checked again here.
                        let validated = permit.validate_before_begin().await;
                        let began = validated.and_then(|_| {
                            let state = self.lock();
                            require_connected(&state,session_ref).and_then(|_| permit.begin(session_ref))
                        });
                        if began.is_err() || permit.notify_started().is_err() {
                            let _ = reply.send(Err(ApiError::new("NOT_SENT", "The queued AI action expired, was cancelled, or no longer matches its approval. Nothing was sent.")));
                            continue;
                        }
                        transport_seq += 1;
                        let write = tokio::time::timeout(NETWORK_TIMEOUT, channel.data(&permit.bytes[..])).await;
                        if !matches!(write, Ok(Ok(()))) {
                            let _ = reply.send(Err(ApiError::unknown()));
                            return Err(ApiError::unknown());
                        }
                        if let Some(wire) = &permit.wire {
                            // The small kickoff contains no command body. Keep
                            // consuming the same PTY output until its exact M
                            // marker proves that the bounded, quiet receiver is
                            // waiting. Do not dequeue human/AI input in between.
                            let deadline = tokio::time::Instant::now() + RECEIVER_TIMEOUT;
                            let receiving = self.wait_receiver_ready(
                                session_ref, &mut channel, output_seq, &permit, deadline,
                            ).await;
                            let released = receiving.and_then(|_| self.release_receiver_body(session_ref, &permit, deadline));
                            if released.is_err() {
                                // Kickoff delivery is already uncertain. Close
                                // rather than letting later input enter a stale
                                // receiver; neither stage is ever retried.
                                let _ = reply.send(Err(ApiError::receiver_unknown()));
                                return Err(ApiError::receiver_unknown());
                            }
                            let write = tokio::time::timeout_at(deadline, channel.data(&wire[..])).await;
                            if !matches!(write, Ok(Ok(()))) {
                                let _ = reply.send(Err(ApiError::receiver_unknown()));
                                return Err(ApiError::receiver_unknown());
                            }
                        }
                        let _ = reply.send(Ok(WriteReceipt {
                            transport_seq: transport_seq.to_string(),
                            delivery: "write_accepted".into(),
                            remote_outcome: "unknown".into(),
                        }));
                    }
                    Some(Command::Resize(size)) => {
                        tokio::time::timeout(NETWORK_TIMEOUT, channel.window_change(size.cols, size.rows, 0, 0)).await.map_err(|_| ApiError::new("SSH_RESIZE_FAILED", "Terminal resize timed out."))?.map_err(|_| ApiError::new("SSH_RESIZE_FAILED", "The terminal resize could not be sent."))?;
                    }
                    None => break,
                },
                message = channel.wait() => {
                    if !self.handle_channel_message(session_ref, message, output_seq).await? { break; }
                }
            }
        }
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            session.disconnect(Disconnect::ByApplication, "", "en"),
        )
        .await;
        Ok(())
    }
    fn release_receiver_body(
        &self,
        session: &SessionRef,
        permit: &crate::execution::DispatchPermit,
        deadline: tokio::time::Instant,
    ) -> Result<(), ApiError> {
        let state = self.lock();
        require_connected(&state, session)?;
        if tokio::time::Instant::now() >= deadline {
            return Err(ApiError::receiver_unknown());
        }
        permit.begin_receiver_body(session, || {
            self.inner
                .shell_integration
                .consume_receiver(session, permit.action_id())
        })
    }
    async fn wait_receiver_ready(
        &self,
        session: &SessionRef,
        channel: &mut russh::Channel<client::Msg>,
        seq: &mut u64,
        permit: &crate::execution::DispatchPermit,
        deadline: tokio::time::Instant,
    ) -> Result<(), ApiError> {
        // The enclosing timeout also bounds output backpressure while an M
        // marker is arriving. A cancelled owner must not wait for more output.
        tokio::time::timeout_at(deadline, async {
            loop {
                permit.validate_receiver_body(session)?;
                require_connected(&self.lock(), session)?;
                if tokio::time::Instant::now() >= deadline {
                    return Err(ApiError::receiver_unknown());
                }
                if self
                    .inner
                    .shell_integration
                    .receiver_ready(session, permit.action_id())
                {
                    return Ok(());
                }
                tokio::select! {
                    _ = tokio::time::sleep(RECEIVER_AUTHORITY_POLL) => {},
                    message = channel.wait() => {
                        if !self.handle_channel_message(session, message, seq).await? {
                            return Err(ApiError::receiver_unknown());
                        }
                    }
                }
            }
        })
        .await
        .map_err(|_| ApiError::receiver_unknown())?
    }
    async fn wait_exec_success(
        &self,
        session: &SessionRef,
        channel: &mut russh::Channel<client::Msg>,
        seq: &mut u64,
    ) -> Result<bool, ApiError> {
        tokio::time::timeout(NETWORK_TIMEOUT, async {
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Success) => return Ok(true),
                    Some(ChannelMsg::Failure) => return Ok(false),
                    None | Some(ChannelMsg::Close) => {
                        return Err(ApiError::new(
                            "SSH_SHELL_FAILED",
                            "Shell 启动时连接已关闭；不会重试或注入输入。",
                        ))
                    }
                    message => {
                        if !self.handle_channel_message(session, message, seq).await? {
                            return Err(ApiError::new(
                                "SSH_SHELL_FAILED",
                                "Shell 启动时连接已关闭。",
                            ));
                        }
                    }
                }
            }
        })
        .await
        .map_err(|_| {
            ApiError::new(
                "SSH_SHELL_FAILED",
                "服务器未确认 Shell 启动；结果未知，不会自动重试。",
            )
        })?
    }
    async fn wait_request_success(
        &self,
        session: &SessionRef,
        channel: &mut russh::Channel<client::Msg>,
        seq: &mut u64,
        code: &str,
    ) -> Result<(), ApiError> {
        tokio::time::timeout(NETWORK_TIMEOUT, async {
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Success) => return Ok(()),
                    Some(ChannelMsg::Failure) | None | Some(ChannelMsg::Close) => {
                        return Err(ApiError::new(
                            code,
                            "The server refused the requested terminal or shell.",
                        ))
                    }
                    message => {
                        if !self.handle_channel_message(session, message, seq).await? {
                            return Err(ApiError::new(
                                code,
                                "The server closed the requested terminal or shell.",
                            ));
                        }
                    }
                }
            }
        })
        .await
        .map_err(|_| {
            ApiError::new(
                code,
                "The server did not acknowledge the terminal or shell request.",
            )
        })?
    }
    async fn handle_channel_message(
        &self,
        session: &SessionRef,
        message: Option<ChannelMsg>,
        seq: &mut u64,
    ) -> Result<bool, ApiError> {
        match message {
            Some(ChannelMsg::Data { data }) => {
                self.publish_output(session, seq, &data, "stdout").await?
            }
            Some(ChannelMsg::ExtendedData { data, .. }) => {
                self.publish_output(session, seq, &data, "stderr").await?
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => self.emit(SshEvent::Exit {
                session: session.clone(),
                exit_status,
            })?,
            Some(ChannelMsg::Close) | None => return Ok(false),
            _ => {}
        }
        Ok(true)
    }
}

fn require_live<'a>(
    state: &'a ManagerState,
    session: &SessionRef,
) -> Result<&'a LiveSession, ApiError> {
    let live = state.live.as_ref().ok_or_else(|| {
        ApiError::new(
            "SESSION_NOT_CONNECTED",
            "There is no active SSH connection.",
        )
    })?;
    if &live.session != session {
        return Err(ApiError::new(
            "SESSION_STALE",
            "This request belongs to an old SSH connection.",
        ));
    }
    Ok(live)
}
fn require_connected<'a>(
    state: &'a ManagerState,
    session: &SessionRef,
) -> Result<&'a LiveSession, ApiError> {
    let live = require_live(state, session)?;
    if state.snapshot.state != SessionState::Connected || *live.cancel.borrow() {
        return Err(ApiError::new(
            "SESSION_NOT_CONNECTED",
            "The SSH terminal is not connected.",
        ));
    }
    Ok(live)
}
fn queue_error<T>(error: mpsc::error::TrySendError<T>) -> ApiError {
    match error {
        mpsc::error::TrySendError::Full(_) => ApiError::new(
            "BACKPRESSURE",
            "The terminal input queue is full. This frame was not accepted.",
        ),
        mpsc::error::TrySendError::Closed(_) => ApiError::new(
            "SESSION_NOT_CONNECTED",
            "The SSH terminal is closed. This frame was not accepted.",
        ),
    }
}
fn decode_input(encoded: &str) -> Result<Zeroizing<Vec<u8>>, ApiError> {
    if encoded.len() > MAX_INPUT_BYTES.div_ceil(3) * 4 {
        return Err(ApiError::new(
            "INPUT_TOO_LARGE",
            "Terminal input must be at most 16 KiB per frame.",
        ));
    }
    let bytes = Zeroizing::new(
        STANDARD
            .decode(encoded)
            .map_err(|_| ApiError::new("INPUT_INVALID", "Terminal input must be valid base64."))?,
    );
    if bytes.is_empty() || bytes.len() > MAX_INPUT_BYTES {
        return Err(ApiError::new(
            "INPUT_TOO_LARGE",
            "Terminal input must contain 1 to 16384 bytes.",
        ));
    }
    Ok(bytes)
}

pub(crate) fn read_private_key(
    path: &str,
    passphrase: Option<&str>,
) -> Result<russh::keys::PrivateKey, ApiError> {
    let invalid = || {
        ApiError::new(
            "LOCAL_FILE_PERMISSION_DENIED",
            "The selected key must be a readable, private, regular file owned by this user.",
        )
    };
    #[cfg(windows)]
    let text = {
        let bytes = Zeroizing::new(
            crate::windows_private_file::read_private_file(Path::new(path), 1024 * 1024)
                .map_err(|_| invalid())?,
        );
        Zeroizing::new(
            std::str::from_utf8(&bytes)
                .map_err(|_| invalid())?
                .to_owned(),
        )
    };
    #[cfg(not(windows))]
    let text = {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(path).map_err(|_| invalid())?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 {
            return Err(invalid());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // SAFETY: geteuid has no preconditions and does not mutate memory.
            if metadata.mode() & 0o077 != 0 || metadata.uid() != unsafe { libc::geteuid() } {
                return Err(invalid());
            }
        }
        let mut text = Zeroizing::new(String::new());
        file.take(1024 * 1024 + 1)
            .read_to_string(&mut text)
            .map_err(|_| invalid())?;
        if text.len() > 1024 * 1024 {
            return Err(invalid());
        }
        text
    };
    russh::keys::decode_secret_key(&text, passphrase).map_err(|error| {
        if passphrase.is_none()
            && (matches!(error, russh::keys::Error::KeyIsEncrypted)
                || text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----"))
        {
            return ApiError::new(
                "CREDENTIAL_PASSPHRASE_REQUIRED",
                "This private key is encrypted. Enter its passphrase for this connection.",
            );
        }
        ApiError::new(
            "SSH_KEY_FAILED",
            "Unable to decode the private key. Check its format and passphrase.",
        )
    })
}

struct ClientHandler {
    manager: SessionManager,
    session: SessionRef,
    host: String,
    port: u16,
    cancelled: watch::Receiver<bool>,
}
impl client::Handler for ClientHandler {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Certificate trust/CA management is not implemented; never imply it is verified.
        if key.certificate().is_some() {
            return Ok(false);
        }
        let key = key.public_key();
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let persistent = match &self.manager.inner.trust {
            Err(_) => return Ok(false),
            Ok(None) => None,
            Ok(Some(store)) => {
                let store = store.clone();
                let host = self.host.clone();
                let port = self.port;
                let fingerprint = fingerprint.clone();
                match tokio::task::spawn_blocking(move || store.check(&host, port, &fingerprint))
                    .await
                {
                    Ok(Ok(crate::storage::trust::HostTrustCheck::Trusted)) => {
                        return Ok(!*self.cancelled.borrow())
                    }
                    Ok(Ok(crate::storage::trust::HostTrustCheck::ApprovalRequired {
                        challenge_id,
                        previous_fingerprint,
                        ..
                    })) => Some((challenge_id, previous_fingerprint)),
                    _ => return Ok(false),
                }
            }
        };
        let (sender, response) = oneshot::channel();
        let (challenge, previous_state) = {
            let mut state = self.manager.lock();
            if require_live(&state, &self.session).is_err() || *self.cancelled.borrow() {
                return Ok(false);
            }
            let previous = persistent
                .as_ref()
                .map(|(_, previous)| previous.clone())
                .unwrap_or_else(|| state.trusted.get(&(self.host.clone(), self.port)).cloned());
            if previous.as_ref() == Some(&fingerprint) {
                return Ok(true);
            }
            let challenge = HostKeyChallenge {
                challenge_id: persistent
                    .as_ref()
                    .map(|(id, _)| id.clone())
                    .unwrap_or_else(|| Uuid::new_v4().to_string()),
                session: self.session.clone(),
                host: self.host.clone(),
                port: self.port,
                algorithm: key.algorithm().to_string(),
                fingerprint: fingerprint.clone(),
                changed: previous.is_some(),
                previous_fingerprint: previous,
            };
            state.challenge = Some((challenge.challenge_id.clone(), fingerprint.clone(), sender));
            state.snapshot.host_key = Some(challenge.clone());
            (challenge, state.snapshot.state)
        };
        if self
            .manager
            .change_state(&self.session, SessionState::AwaitingHostKey, None)
            .is_err()
            || self.manager.emit(SshEvent::HostKey { challenge }).is_err()
        {
            return Ok(false);
        }
        let accepted = tokio::select! {
            biased;
            _ = self.cancelled.changed() => false,
            result = tokio::time::timeout(Duration::from_secs(120), response) => matches!(result, Ok(Ok(true))),
        };
        let accepted = if accepted {
            if let (Some((challenge_id, _)), Ok(Some(store))) =
                (persistent, &self.manager.inner.trust)
            {
                let store = store.clone();
                let host = self.host.clone();
                let port = self.port;
                let fingerprint = fingerprint.clone();
                !*self.cancelled.borrow()
                    && matches!(
                        tokio::task::spawn_blocking(move || store.accept_from_trust_ui(
                            &challenge_id,
                            &host,
                            port,
                            &fingerprint
                        ))
                        .await,
                        Ok(Ok(()))
                    )
            } else {
                true
            }
        } else {
            false
        };
        let accepted = {
            let mut state = self.manager.lock();
            if require_live(&state, &self.session).is_err() {
                return Ok(false);
            }
            state.challenge = None;
            if accepted && !*self.cancelled.borrow() {
                state
                    .trusted
                    .insert((self.host.clone(), self.port), fingerprint);
                true
            } else {
                false
            }
        };
        if accepted
            && self
                .manager
                .change_state(&self.session, previous_state, None)
                .is_err()
        {
            return Ok(false);
        }
        Ok(accepted)
    }
}

struct ShutdownOnDrop(std::net::TcpStream);
impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ConnectRequest {
        ConnectRequest {
            host: "example.invalid".into(),
            port: 22,
            username: "tester".into(),
            auth: Authentication::Password {
                password: Secret::new("test-only-credential".into()),
            },
            session_only: true,
            size: TerminalSize { cols: 80, rows: 24 },
        }
    }
    fn connected() -> (SessionManager, SessionRef, mpsc::Receiver<Command>) {
        let manager = SessionManager::new(Arc::new(|_| true));
        let session = SessionRef {
            session_id: "fixture-session".into(),
            generation: "1".into(),
        };
        let (commands, receiver) = mpsc::channel(INPUT_QUEUE_FRAMES);
        let (cancel, _) = watch::channel(false);
        {
            let mut state = manager.lock();
            state.snapshot.session = Some(session.clone());
            state.snapshot.state = SessionState::Connected;
            state.live = Some(LiveSession {
                session: session.clone(),
                commands,
                cancel,
                output_window: Arc::new(Semaphore::new(OUTPUT_WINDOW_BYTES)),
                unacked: VecDeque::new(),
            });
        }
        (manager, session, receiver)
    }

    #[test]
    fn validates_dimensions_and_connection_fields() {
        let mut req = request();
        assert!(req.validate().is_ok());
        req.size.cols = 0;
        assert_eq!(req.validate().unwrap_err().code, "INPUT_INVALID");
        req.size.cols = 1001;
        assert!(req.validate().is_err());
        req.size.cols = 80;
        for host in ["", "ssh://host", "user@host", "host\nother", "host name"] {
            req.host = host.into();
            assert_eq!(req.validate().unwrap_err().code, "SSH_CONFIG_INVALID");
        }
        req.host = "::1".into();
        assert!(req.validate().is_ok());
        req.port = 0;
        assert!(req.validate().is_err());
    }
    #[test]
    fn session_only_requires_explicit_opt_in() {
        let mut req = request();
        req.session_only = false;
        assert_eq!(req.validate().unwrap_err().code, "CREDENTIAL_MODE_REQUIRED");
    }
    #[test]
    fn private_key_requires_absolute_path() {
        let mut req = request();
        req.auth = Authentication::PrivateKey {
            private_key_path: "relative-key".into(),
            passphrase: None,
        };
        assert_eq!(req.validate().unwrap_err().code, "LOCAL_PATH_INVALID");
    }
    #[test]
    fn binary_codec_preserves_bytes_and_rejects_oversized_frames() {
        let raw = [0, 0xff, 0xc3, 0xa9, 0x1b, b'[', b'm'];
        assert_eq!(&decode_input(&STANDARD.encode(raw)).unwrap()[..], raw);
        assert!(decode_input(&STANDARD.encode(vec![7; MAX_INPUT_BYTES])).is_ok());
        assert_eq!(
            decode_input(&STANDARD.encode(vec![7; MAX_INPUT_BYTES + 1]))
                .unwrap_err()
                .code,
            "INPUT_TOO_LARGE"
        );
        assert_eq!(
            decode_input("not!base64").unwrap_err().code,
            "INPUT_INVALID"
        );
        assert!(decode_input("").is_err());
    }
    #[test]
    fn connection_dto_rejects_unknown_fields() {
        let invalid = r#"{"host":"h","port":22,"username":"u","auth":{"kind":"password","password":"fixture","approved":true},"session_only":true,"size":{"cols":80,"rows":24}}"#;
        assert!(serde_json::from_str::<ConnectRequest>(invalid).is_err());
    }
    #[test]
    fn one_connection_and_session_generation_are_enforced() {
        let (manager, session, _receiver) = connected();
        assert_eq!(manager.connect(request()).unwrap_err().code, "SESSION_BUSY");
        let stale = SessionRef {
            generation: "0".into(),
            ..session
        };
        assert_eq!(
            manager
                .resize(&stale, TerminalSize { cols: 80, rows: 24 })
                .unwrap_err()
                .code,
            "SESSION_STALE"
        );
        assert_eq!(
            manager.disconnect(&stale).unwrap_err().code,
            "SESSION_STALE"
        );
    }
    #[tokio::test]
    async fn writes_are_fifo_and_receipts_never_claim_remote_success() {
        let (manager, session, mut receiver) = connected();
        let writer = manager.clone();
        let task = tokio::spawn(async move { writer.write(&session, "aGVsbG8=").await });
        match receiver.recv().await.unwrap() {
            Command::Write { data, reply } => {
                assert_eq!(&data[..], b"hello");
                reply
                    .send(Ok(WriteReceipt {
                        transport_seq: "1".into(),
                        delivery: "write_accepted".into(),
                        remote_outcome: "unknown".into(),
                    }))
                    .unwrap();
            }
            _ => panic!("expected input frame"),
        }
        assert_eq!(task.await.unwrap().unwrap().remote_outcome, "unknown");
    }
    #[tokio::test]
    async fn full_queue_refuses_entire_input_without_replay() {
        let (manager, session, _receiver) = connected();
        for _ in 0..INPUT_QUEUE_FRAMES {
            manager
                .resize(&session, TerminalSize { cols: 80, rows: 24 })
                .unwrap();
        }
        assert_eq!(
            manager.write(&session, "aGVsbG8=").await.unwrap_err().code,
            "BACKPRESSURE"
        );
        assert_eq!(manager.lock().live.as_ref().unwrap().commands.capacity(), 0);
    }
    #[tokio::test]
    async fn dropped_inflight_receipt_is_unknown() {
        let (manager, session, mut receiver) = connected();
        let task = tokio::spawn(async move { manager.write(&session, "aGVsbG8=").await });
        drop(receiver.recv().await.unwrap());
        assert_eq!(task.await.unwrap().unwrap_err().code, "DELIVERY_UNKNOWN");
    }
    #[test]
    fn host_key_confirmation_matches_both_challenge_and_fingerprint() {
        let manager = SessionManager::new(Arc::new(|_| true));
        let (sender, mut receiver) = oneshot::channel();
        manager.lock().challenge = Some(("challenge".into(), "SHA256:fixture".into(), sender));
        assert_eq!(
            manager
                .confirm_host_key(HostKeyDecision {
                    challenge_id: "challenge".into(),
                    expected_fingerprint: "SHA256:other".into(),
                    decision: TrustDecision::Accept
                })
                .unwrap_err()
                .code,
            "HOST_KEY_STALE"
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        manager
            .confirm_host_key(HostKeyDecision {
                challenge_id: "challenge".into(),
                expected_fingerprint: "SHA256:fixture".into(),
                decision: TrustDecision::Reject,
            })
            .unwrap();
        assert!(!receiver.try_recv().unwrap());
        assert_eq!(
            manager
                .confirm_host_key(HostKeyDecision {
                    challenge_id: "challenge".into(),
                    expected_fingerprint: "SHA256:fixture".into(),
                    decision: TrustDecision::Accept
                })
                .unwrap_err()
                .code,
            "HOST_KEY_STALE"
        );
    }
    #[tokio::test]
    async fn output_backpressure_requires_actual_cursor_acknowledgement() {
        let (manager, session, _receiver) = connected();
        let mut seq = 0;
        manager
            .publish_output(&session, &mut seq, &vec![7; OUTPUT_WINDOW_BYTES], "stdout")
            .await
            .unwrap();
        assert_eq!(seq, (OUTPUT_WINDOW_BYTES / MAX_OUTPUT_CHUNK) as u64);
        let semaphore = manager.lock().live.as_ref().unwrap().output_window.clone();
        assert_eq!(semaphore.available_permits(), 0);
        assert_eq!(
            manager
                .ack_output(&session, &(seq + 1).to_string())
                .unwrap_err()
                .code,
            "INPUT_INVALID"
        );
        assert_eq!(semaphore.available_permits(), 0);
        manager.ack_output(&session, "1").unwrap();
        assert_eq!(semaphore.available_permits(), MAX_OUTPUT_CHUNK);
        manager.ack_output(&session, &seq.to_string()).unwrap();
        assert_eq!(semaphore.available_permits(), OUTPUT_WINDOW_BYTES);
    }
    #[test]
    fn initial_snapshot_contains_no_credentials() {
        let manager = SessionManager::new(Arc::new(|_| true));
        let serialized = serde_json::to_string(&manager.snapshot()).unwrap();
        assert!(!serialized.contains("password"));
        assert!(!serialized.contains("passphrase"));
        assert!(!serialized.contains("private_key"));
        assert_eq!(manager.snapshot().state, SessionState::Disconnected);
    }
    #[cfg(unix)]
    #[test]
    fn key_reader_refuses_publicly_accessible_files_and_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let base = std::env::temp_dir().join(format!("xyz-key-check-{}", Uuid::new_v4()));
        std::fs::create_dir(&base).unwrap();
        let path = base.join("credential-fixture");
        std::fs::write(&path, "not-a-private-key").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            read_private_key(path.to_str().unwrap(), None)
                .unwrap_err()
                .code,
            "LOCAL_FILE_PERMISSION_DENIED"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = base.join("link");
        symlink(&path, &link).unwrap();
        assert_eq!(
            read_private_key(link.to_str().unwrap(), None)
                .unwrap_err()
                .code,
            "LOCAL_FILE_PERMISSION_DENIED"
        );
        std::fs::remove_dir_all(base).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn windows_ssh_key_reader_uses_private_acl_adapter_with_synthetic_encrypted_key() {
        let root = std::env::temp_dir().join(format!("xyz-ssh-key-{}", Uuid::new_v4()));
        let directory =
            crate::storage::private_files::PrivateDirectory::open_or_create(&root).unwrap();
        let key =
            russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                .unwrap();
        let encrypted = key
            .encrypt(&mut rand::rng(), "synthetic-native-key-passphrase")
            .unwrap();
        let encoded = encrypted
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        directory
            .update("fixture-key", 1024 * 1024, |_| {
                Ok(((), Some(Zeroizing::new(encoded.as_bytes().to_vec()))))
            })
            .unwrap();
        drop(directory);
        let path = root.join("fixture-key");
        let decoded = read_private_key(
            path.to_str().unwrap(),
            Some("synthetic-native-key-passphrase"),
        )
        .unwrap();
        assert_eq!(decoded.public_key(), key.public_key());
        assert_eq!(
            read_private_key(path.to_str().unwrap(), Some("synthetic-wrong-passphrase"))
                .unwrap_err()
                .code,
            "SSH_KEY_FAILED"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    struct ReceiverFixture {
        manager: SessionManager,
        owner: crate::execution::Owner,
        gate: crate::execution::ExecutionGate,
        permit: crate::execution::DispatchPermit,
        binding: crate::shell_integration::DispatchBinding,
    }
    impl ReceiverFixture {
        fn new() -> Self {
            let (manager, session, _receiver) = connected();
            let integration = manager.shell_integration();
            integration.prepare_startup(&session).unwrap();
            let nonce = integration.status().shell_id.unwrap();
            integration.ingest(&session, format!("\x1b]777;xyzlog;2;{nonce};H;5.2.37(1)-release\x07\x1b]777;xyzlog;2;{nonce};R;0;123;\x07").as_bytes());
            let ready = integration.ready_snapshot(&session).unwrap();
            let owner = crate::execution::Owner {
                task_id: "receiver-task".into(),
                run_id: "receiver-run".into(),
                session,
            };
            let gate = crate::execution::ExecutionGate::default();
            gate.acquire(owner.clone(), true).unwrap();
            let action = gate.prepare(&owner, "pwd", &ready).unwrap();
            gate.acknowledge(&owner, &action.action_id, &action.display_revision)
                .unwrap();
            let binding = integration
                .prepare_dispatch(
                    &owner.session,
                    &action.action_id,
                    &action.command_text,
                    &ready,
                )
                .unwrap();
            let mut permit = gate.queue(&owner).unwrap();
            let session = owner.session.clone();
            let validator = integration.clone();
            let expected = binding.clone();
            permit.on_validate = Some(Arc::new(move || {
                validator.validate_dispatch(&session, &expected)
            }));
            permit.begin(&owner.session).unwrap();
            Self {
                manager,
                owner,
                gate,
                permit,
                binding,
            }
        }
        fn marker(&self, action_id: &str) {
            let session = &self.owner.session;
            let binding = &self.binding;
            self.manager.shell_integration().ingest(
                session,
                format!(
                    "\x1b]777;xyzlog;3;{};M;{};{};{};{};{}\x07",
                    binding.shell_id,
                    binding.sequence,
                    session.session_id,
                    session.generation,
                    action_id,
                    binding.payload_digest
                )
                .as_bytes(),
            );
        }
        fn release(&self) -> Result<(), ApiError> {
            self.manager.release_receiver_body(
                &self.owner.session,
                &self.permit,
                tokio::time::Instant::now() + RECEIVER_TIMEOUT,
            )
        }
    }
    #[test]
    fn ai_body_release_requires_exact_receiver_marker_and_is_single_use() {
        let fixture = ReceiverFixture::new();
        assert!(fixture.release().is_err());
        fixture.marker(fixture.permit.action_id());
        fixture.release().unwrap();
        assert_eq!(
            fixture.manager.shell_integration().status().phase,
            crate::shell_integration::IntegrationPhase::AwaitingBegin
        );
        assert!(fixture.release().is_err());
        assert!(fixture
            .manager
            .shell_integration()
            .status()
            .completed
            .is_none());
    }
    #[test]
    fn ai_body_release_rejects_wrong_receiver_action() {
        let fixture = ReceiverFixture::new();
        fixture.marker("wrong-action");
        assert!(fixture.release().is_err());
        assert!(fixture
            .manager
            .shell_integration()
            .status()
            .completed
            .is_none());
    }
    #[test]
    fn ai_body_release_rechecks_cancellation_after_marker() {
        let fixture = ReceiverFixture::new();
        fixture.marker(fixture.permit.action_id());
        assert!(fixture.gate.cancel(&fixture.owner));
        assert!(fixture.release().is_err());
        assert_eq!(
            fixture.manager.shell_integration().status().phase,
            crate::shell_integration::IntegrationPhase::Receiving
        );
    }
    #[test]
    fn ai_body_release_rechecks_deadline_and_physical_session() {
        let fixture = ReceiverFixture::new();
        fixture.marker(fixture.permit.action_id());
        assert!(fixture
            .manager
            .release_receiver_body(
                &fixture.owner.session,
                &fixture.permit,
                tokio::time::Instant::now() - Duration::from_millis(1)
            )
            .is_err());
        fixture
            .manager
            .lock()
            .live
            .as_mut()
            .unwrap()
            .session
            .generation = "2".into();
        assert!(fixture.release().is_err());
        assert_eq!(
            fixture.manager.shell_integration().status().phase,
            crate::shell_integration::IntegrationPhase::Receiving
        );
    }

    #[tokio::test]
    async fn dropped_ai_receipt_distinguishes_before_and_after_actor_begin() {
        use crate::execution::{ExecutionGate, Owner};
        for started in [false, true] {
            let (manager, session, mut receiver) = connected();
            let gate = ExecutionGate::default();
            let owner = Owner {
                task_id: "task".into(),
                run_id: "run".into(),
                session: session.clone(),
            };
            gate.acquire(owner.clone(), true).unwrap();
            let action = gate
                .prepare(
                    &owner,
                    "pwd",
                    &crate::shell_integration::ReadyBinding {
                        shell_id: "0123456789abcdef0123456789abcdef".into(),
                        sequence: 0,
                        revision: 1,
                        shadowed_commands: vec![],
                    },
                )
                .unwrap();
            gate.acknowledge(&owner, &action.action_id, "1").unwrap();
            let permit = gate.queue(&owner).unwrap();
            let writer = manager.clone();
            let target = session.clone();
            let write = tokio::spawn(async move { writer.write_ai(&target, permit).await });
            match receiver.recv().await.unwrap() {
                Command::AiWrite { permit, reply } => {
                    if started {
                        permit.begin(&session).unwrap();
                    }
                    drop(reply);
                }
                _ => panic!("expected AI frame"),
            }
            assert_eq!(
                write.await.unwrap().unwrap_err().code,
                if started {
                    "DELIVERY_UNKNOWN"
                } else {
                    "NOT_SENT"
                }
            );
        }
    }
}
