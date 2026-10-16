//! Explicitly opted-in, passive shell metadata capture. No keystrokes or PTY
//! output are logged, and no model, credential store, or approval system is consulted.
use crate::{
    observation::ObservationBuffer,
    shell_log_protocol::{ProtocolParser, WireEvent},
    shell_log_store::ShellLogStore,
    shell_log_types::*,
    ssh::{ApiError, SessionRef, SessionSnapshot, SessionState},
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const QUEUE_CAPACITY: usize = 128;
#[cfg(test)]
const MAX_START_ATTEMPTS: usize = 1024;
#[cfg(test)]
use crate::shell_log_protocol::bootstrap_command;
#[cfg(test)]
use std::time::Duration;
pub type ShellLogEventSink = Arc<dyn Fn(ShellLogStatus) + Send + Sync>;
type Reply<T> = mpsc::Sender<Result<T, ApiError>>;
#[derive(Clone)]
pub struct ShellLogService {
    sender: SyncSender<Work>,
    shared: Arc<Shared>,
    capture: Arc<Mutex<Capture>>,
    redactor: ObservationBuffer,
}
struct Shared {
    status: Mutex<ShellLogStatus>,
    epoch: AtomicU64,
    // Only control calls and the disk worker take this gate. SSH never does.
    commit: Mutex<()>,
    events: ShellLogEventSink,
    handshake_deadline: Mutex<Option<Instant>>,
}
struct Capture {
    parser: ProtocolParser,
    parser_session: Option<SessionRef>,
    snapshot: Option<SessionSnapshot>,
    #[cfg(test)]
    attempted: BTreeSet<(String, String)>,
    shell_id: String,
    hello: bool,
    integration_lost: bool,
    high_sequence: u64,
    active: Option<(u64, String)>,
}
enum Work {
    Open(Reply<()>),
    Append(u64, ShellLogEntry),
    Finish(u64, String, CommandStatus, Option<i32>, i64),
    Wake,
    Query(ShellLogQuery, Reply<ShellLogPage>),
    Export(ShellLogQuery, ExportFormat, Reply<ShellLogExport>),
    Flush(Reply<()>),
}
fn error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
impl Shared {
    fn status(&self) -> ShellLogStatus {
        let expired = {
            let mut deadline = self
                .handshake_deadline
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if deadline.is_some_and(|d| Instant::now() >= d) {
                *deadline = None;
                true
            } else {
                false
            }
        };
        if expired {
            self.fail(error(
                "SHELL_LOG_HANDSHAKE_TIMEOUT",
                "Shell integration did not acknowledge startup. Reconnect before trying again.",
            ));
        }
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    fn update(&self, update: impl FnOnce(&mut ShellLogStatus)) -> ShellLogStatus {
        let status = {
            let mut s = self.status.lock().unwrap_or_else(|p| p.into_inner());
            update(&mut s);
            s.revision = s
                .revision
                .parse::<u64>()
                .unwrap_or_default()
                .saturating_add(1)
                .to_string();
            s.clone()
        };
        (self.events)(status.clone());
        status
    }
    fn fail(&self, e: ApiError) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.update(|s| {
            s.enabled = false;
            s.state = CaptureState::Gap;
            s.error = Some(e);
            s.reason = Some("Capture stopped; records may be incomplete.".into());
        });
    }
}
impl ShellLogService {
    /// The path is the same private application data root used by History.
    /// Construction starts one bounded worker but never opens or creates files.
    pub fn new(
        path: Result<PathBuf, ApiError>,
        redactor: ObservationBuffer,
        events: ShellLogEventSink,
    ) -> Self {
        let shared = Arc::new(Shared {
            status: Mutex::new(ShellLogStatus {
                enabled: false,
                available: path.is_ok(),
                plaintext: true,
                revision: "0".into(),
                state: CaptureState::Disabled,
                session: None,
                reason: None,
                error: path.as_ref().err().cloned(),
            }),
            epoch: AtomicU64::new(0),
            commit: Mutex::new(()),
            events,
            handshake_deadline: Mutex::new(None),
        });
        let nonce = Uuid::new_v4().simple().to_string();
        let capture = Arc::new(Mutex::new(Capture {
            parser: ProtocolParser::new(&nonce).expect("UUID nonce"),
            parser_session: None,
            snapshot: None,
            #[cfg(test)]
            attempted: BTreeSet::new(),
            shell_id: String::new(),
            hello: false,
            integration_lost: false,
            high_sequence: 0,
            active: None,
        }));
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let worker_shared = shared.clone();
        let worker_redactor = redactor.clone();
        if std::thread::Builder::new()
            .name("xyz-shell-log".into())
            .spawn(move || worker(path, receiver, worker_shared, worker_redactor))
            .is_err()
        {
            shared.fail(error(
                "SHELL_LOG_WORKER",
                "Shell log worker could not start.",
            ));
        }
        Self {
            sender,
            shared,
            capture,
            redactor,
        }
    }
    pub fn status(&self) -> ShellLogStatus {
        self.shared.status()
    }
    /// Subscribe the optional disk consumer to an already-verified integration.
    /// Never injects shell input, changes hook ownership, or replays old events.
    pub fn start_shared(
        &self,
        snapshot: SessionSnapshot,
        plaintext_consent: bool,
        shell_id: String,
        handshaken: bool,
    ) -> Result<(), ApiError> {
        if !plaintext_consent {
            return Err(error(
                "SHELL_LOG_CONSENT",
                "Explicit consent to local plaintext shell logs is required.",
            ));
        }
        if snapshot.state != SessionState::Connected || !handshaken || shell_id.is_empty() {
            return Err(error("SHELL_INTEGRATION_REQUIRED", "Enable the current-shell integration at an idle supported Bash prompt before recording."));
        }
        let session = snapshot
            .session
            .clone()
            .ok_or_else(|| error("SHELL_LOG_SESSION", "No connected session."))?;
        if snapshot.host.is_none() || snapshot.port.is_none() || snapshot.username.is_none() {
            return Err(error(
                "SHELL_LOG_SESSION",
                "Connection details are unavailable.",
            ));
        }
        {
            let _gate = self.shared.commit.lock().unwrap_or_else(|p| p.into_inner());
            let mut c = self.capture.lock().unwrap_or_else(|p| p.into_inner());
            if self.status().enabled {
                return Err(error(
                    "SHELL_LOG_ALREADY_STARTED",
                    "Shell recording is already enabled; resume it if paused.",
                ));
            }
            self.shared.epoch.fetch_add(1, Ordering::SeqCst);
            c.parser_session = Some(session.clone());
            c.snapshot = Some(snapshot);
            c.shell_id = shell_id;
            c.hello = true;
            c.integration_lost = false;
            c.high_sequence = 0;
            c.active = None;
        }
        self.shared.update(|s| {
            s.enabled = true;
            s.state = CaptureState::Recording;
            s.session = Some(session);
            s.error = None;
            s.reason = None;
        });
        if let Err(e) = self.request(Work::Open) {
            self.shared.fail(e.clone());
            return Err(e);
        }
        Ok(())
    }
    /// Events arrive from the shared live parser only. Historical files and
    /// exported metadata cannot call this via IPC or restore execution state.
    pub(crate) fn ingest_event(&self, session: &SessionRef, event: WireEvent) {
        let mut c = self.capture.lock().unwrap_or_else(|p| p.into_inner());
        if self.status().session.as_ref() == Some(session) {
            self.handle_event(&mut c, event);
        }
    }
    /// Explicit consent is required every process. Return this bootstrap for ONE
    /// visible PTY write. Never retry it automatically, even if delivery is unknown.
    /// This control operation can block on the worker; keep it off the SSH loop.
    #[cfg(test)]
    fn start(
        &self,
        snapshot: SessionSnapshot,
        plaintext_consent: bool,
    ) -> Result<String, ApiError> {
        if !plaintext_consent {
            return Err(error(
                "SHELL_LOG_CONSENT",
                "Explicit consent to local plaintext shell logs is required.",
            ));
        }
        if snapshot.state != SessionState::Connected {
            return Err(error(
                "SHELL_LOG_SESSION",
                "Connect an idle shell before enabling shell logs.",
            ));
        }
        let session = snapshot
            .session
            .clone()
            .ok_or_else(|| error("SHELL_LOG_SESSION", "No connected session."))?;
        if snapshot.host.is_none() || snapshot.port.is_none() || snapshot.username.is_none() {
            return Err(error(
                "SHELL_LOG_SESSION",
                "Connection details are unavailable.",
            ));
        }
        // Never hold the capture lock while waiting for disk work.
        let gate = self.shared.commit.lock().unwrap_or_else(|p| p.into_inner());
        // Reserve before opening files so concurrent starts cannot inject twice.
        let mut c = self.capture.lock().unwrap_or_else(|p| p.into_inner());
        let key = (session.session_id.clone(), session.generation.clone());
        if c.attempted.contains(&key) {
            return Err(error("SHELL_LOG_ALREADY_STARTED", "Shell integration was already requested for this connection; reconnect to enable again."));
        }
        if c.attempted.len() >= MAX_START_ATTEMPTS {
            return Err(error("SHELL_LOG_SESSION_LIMIT", "Shell integration session limit reached; restart the application before enabling again."));
        }
        let nonce = Uuid::new_v4().simple().to_string();
        let bootstrap = bootstrap_command(&nonce)
            .map_err(|_| error("SHELL_LOG_PROTOCOL", "Cannot prepare shell integration."))?;
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        c.attempted.insert(key);
        c.parser = ProtocolParser::new(&nonce).expect("UUID nonce");
        c.parser_session = Some(session.clone());
        c.snapshot = Some(snapshot);
        c.shell_id = Uuid::new_v4().to_string();
        c.hello = false;
        c.integration_lost = false;
        c.high_sequence = 0;
        c.active = None;
        drop(gate);
        drop(c);
        self.shared.update(|s| {
            s.enabled = true;
            s.state = CaptureState::Starting;
            s.session = Some(session.clone());
            s.error = None;
            s.reason = Some("Waiting for shell integration handshake.".into());
        });
        if let Err(e) = self.request(Work::Open) {
            self.shared.fail(e.clone());
            return Err(e);
        }
        let status = self.status();
        if !status.enabled || status.session.as_ref() != Some(&session) {
            return Err(error(
                "SHELL_LOG_SESSION",
                "The shell disconnected or logging stopped during startup.",
            ));
        }
        *self
            .shared
            .handshake_deadline
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(Instant::now() + Duration::from_secs(15));
        self.wake();
        Ok(bootstrap)
    }
    /// On return, no old queued capture can be committed. A transaction already
    /// in progress is allowed to finish before this control operation returns.
    pub fn pause(&self) -> Result<ShellLogStatus, ApiError> {
        let gate = self.shared.commit.lock().unwrap_or_else(|p| p.into_inner());
        let mut c = self.capture.lock().unwrap_or_else(|p| p.into_inner());
        let s = self.status();
        if !s.enabled {
            return Err(error(
                "SHELL_LOG_NOT_ENABLED",
                "Shell logging is not enabled.",
            ));
        }
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        c.active = None;
        let result = self.shared.update(|s| {
            s.state = CaptureState::Paused;
            s.reason = Some(if c.integration_lost {
                "Collection paused; shell integration was lost. Reconnect and wait for automatic shell preparation before recording again."
            } else {
                "Collection paused; protocol metadata remains filtered."
            }.into());
        });
        drop(gate);
        drop(c);
        self.wake();
        Ok(result)
    }
    /// Resume the already-enabled shell only; this never returns an injection.
    pub fn resume(&self, session: &SessionRef) -> Result<ShellLogStatus, ApiError> {
        let c = self.capture.lock().unwrap_or_else(|p| p.into_inner());
        let s = self.status();
        if c.integration_lost && s.session.as_ref() == Some(session) {
            return Err(error("SHELL_LOG_INTEGRATION_LOST", "Shell integration was lost while recording or paused. Reconnect and wait for automatic shell preparation before recording again."));
        }
        if !s.enabled
            || s.session.as_ref() != Some(session)
            || s.state != CaptureState::Paused
            || !c.hello
            || s.error.is_some()
        {
            return Err(error(
                "SHELL_LOG_CANNOT_RESUME",
                "Only a paused, connected, handshaken shell can resume logging.",
            ));
        }
        Ok(self.shared.update(|s| {
            s.state = CaptureState::Recording;
            s.reason = None;
        }))
    }
    /// Metadata is removed even when disabled, paused, or in an error state.
    /// Return value is ONLY for observation. Render original bytes in the PTY.
    pub fn ingest(&self, session: &SessionRef, bytes: &[u8]) -> Vec<u8> {
        let mut c = self.capture.lock().unwrap_or_else(|p| p.into_inner());
        let status = self.status();
        if c.parser_session
            .as_ref()
            .is_some_and(|current| current != session)
        {
            let previous = c.parser_session.as_ref().expect("checked session");
            // SessionManager generations increase monotonically. Reject old
            // callbacks even after capture stops, but let a newer unlogged
            // connection continue ordinary observation and metadata filtering.
            let newer = session
                .generation
                .parse::<u64>()
                .ok()
                .zip(previous.generation.parse::<u64>().ok())
                .is_some_and(|(next, old)| next > old);
            if status.enabled || !newer {
                return Vec::new();
            }
            let nonce = Uuid::new_v4().simple().to_string();
            c.parser = ProtocolParser::new(&nonce).expect("UUID nonce");
        }
        c.parser_session = Some(session.clone());
        let parsed = c.parser.feed(bytes);
        if status.session.as_ref() == Some(session) {
            for event in parsed.events {
                self.handle_event(&mut c, event);
            }
        }
        parsed.visible
    }
    /// A failed/uncertain bootstrap delivery is terminal for this generation.
    pub fn fail_start(&self, session: &SessionRef, _cause: ApiError) {
        if self.status().session.as_ref() != Some(session) {
            return;
        }
        *self
            .shared
            .handshake_deadline
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        self.shared.fail(error(
            "SHELL_LOG_START_FAILED",
            "Shell integration delivery failed or is uncertain. Reconnect before enabling again.",
        ));
        self.wake();
    }
    pub fn disconnect(&self, session: &SessionRef) {
        let mut c = self.capture.lock().unwrap_or_else(|p| p.into_inner());
        if self.status().session.as_ref() != Some(session) {
            return;
        }
        // No disk lock on the SSH event thread. The worker reconciles this epoch.
        *self
            .shared
            .handshake_deadline
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        c.active = None;
        c.hello = false;
        c.parser.finish();
        self.shared.update(|s| {
            s.enabled = false;
            s.state = CaptureState::Disconnected;
            s.reason =
                Some("Shell disconnected; unfinished commands have unknown outcomes.".into());
        });
        drop(c);
        self.wake();
    }
    pub fn query(&self, query: ShellLogQuery) -> Result<ShellLogPage, ApiError> {
        self.request(|r| Work::Query(query, r))
    }
    pub fn export(
        &self,
        query: ShellLogQuery,
        format: ExportFormat,
    ) -> Result<ShellLogExport, ApiError> {
        self.request(|r| Work::Export(query, format, r))
    }
    pub fn flush(&self) -> Result<(), ApiError> {
        self.request(Work::Flush)
    }
    fn request<T>(&self, make: impl FnOnce(Reply<T>) -> Work) -> Result<T, ApiError> {
        let (send, recv) = mpsc::channel();
        self.sender.try_send(make(send)).map_err(|_| {
            error(
                "SHELL_LOG_BUSY",
                "Shell log worker is busy or unavailable; try again.",
            )
        })?;
        recv.recv()
            .map_err(|_| error("SHELL_LOG_WORKER", "Shell log worker is unavailable."))?
    }
    fn wake(&self) {
        let _ = self.sender.try_send(Work::Wake);
    }
    fn enqueue(&self, work: Work) {
        if self.sender.try_send(work).is_err() {
            self.shared.fail(error(
                "SHELL_LOG_QUEUE_FULL",
                "Shell logging stopped because its storage queue is full or unavailable.",
            ));
        }
    }
    fn finish_active(&self, c: &mut Capture, status: CommandStatus, code: Option<i32>) {
        if let Some((_, id)) = c.active.take() {
            self.enqueue(Work::Finish(
                self.shared.epoch.load(Ordering::SeqCst),
                id,
                status,
                code,
                now_ms(),
            ));
        }
    }
    fn integration_lost(&self, c: &mut Capture) {
        c.integration_lost = true;
        self.finish_active(c, CommandStatus::Unknown, None);
        self.shared.update(|s| {
            if s.state != CaptureState::Paused { s.state = CaptureState::Gap; }
            s.reason = Some("Shell integration was lost; records may be incomplete. Reconnect and wait for automatic shell preparation before recording again.".into());
        });
    }
    fn handle_event(&self, c: &mut Capture, event: WireEvent) {
        let s = self.status();
        if !s.enabled || (c.integration_lost && !matches!(&event, WireEvent::Closed)) {
            return;
        }
        match event {
            WireEvent::ReceiverReady { .. }
            | WireEvent::Ready { .. }
            | WireEvent::ActionBegin { .. }
            | WireEvent::ActionEnd { .. } => {}
            WireEvent::Unsupported { .. } => {
                self.finish_active(c, CommandStatus::Unknown, None);
                self.shared.fail(error("SHELL_LOG_UNSUPPORTED", "Current-shell integration is unsupported; the manual terminal remains available."));
            }
            WireEvent::Hello { .. } => {
                if c.hello {
                    return;
                }
                c.hello = true;
                *self
                    .shared
                    .handshake_deadline
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = None;
                if s.state == CaptureState::Starting {
                    self.shared.update(|s| {
                        s.state = CaptureState::Recording;
                        s.reason = None;
                    });
                }
            }
            WireEvent::Begin {
                sequence,
                remote_time_s,
                cwd,
                command,
            } => {
                if !c.hello || sequence <= c.high_sequence {
                    return;
                }
                c.high_sequence = sequence;
                if !matches!(s.state, CaptureState::Recording | CaptureState::Gap) {
                    return;
                }
                self.finish_active(c, CommandStatus::Unknown, None);
                if !self.status().enabled {
                    return;
                }
                let snapshot = c.snapshot.as_ref().expect("enabled snapshot");
                let session = snapshot.session.as_ref().expect("enabled session");
                let command_clean = self.redactor.redact_prose(&command);
                let cwd_clean = self.redactor.redact_prose(&cwd);
                let host_clean = self
                    .redactor
                    .redact_prose(snapshot.host.as_deref().unwrap_or_default());
                let username_clean = self
                    .redactor
                    .redact_prose(snapshot.username.as_deref().unwrap_or_default());
                let redacted = command_clean != command
                    || cwd_clean != cwd
                    || host_clean != snapshot.host.as_deref().unwrap_or_default()
                    || username_clean != snapshot.username.as_deref().unwrap_or_default();
                let id = Uuid::new_v4().to_string();
                let entry = ShellLogEntry {
                    id: id.clone(),
                    profile_id: snapshot.profile_id.clone(),
                    sequence: "0".into(),
                    session_id: session.session_id.clone(),
                    generation: session.generation.clone(),
                    shell_id: c.shell_id.clone(),
                    shell_sequence: sequence.to_string(),
                    host: host_clean,
                    port: snapshot.port.unwrap_or_default(),
                    username: username_clean,
                    started_at_ms: now_ms(),
                    finished_at_ms: None,
                    remote_started_at: Some(remote_time_s.to_string()),
                    command: command_clean.clone(),
                    cwd: cwd_clean.clone(),
                    status: CommandStatus::Running,
                    exit_code: None,
                    redacted,
                };
                c.active = Some((sequence, id));
                self.enqueue(Work::Append(
                    self.shared.epoch.load(Ordering::SeqCst),
                    entry,
                ));
                if self.status().enabled {
                    self.shared.update(|s| {
                        s.state = CaptureState::Recording;
                        s.reason = None;
                    });
                }
            }
            WireEvent::End {
                sequence,
                exit_code,
                ..
            } => {
                if c.active.as_ref().map(|a| a.0) == Some(sequence)
                    && matches!(s.state, CaptureState::Recording | CaptureState::Gap)
                {
                    self.finish_active(c, CommandStatus::Completed, Some(exit_code.into()));
                }
            }
            WireEvent::Closed => {
                self.finish_active(c, CommandStatus::Unknown, None);
                c.hello = false;
                *self
                    .shared
                    .handshake_deadline
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = None;
                self.shared.update(|s| {
                    s.enabled = false;
                    s.state = CaptureState::Disconnected;
                    s.reason = Some("The integrated shell exited.".into());
                });
            }
            WireEvent::Malformed { .. } => {
                if c.hello {
                    self.integration_lost(c);
                }
            }
            WireEvent::Gap { sequence, reason }
                if matches!(
                    reason.as_str(),
                    "legacy_history_fidelity"
                        | "legacy_unobserved_command"
                        | "legacy_async_command"
                ) && c.active.is_none() =>
            {
                if !c.hello || sequence < c.high_sequence {
                    return;
                }
                c.high_sequence = sequence;
                self.shared.update(|s| {
                    if s.state != CaptureState::Paused {
                        s.state = CaptureState::Gap;
                    }
                    s.reason = Some(
                        "手动命令观察存在缺口，未推断其命令原文或退出状态；后续 AI 命令仍可记录。"
                            .into(),
                    );
                });
            }
            WireEvent::Gap { sequence, .. } => {
                // A prompt/hook failure can be reported AFTER End, using the
                // same top-level command sequence that Begin already accepted.
                // Drop only older replays; a current-boundary gap must remain
                // visible even when the foreground record is already complete.
                if !c.hello || sequence < c.high_sequence {
                    return;
                }
                c.high_sequence = sequence;
                self.integration_lost(c);
            }
        }
    }
}

fn store_error(e: crate::persistence::HistoryError) -> ApiError {
    use crate::persistence::HistoryError::*;
    let (code, message) = match e {
        LimitReached => (
            "SHELL_LOG_LIMIT",
            "Shell log capacity was reached; no records were silently evicted.",
        ),
        Busy => (
            "SHELL_LOG_BUSY",
            "Shell log files are in use; try again later.",
        ),
        UnsafeFile | InvalidPath => (
            "SHELL_LOG_UNSAFE_PATH",
            "Shell logs require a private owned directory without links.",
        ),
        UnsupportedSchema => (
            "SHELL_LOG_SCHEMA",
            "This shell log database format is unsupported; it was not changed.",
        ),
        Corrupt => (
            "SHELL_LOG_CORRUPT",
            "Shell log storage is damaged; preserve the files for recovery.",
        ),
        SaveUncertain => (
            "SHELL_LOG_SAVE_UNCERTAIN",
            "A shell log save could not be confirmed; logging stopped.",
        ),
        InvalidRecord => (
            "SHELL_LOG_INVALID_RECORD",
            "A shell log record or query is invalid or exceeds its limits.",
        ),
        _ => ("SHELL_LOG_STORAGE", "Shell log storage is unavailable."),
    };
    error(code, message)
}
fn worker(
    path: Result<PathBuf, ApiError>,
    receiver: mpsc::Receiver<Work>,
    shared: Arc<Shared>,
    redactor: ObservationBuffer,
) {
    let mut store: Option<ShellLogStore> = None;
    let mut epoch = shared.epoch.load(Ordering::SeqCst);
    let mut running = BTreeSet::<String>::new();
    while let Ok(work) = receiver.recv() {
        // Serializes the epoch check AND disk commit against pause. The network
        // path never takes this lock, and control replies are sent after commit.
        let gate = shared.commit.lock().unwrap_or_else(|p| p.into_inner());
        let current = shared.epoch.load(Ordering::SeqCst);
        if epoch != current {
            if let Some(store) = store.as_mut() {
                for id in std::mem::take(&mut running) {
                    if let Err(e) = store.finish(&id, &CommandStatus::Unknown, None, now_ms()) {
                        shared.fail(store_error(e));
                    }
                }
            }
            epoch = current;
        }
        match work {
            Work::Open(reply) => {
                let result = ensure_open(&mut store, &path).map(|_| ());
                if let Err(e) = &result {
                    shared.fail(e.clone());
                }
                let _ = reply.send(result);
            }
            Work::Append(captured_epoch, entry) => {
                if captured_epoch != shared.epoch.load(Ordering::SeqCst) {
                    continue;
                }
                let result = ensure_open(&mut store, &path).and_then(|s| {
                    s.append(&entry, |text| redactor.redact_prose(text))
                        .map_err(store_error)
                });
                match result {
                    Ok(entry) => {
                        running.insert(entry.id);
                    }
                    Err(e) => shared.fail(e),
                }
            }
            Work::Finish(captured_epoch, id, status, exit, time) => {
                if captured_epoch != shared.epoch.load(Ordering::SeqCst) || !running.contains(&id) {
                    continue;
                }
                if let Some(s) = store.as_mut() {
                    match s.finish(&id, &status, exit, time) {
                        Ok(()) => {
                            running.remove(&id);
                        }
                        Err(e) => shared.fail(store_error(e)),
                    }
                }
            }
            Work::Query(query, reply) => {
                let result = existing_store(&mut store, &path).and_then(|s| match s {
                    Some(s) => s
                        .query(&query, |text| redactor.redact_prose(text))
                        .map_err(store_error),
                    None => Ok(ShellLogPage {
                        entries: Vec::new(),
                        next_before_seq: None,
                        snapshot_max_seq: "0".into(),
                    }),
                });
                let _ = reply.send(result);
            }
            Work::Export(query, format, reply) => {
                let result = existing_store(&mut store, &path).and_then(|s| match s {
                    Some(s) => s
                        .export(&query, format, |text| redactor.redact_prose(text))
                        .map_err(store_error),
                    None => Err(error(
                        "SHELL_LOG_EMPTY",
                        "No shell log database exists to export.",
                    )),
                });
                let _ = reply.send(result);
            }
            Work::Flush(reply) => {
                let result = shared.status().error.map_or(Ok(()), Err);
                let _ = reply.send(result);
            }
            Work::Wake => {}
        }
        drop(gate);
    }
    // Normal application teardown is also an unknown outcome, not success.
    if let Some(store) = store.as_mut() {
        for id in running {
            let _ = store.finish(&id, &CommandStatus::Unknown, None, now_ms());
        }
    }
}
fn ensure_open<'a>(
    store: &'a mut Option<ShellLogStore>,
    path: &Result<PathBuf, ApiError>,
) -> Result<&'a mut ShellLogStore, ApiError> {
    if store.is_none() {
        *store =
            Some(ShellLogStore::open(path.as_ref().map_err(Clone::clone)?).map_err(store_error)?);
    }
    Ok(store.as_mut().expect("opened store"))
}

#[cfg(test)]
#[path = "shell_log_tests.rs"]
mod tests;

// Listing an untouched, disabled log must not create directories or a database.
fn existing_store<'a>(
    store: &'a mut Option<ShellLogStore>,
    path: &Result<PathBuf, ApiError>,
) -> Result<Option<&'a mut ShellLogStore>, ApiError> {
    if store.is_none() {
        let root = path.as_ref().map_err(Clone::clone)?;
        match std::fs::symlink_metadata(root.join(crate::shell_log_store::DATABASE_NAME)) {
            Ok(_) => {
                ensure_open(store, path)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => {
                return Err(error(
                    "SHELL_LOG_STORAGE",
                    "Shell log storage cannot be inspected.",
                ))
            }
        }
    }
    Ok(store.as_mut())
}
