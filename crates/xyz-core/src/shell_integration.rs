//! Cooperative CURRENT-shell lifecycle, independent of persistent logging.
//! Remote metadata is untrusted correlation evidence, never an approval or an
//! authenticated audit. No observation text, prompt shape, or silence is readiness.
use crate::{
    shell_log::ShellLogService,
    shell_log_protocol::{bootstrap_command, ProtocolParser, WireEvent},
    ssh::{ApiError, SessionRef, SessionSnapshot, SessionState},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationPhase {
    Unintegrated,
    Installing,
    AwaitingReady,
    Ready,
    AwaitingBegin,
    Running,
    Unknown,
    Unsupported,
    Disconnected,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandCompletion {
    pub action_id: String,
    pub sequence: u64,
    pub exit_code: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntegrationStatus {
    pub session: Option<SessionRef>,
    pub shell_id: Option<String>,
    pub phase: IntegrationPhase,
    pub sequence: u64,
    pub revision: u64,
    pub active_action_id: Option<String>,
    pub completed: Option<CommandCompletion>,
    pub shadowed_commands: Vec<String>,
    pub error: Option<ApiError>,
}
#[derive(Clone, Debug)]
pub struct ReadyBinding {
    pub shell_id: String,
    pub sequence: u64,
    pub revision: u64,
    pub shadowed_commands: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct DispatchBinding {
    pub shell_id: String,
    pub sequence: u64,
    pub revision: u64,
    pub action_id: String,
    pub command: String,
    pub transport_command: String,
}
/// Remove only host-issued correlation suffixes from model-facing echoed text.
/// Streaming prefixes and the recent marker set are independently bounded; the
/// original terminal stream is never modified. These markers are not secrets.
#[derive(Default)]
struct EchoMetadataFilter {
    markers: VecDeque<Vec<u8>>,
    prefix: Vec<u8>,
}
impl EchoMetadataFilter {
    fn register(&mut self, action_id: &str) {
        let marker = format!(" # xyz-action:{action_id}").into_bytes();
        if !self.markers.contains(&marker) {
            self.markers.push_back(marker);
        }
        while self.markers.len() > 16 {
            self.markers.pop_front();
        }
    }
    fn feed(&mut self, input: &[u8]) -> Vec<u8> {
        let mut visible = Vec::with_capacity(input.len());
        for &byte in input {
            if matches!(byte, b'\r' | b'\n') && self.markers.iter().any(|m| *m == self.prefix) {
                self.prefix.clear();
                visible.push(byte);
                continue;
            }
            self.prefix.push(byte);
            while !self.prefix.is_empty()
                && !self.markers.iter().any(|m| m.starts_with(&self.prefix))
            {
                visible.push(self.prefix.remove(0));
            }
        }
        visible
    }
}
struct Capture {
    parser: ProtocolParser,
    parser_session: Option<SessionRef>,
    echo_filter: EchoMetadataFilter,
    status: IntegrationStatus,
    attempted: BTreeSet<(String, String)>,
    hello: bool,
    deadline: Option<Instant>,
    pending: Option<DispatchBinding>,
    // Includes manual commands: they invalidate ready snapshots without locking
    // keyboard input. Only a reserved, exactly correlated action is completed.
    active_sequence: Option<u64>,
}
#[derive(Clone)]
pub struct ShellIntegrationService {
    inner: Arc<Mutex<Capture>>,
    updates: watch::Sender<IntegrationStatus>,
    log: Arc<Mutex<Option<ShellLogService>>>,
}
fn error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
/// Remote reason strings are untrusted. Only locally defined descriptions are
/// exposed; an unknown reason never becomes UI text or a repair command.
fn unsupported_shell(reason: &str) -> ApiError {
    let (code, message) = match reason {
        "unsupported_bash" => (
            "SHELL_UNSUPPORTED_BASH",
            "Task integration requires an interactive Bash 5.1 or 5.2 prompt. Use the manual terminal, or open a supported Bash session yourself and reconnect before confirming an idle prompt. No shell settings were changed.",
        ),
        "namespace_conflict" => (
            "SHELL_UNSUPPORTED_NAMESPACE",
            "The current shell already defines a variable, function, or alias beginning with __xyz_. Use the manual terminal or review the conflict in a fresh session yourself, then reconnect. Existing definitions were preserved.",
        ),
        "unsupported_options" => (
            "SHELL_UNSUPPORTED_OPTIONS",
            "The shell options or history settings cannot safely track task commands. Integration requires history, cmdhist, promptvars and interactive_comments, positive HISTSIZE, empty HISTIGNORE and standard histchars; errexit, functrace, allexport, restricted mode, extdebug and POSIX mode are unsupported. Review these settings yourself or use the manual terminal. No options or history settings were changed.",
        ),
        "unsupported_trap" => (
            "SHELL_UNSUPPORTED_TRAP",
            "The shell has a trap other than EXIT, which can interfere with command lifecycle evidence. Review the traps yourself or use the manual terminal; integration will not remove them. Existing traps were preserved.",
        ),
        "dynamic_prompt" => (
            "SHELL_UNSUPPORTED_DYNAMIC_PROMPT",
            "PS1 contains a dynamic or unsupported prompt expansion, so the application cannot establish a safe ready boundary. Use the manual terminal or choose a simple non-executing prompt yourself in a fresh supported session. The existing prompt was preserved.",
        ),
        "unsupported_hook_attributes" => (
            "SHELL_UNSUPPORTED_HOOK_ATTRIBUTES",
            "PROMPT_COMMAND or PS0 has unsupported variable attributes, such as readonly, export, nameref or an unsupported array type. Review the hook declarations yourself or use the manual terminal. Existing hooks and attributes were preserved.",
        ),
        "missing_tools" => (
            "SHELL_UNSUPPORTED_MISSING_TOOLS",
            "The current shell cannot find base64, which is required for bounded command metadata. Ask the server administrator to make it available, or use the manual terminal. The application did not install software or change PATH.",
        ),
        _ => (
            "SHELL_UNSUPPORTED",
            "The shell reported an unrecognized integration limitation. Use the manual terminal and review the supported Bash requirements before reconnecting. No automatic repair or retry was attempted.",
        ),
    };
    error(code, message)
}
/// Correlation-only metadata for a host-validated COMPLETE single-line command.
/// The inert comment is visible in the PTY and makes repeated actions distinct
/// under Bash's default ignoredups. It does not wrap/evaluate executable text.
/// The execution gate must bind these exact bytes to its immutable action before
/// dispatch. Arbitrary/partial shell syntax is not eligible for this operation.
pub fn correlated_command(action_id: &str, command: &str) -> Result<String, ApiError> {
    if action_id.is_empty()
        || action_id.len() > 128
        || !action_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        || command.is_empty()
        || command.len() > 8192
        || command.starts_with(char::is_whitespace)
        || command.chars().any(char::is_control)
    {
        return Err(error("SHELL_ACTION_INVALID", "A complete bounded single-line command without leading whitespace is required for current-shell correlation."));
    }
    Ok(format!("{command} # xyz-action:{action_id}"))
}
impl Default for ShellIntegrationService {
    fn default() -> Self {
        Self::new()
    }
}
impl ShellIntegrationService {
    pub fn new() -> Self {
        let status = IntegrationStatus {
            session: None,
            shell_id: None,
            phase: IntegrationPhase::Unintegrated,
            sequence: 0,
            revision: 0,
            active_action_id: None,
            completed: None,
            shadowed_commands: vec![],
            error: None,
        };
        let (updates, _) = watch::channel(status.clone());
        Self {
            inner: Arc::new(Mutex::new(Capture {
                parser: ProtocolParser::new(&Uuid::new_v4().simple().to_string()).unwrap(),
                parser_session: None,
                echo_filter: EchoMetadataFilter::default(),
                status,
                attempted: BTreeSet::new(),
                hello: false,
                deadline: None,
                pending: None,
                active_sequence: None,
            })),
            updates,
            log: Arc::new(Mutex::new(None)),
        }
    }
    pub fn attach_log(&self, log: ShellLogService) {
        *self.log.lock().unwrap_or_else(|p| p.into_inner()) = Some(log);
    }
    pub fn subscribe(&self) -> watch::Receiver<IntegrationStatus> {
        self.updates.subscribe()
    }
    fn publish(&self, c: &mut Capture) {
        c.status.revision = c.status.revision.saturating_add(1);
        self.updates.send_replace(c.status.clone());
    }
    fn unknown(&self, c: &mut Capture, code: &str, message: &str) {
        c.status.phase = IntegrationPhase::Unknown;
        c.status.error = Some(error(code, message));
        c.status.shadowed_commands.clear();
        c.deadline = None;
        c.pending = None;
        c.active_sequence = None;
        self.publish(c);
    }
    pub fn status(&self) -> IntegrationStatus {
        let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if c.deadline.is_some_and(|d| Instant::now() >= d) {
            self.unknown(&mut c, "SHELL_INTEGRATION_TIMEOUT", "The current-shell handshake or first ready boundary did not arrive. Automatic input stopped; verify the foreground before any recovery.");
        }
        c.status.clone()
    }
    /// A connection/PTY is NOT an idle-shell assertion. The initial confirmation
    /// is required before even the capability probe; never replay this bootstrap.
    pub fn confirm_install(
        &self,
        snapshot: SessionSnapshot,
        readiness_confirmed: bool,
    ) -> Result<Option<String>, ApiError> {
        let session = snapshot
            .session
            .filter(|_| snapshot.state == SessionState::Connected)
            .ok_or_else(|| error("SHELL_INTEGRATION_SESSION", "Connect a terminal first."))?;
        let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if c.status.session.as_ref() == Some(&session) && c.status.phase == IntegrationPhase::Ready
        {
            return Ok(None);
        }
        if !readiness_confirmed {
            return Err(error("SHELL_INTEGRATION_CONFIRMATION", "Confirm that the current foreground is an idle supported Bash prompt before installing temporary hooks."));
        }
        let key = (session.session_id.clone(), session.generation.clone());
        if c.attempted.contains(&key) {
            return Err(error("SHELL_INTEGRATION_ALREADY_ATTEMPTED", "Integration was already attempted on this connection. Keep using the manual terminal; reconnect and verify its foreground before installing again."));
        }
        if c.attempted.len() >= 1024 {
            return Err(error(
                "SHELL_INTEGRATION_LIMIT",
                "Restart the application before integrating another shell.",
            ));
        }
        let nonce = Uuid::new_v4().simple().to_string();
        let bootstrap = bootstrap_command(&nonce).map_err(|_| {
            error(
                "SHELL_INTEGRATION_BOOTSTRAP",
                "Cannot prepare the current-shell integration.",
            )
        })?;
        c.attempted.insert(key);
        c.parser = ProtocolParser::new(&nonce).unwrap();
        c.parser_session = Some(session.clone());
        c.echo_filter = EchoMetadataFilter::default();
        c.status.session = Some(session);
        c.status.shell_id = Some(nonce);
        c.status.phase = IntegrationPhase::Installing;
        c.status.sequence = 0;
        c.status.active_action_id = None;
        c.status.completed = None;
        c.status.shadowed_commands.clear();
        c.status.error = None;
        c.hello = false;
        c.pending = None;
        c.active_sequence = None;
        c.deadline = Some(Instant::now() + Duration::from_secs(15));
        self.publish(&mut c);
        Ok(Some(bootstrap))
    }
    pub fn fail_install(&self, session: &SessionRef, _cause: ApiError) {
        let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if c.status.session.as_ref() == Some(session) {
            self.unknown(&mut c, "SHELL_INTEGRATION_DELIVERY", "Integration delivery failed or is uncertain. No retry was sent; check the manual terminal.");
        }
    }
    pub fn ready_snapshot(&self, session: &SessionRef) -> Result<ReadyBinding, ApiError> {
        let s = self.status();
        if s.session.as_ref() != Some(session) || s.phase != IntegrationPhase::Ready {
            return Err(error(
                "SHELL_NOT_READY",
                "A separate current-shell ready event is required before automatic input.",
            ));
        }
        Ok(ReadyBinding {
            shell_id: s.shell_id.expect("ready shell"),
            sequence: s.sequence,
            revision: s.revision,
            shadowed_commands: s.shadowed_commands,
        })
    }
    pub fn prepare_dispatch(
        &self,
        session: &SessionRef,
        action_id: &str,
        command: &str,
        expected: &ReadyBinding,
    ) -> Result<DispatchBinding, ApiError> {
        let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if c.status.session.as_ref() != Some(session)
            || c.status.phase != IntegrationPhase::Ready
            || c.status.revision != expected.revision
            || c.status.sequence != expected.sequence
            || c.status.shell_id.as_deref() != Some(expected.shell_id.as_str())
        {
            return Err(error("SHELL_CONTEXT_CHANGED", "The shell context changed after this action was prepared. Prepare a new action; the old approval cannot be reused."));
        }
        let transport_command = correlated_command(action_id, command)?;
        let sequence = c.status.sequence.checked_add(1).ok_or_else(|| {
            error(
                "SHELL_SEQUENCE_LIMIT",
                "Shell sequence exhausted; reconnect before automatic execution.",
            )
        })?;
        let binding = DispatchBinding {
            shell_id: expected.shell_id.clone(),
            sequence,
            revision: expected.revision,
            action_id: action_id.into(),
            command: command.into(),
            transport_command,
        };
        c.echo_filter.register(action_id);
        c.pending = Some(binding.clone());
        c.status.active_action_id = Some(action_id.into());
        c.status.completed = None;
        c.status.phase = IntegrationPhase::AwaitingBegin;
        self.publish(&mut c);
        Ok(binding)
    }
    /// Called by the sole SSH writer immediately before the first byte. This
    /// checks correlation only; the local execution gate owns all authorization.
    pub fn validate_dispatch(
        &self,
        session: &SessionRef,
        binding: &DispatchBinding,
    ) -> Result<(), ApiError> {
        let c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if c.status.session.as_ref() != Some(session)
            || c.status.phase != IntegrationPhase::AwaitingBegin
            || c.status.shell_id.as_deref() != Some(binding.shell_id.as_str())
            || !c.pending.as_ref().is_some_and(|p| {
                p.action_id == binding.action_id
                    && p.sequence == binding.sequence
                    && p.revision == binding.revision
                    && p.command == binding.command
                    && p.transport_command == binding.transport_command
            })
        {
            return Err(error(
                "SHELL_DISPATCH_STALE",
                "The shell changed before input could be sent. This action was not dispatched.",
            ));
        }
        Ok(())
    }
    pub fn dispatch_failed(&self, session: &SessionRef, action_id: &str, cause: &str) {
        let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if c.status.session.as_ref() == Some(session)
            && c.status.active_action_id.as_deref() == Some(action_id)
        {
            if cause == "not_sent"
                && c.status.phase == IntegrationPhase::AwaitingBegin
                && c.active_sequence.is_none()
                && c.pending.as_ref().is_some_and(|p| p.action_id == action_id)
            {
                c.pending = None;
                c.status.active_action_id = None;
                c.status.phase = IntegrationPhase::Ready;
                self.publish(&mut c);
            } else {
                self.unknown(
                    &mut c,
                    "SHELL_DISPATCH_UNKNOWN",
                    "Command delivery or lifecycle is uncertain. No automatic retry was sent.",
                );
            }
        }
    }
    /// Metadata filtering runs regardless of logs, paused collection, capacity,
    /// or disk failure. Original SSH bytes still go to the visible terminal.
    pub fn ingest(&self, session: &SessionRef, bytes: &[u8]) -> Vec<u8> {
        let (parsed, events) = {
            let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            if c.deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                self.unknown(&mut c, "SHELL_INTEGRATION_TIMEOUT", "The current-shell installation timed out. Late metadata cannot restore readiness; check the manual terminal.");
            }
            if c.parser_session
                .as_ref()
                .is_some_and(|previous| previous != session)
            {
                let previous = c.parser_session.as_ref().unwrap();
                if !session
                    .generation
                    .parse::<u64>()
                    .ok()
                    .zip(previous.generation.parse::<u64>().ok())
                    .is_some_and(|(next, old)| next > old)
                {
                    return vec![];
                }
                c.parser = ProtocolParser::new(&Uuid::new_v4().simple().to_string()).unwrap();
                c.echo_filter = EchoMetadataFilter::default();
            }
            c.parser_session = Some(session.clone());
            let mut parsed = c.parser.feed(bytes);
            parsed.visible = c.echo_filter.feed(&parsed.visible);
            let mut events = Vec::with_capacity(parsed.events.len());
            for event in &parsed.events {
                let mut log_event = event.clone();
                if c.status.session.as_ref() == Some(session) {
                    if c.status.phase == IntegrationPhase::AwaitingBegin {
                        if let (
                            WireEvent::Begin {
                                sequence, command, ..
                            },
                            Some(binding),
                        ) = (event, c.pending.as_ref())
                        {
                            if *sequence == binding.sequence
                                && *command == binding.transport_command
                            {
                                if let WireEvent::Begin { command, .. } = &mut log_event {
                                    *command = binding.command.clone();
                                }
                            }
                        }
                    }
                    self.event(&mut c, event);
                }
                events.push(log_event);
            }
            (parsed, events)
        };
        if let Some(log) = self.log.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            for event in events {
                log.ingest_event(session, event);
            }
        }
        parsed.visible
    }
    fn event(&self, c: &mut Capture, event: &WireEvent) {
        use IntegrationPhase::*;
        // A known lost boundary is terminal for automatic execution. Later bytes
        // can neither repair it nor revive an old action/approval.
        if matches!(
            c.status.phase,
            Unknown | Unsupported | Disconnected | Unintegrated
        ) {
            return;
        }
        match event {
            WireEvent::Hello { shell_version } => {
                if c.status.phase != Installing || c.hello || !(shell_version.starts_with("5.1.") || shell_version.starts_with("5.2.")) { self.unknown(c, "SHELL_HANDSHAKE_INVALID", "The current-shell handshake did not match the supported integration."); return; }
                c.hello = true;
                c.status.phase = AwaitingReady;
                self.publish(c);
            }
            WireEvent::Unsupported { reason } => {
                c.status.phase = Unsupported;
                c.status.error = Some(unsupported_shell(reason));
                c.deadline = None;
                c.pending = None;
                c.active_sequence = None;
                c.status.shadowed_commands.clear();
                self.publish(c);
            }
            WireEvent::Ready { sequence, shadowed_commands, .. } => {
                if !c.hello || c.status.phase != AwaitingReady || c.active_sequence.is_some() || *sequence != c.status.sequence { self.unknown(c, "SHELL_READY_ORDER", "An out-of-order shell-ready event was discarded; automatic input stopped."); return; }
                c.deadline = None;
                c.status.phase = Ready;
                c.status.shadowed_commands = shadowed_commands.clone();
                c.status.active_action_id = None;
                c.pending = None;
                self.publish(c);
            }
            WireEvent::Begin { sequence, command, .. } => {
                if !c.hello || !matches!(c.status.phase, Ready | AwaitingBegin) || c.active_sequence.is_some() || c.status.sequence.checked_add(1) != Some(*sequence) { self.unknown(c, "SHELL_BEGIN_ORDER", "Command start correlation was lost; automatic input stopped."); return; }
                if c.pending.as_ref().is_some_and(|p| p.sequence != *sequence || p.transport_command != *command) { self.unknown(c, "SHELL_INPUT_CONFLICT", "The observed command did not match the prepared action. Manual/AI input may have overlapped; automatic input stopped."); return; }
                if c.pending.is_none() { c.status.completed = None; }
                c.status.sequence = *sequence;
                c.active_sequence = Some(*sequence);
                c.status.phase = Running;
                self.publish(c);
            }
            WireEvent::End { sequence, exit_code, .. } => {
                if !c.hello || c.status.phase != Running || c.active_sequence != Some(*sequence) { self.unknown(c, "SHELL_END_ORDER", "Command completion correlation was lost; automatic input stopped."); return; }
                c.active_sequence = None;
                c.status.phase = AwaitingReady;
                if let Some(binding) = c.pending.as_ref() {
                    c.status.completed = Some(CommandCompletion { action_id: binding.action_id.clone(), sequence: *sequence, exit_code: *exit_code });
                }
                self.publish(c);
            }
            WireEvent::Closed => { c.status.phase = Disconnected; c.deadline = None; c.pending = None; c.active_sequence = None; self.publish(c); }
            WireEvent::Gap { .. } | WireEvent::Malformed { .. } => self.unknown(c, "SHELL_METADATA_GAP", "Shell metadata was missing, changed, malformed, or oversized. No completion or readiness was inferred."),
        }
    }
    pub fn disconnect(&self, session: &SessionRef) {
        let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if c.status.session.as_ref() != Some(session) {
            return;
        }
        c.parser.finish();
        c.status.phase = IntegrationPhase::Disconnected;
        c.status.error = Some(error(
            "SHELL_DISCONNECTED",
            "The shell disconnected; unfinished command outcomes remain unknown.",
        ));
        c.status.active_action_id = None;
        c.status.completed = None;
        c.status.shadowed_commands.clear();
        c.pending = None;
        c.active_sequence = None;
        c.deadline = None;
        c.hello = false;
        self.publish(&mut c);
    }
}

#[cfg(test)]
#[path = "shell_integration_tests.rs"]
mod tests;
