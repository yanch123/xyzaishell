//! Cooperative CURRENT-shell lifecycle, independent of persistent logging.
//! Remote metadata is untrusted correlation evidence, never an approval or an
//! authenticated audit. No observation text, prompt shape, or silence is readiness.
use crate::{
    shell_log::ShellLogService,
    shell_log_protocol::{bootstrap_command, startup_command, ProtocolParser, WireEvent},
    ssh::{ApiError, SessionRef, SessionSnapshot, SessionState},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    AwaitingReceiver,
    Receiving,
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
    /// Manual audit capability; explicit AI action records remain exact.
    #[serde(default)]
    pub manual_tracking: Option<String>,
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
    pub payload_digest: String,
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
    fn register_transport(&mut self, transport: &str) {
        self.markers.push_back(transport.as_bytes().to_vec());
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
        "control_conflict" => ("SHELL_UNSUPPORTED_CONTROL_CONFLICT", "Shell 的控制原语存在同名函数或别名，无法可靠封装命令。原有定义已保留，可继续使用手动终端。"),
        "unsupported_bash" => (
            "SHELL_UNSUPPORTED_BASH",
            "自动执行需要交互式 Bash 4.2、5.1 或 5.2。当前环境不受支持，请继续使用手动终端；未修改 Shell 设置。",
        ),
        "namespace_conflict" => (
            "SHELL_UNSUPPORTED_NAMESPACE",
            "Shell 已定义 __xyz_ 开头的变量、函数或别名。为保留现有定义，已停止自动准备；可继续使用手动终端。",
        ),
        "unsupported_options" => (
            "SHELL_UNSUPPORTED_OPTIONS",
            "当前 Shell 选项或历史设置不支持自动执行。需要开启 history、cmdhist、promptvars、interactive_comments，HISTSIZE 为正，HISTIGNORE 为空，histchars 为默认值；不支持 errexit、functrace、allexport、restricted、extdebug 或 POSIX 模式。设置未改动，可继续使用手动终端。",
        ),
        "unsupported_trap" => (
            "SHELL_UNSUPPORTED_TRAP",
            "Shell 存在 EXIT 之外的 trap，无法可靠识别命令状态。已有 trap 已保留，可继续使用手动终端。",
        ),
        "dynamic_prompt" => (
            "SHELL_UNSUPPORTED_DYNAMIC_PROMPT",
            "PS1 包含动态或不支持的提示符展开，无法可靠确认 Shell 已就绪。原提示符已保留，可继续使用手动终端。",
        ),
        "unsupported_hook_attributes" => (
            "SHELL_UNSUPPORTED_HOOK_ATTRIBUTES",
            "PROMPT_COMMAND 或 PS0 包含不支持的属性，例如 readonly、export、nameref 或数组类型。原 hooks 和属性已保留，可继续使用手动终端。",
        ),
        "missing_tools" => (
            "SHELL_UNSUPPORTED_MISSING_TOOLS",
            "Shell 找不到命令接收所需的 base64、stty 或 sha256sum。未安装软件或修改 PATH，可继续使用手动终端。",
        ),
        _ => (
            "SHELL_UNSUPPORTED",
            "Shell 报告了未识别的准备限制。未自动修复或重试，可继续使用手动终端。",
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
/// A bounded top-level kickoff; original command bytes travel separately in
/// the same immutable permit's suffix-safe, quiet receiver frame.
pub const MAX_ACTION_TRANSPORT_BYTES: usize = 128 * 1024;
pub fn action_envelope(
    session: &SessionRef,
    action_id: &str,
    command: &str,
    expected: &ReadyBinding,
) -> Result<String, ApiError> {
    crate::execution::validate_command(command)?;
    let safe_id = |s: &str| {
        !s.is_empty()
            && s.len() <= 128
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    };
    if !safe_id(action_id)
        || !safe_id(&session.session_id)
        || !safe_id(&session.generation)
        || expected.shell_id.len() != 32
        || !expected
            .shell_id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(error(
            "SHELL_ACTION_INVALID",
            "命令封装身份无效，未发送任何输入。",
        ));
    }
    let sequence = expected
        .sequence
        .checked_add(1)
        .ok_or_else(|| error("SHELL_SEQUENCE_LIMIT", "Shell 序号已耗尽，请重新连接。"))?;
    let digest = format!("{:x}", Sha256::digest(command.as_bytes()));
    let transport = format!("if __xyz_receive '{}' '{}' '{}' {} '{}' \"$(builtin trap -p)\"; then builtin eval -- \"$__xyz_payload\"; __xyz_action_end \"$?\"; fi # xyz-action:{}", session.session_id, session.generation, action_id, sequence, digest, action_id);
    if transport.len() + 1 > MAX_ACTION_TRANSPORT_BYTES {
        return Err(error(
            "SHELL_ACTION_INVALID",
            "命令封装超过传输上限，未发送。",
        ));
    }
    Ok(transport)
}
/// Quiet body: every suffix is a Bash comment/blank, never executable text.
pub fn action_wire_frame(
    session: &SessionRef,
    action_id: &str,
    command: &str,
    expected: &ReadyBinding,
) -> Result<Vec<u8>, ApiError> {
    action_envelope(session, action_id, command, expected)?;
    let digest = format!("{:x}", Sha256::digest(command.as_bytes()));
    let record = format!(
        "v1|{}|{}|{}|{}|{}|{}|{}|{}",
        expected.shell_id,
        session.session_id,
        session.generation,
        action_id,
        expected.sequence + 1,
        digest,
        command.len(),
        command
    );
    let mut wire = Vec::with_capacity(record.len() * 8 + 2);
    wire.push(b'#');
    for byte in record.bytes() {
        for bit in (0..8).rev() {
            wire.push(if byte & (1 << bit) != 0 { b'#' } else { b' ' });
        }
    }
    wire.push(b'\n');
    if wire.len() > MAX_ACTION_TRANSPORT_BYTES {
        return Err(error("SHELL_ACTION_INVALID", "命令接收帧超过上限。"));
    }
    Ok(wire)
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
            manual_tracking: None,
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
            self.unknown(&mut c, "SHELL_INTEGRATION_TIMEOUT", "Shell 启动未在期限内完成。自动输入已停止；请查看终端，登录或启动程序可能正在等待手动处理。");
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
        if !readiness_confirmed {
            let status = self.status();
            if status.session.as_ref() == Some(&session) && status.phase == IntegrationPhase::Ready
            {
                return Ok(None);
            }
            return Err(error("SHELL_INTEGRATION_CONFIRMATION", "Confirm that the current foreground is an idle supported Bash prompt before installing temporary hooks."));
        }
        self.prepare_install(session, false)
    }
    /// Reserve the protocol before the connection actor requests its controlled
    /// PTY exec. This never asserts that an existing terminal is an idle shell.
    pub(crate) fn prepare_startup(&self, session: &SessionRef) -> Result<String, ApiError> {
        self.prepare_install(session.clone(), true)?.ok_or_else(|| {
            error(
                "SHELL_INTEGRATION_ALREADY_ATTEMPTED",
                "此连接已尝试准备 Shell；不会重复发送启动命令。",
            )
        })
    }
    fn prepare_install(
        &self,
        session: SessionRef,
        startup: bool,
    ) -> Result<Option<String>, ApiError> {
        let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if !startup
            && c.status.session.as_ref() == Some(&session)
            && c.status.phase == IntegrationPhase::Ready
        {
            return Ok(None);
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
        let bootstrap = (if startup {
            startup_command(&nonce)
        } else {
            bootstrap_command(&nonce)
        })
        .map_err(|_| {
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
        c.status.manual_tracking = None;
        c.status.error = None;
        c.hello = false;
        c.pending = None;
        c.active_sequence = None;
        c.deadline = Some(Instant::now() + Duration::from_secs(15));
        self.publish(&mut c);
        Ok(Some(bootstrap))
    }
    pub(crate) fn startup_unavailable(&self, session: &SessionRef, code: &str, message: &str) {
        let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if c.status.session.as_ref() == Some(session) {
            self.unknown(&mut c, code, message);
        }
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
                "Shell 尚未确认就绪，请查看终端启动状态；自动输入不会发送到正在等待输入的程序。",
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
        let transport_command = action_envelope(session, action_id, command, expected)?;
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
            payload_digest: format!("{:x}", Sha256::digest(command.as_bytes())),
        };
        c.echo_filter.register(action_id);
        c.echo_filter.register_transport(&binding.transport_command);
        c.pending = Some(binding.clone());
        c.status.active_action_id = Some(action_id.into());
        c.status.completed = None;
        c.status.phase = IntegrationPhase::AwaitingReceiver;
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
            || c.status.phase != IntegrationPhase::AwaitingReceiver
            || c.status.shell_id.as_deref() != Some(binding.shell_id.as_str())
            || !c.pending.as_ref().is_some_and(|p| {
                p.action_id == binding.action_id
                    && p.sequence == binding.sequence
                    && p.revision == binding.revision
                    && p.command == binding.command
                    && p.payload_digest == binding.payload_digest
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
    pub fn receiver_ready(&self, session: &SessionRef, action_id: &str) -> bool {
        let c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        c.status.session.as_ref() == Some(session)
            && c.status.phase == IntegrationPhase::Receiving
            && c.pending.as_ref().is_some_and(|p| p.action_id == action_id)
    }
    pub fn consume_receiver(&self, session: &SessionRef, action_id: &str) -> Result<(), ApiError> {
        let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if c.status.session.as_ref() != Some(session)
            || c.status.phase != IntegrationPhase::Receiving
            || !c.pending.as_ref().is_some_and(|p| p.action_id == action_id)
        {
            return Err(error(
                "SHELL_RECEIVER_STALE",
                "命令接收状态已失效，未继续发送。",
            ));
        }
        c.status.phase = IntegrationPhase::AwaitingBegin;
        self.publish(&mut c);
        Ok(())
    }
    pub fn dispatch_failed(&self, session: &SessionRef, action_id: &str, cause: &str) {
        let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if c.status.session.as_ref() == Some(session)
            && c.status.active_action_id.as_deref() == Some(action_id)
        {
            if cause == "not_sent"
                && c.status.phase == IntegrationPhase::AwaitingReceiver
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
        self.ingest_with_observer(session, bytes, |_| {})
    }
    /// Append filtered output before any lifecycle publication can wake an
    /// action consumer. The callback must not re-enter this integration service.
    pub(crate) fn ingest_with_observer(
        &self,
        session: &SessionRef,
        bytes: &[u8],
        observe: impl FnOnce(&[u8]),
    ) -> Vec<u8> {
        let (parsed, events) = {
            let mut c = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            if c.deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                self.unknown(&mut c, "SHELL_INTEGRATION_TIMEOUT", "Shell 准备超时，自动输入已停止。迟到的启动信息不会重新启用执行；请查看手动终端。");
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
            observe(&parsed.visible);
            let mut events = Vec::with_capacity(parsed.events.len());
            for event in &parsed.events {
                if c.status.session.as_ref() == Some(session) {
                    self.event(&mut c, event);
                }
                // Only the integration validator can convert explicit action
                // frames into logging events. No wrapper history is recorded.
                let log_event = match event {
                    WireEvent::ActionBegin {
                        sequence,
                        remote_time_s,
                        cwd,
                        command,
                        ..
                    } if c.status.phase == IntegrationPhase::Running
                        && c.active_sequence == Some(*sequence) =>
                    {
                        Some(WireEvent::Begin {
                            sequence: *sequence,
                            remote_time_s: *remote_time_s,
                            cwd: cwd.clone(),
                            command: command.clone(),
                        })
                    }
                    WireEvent::ActionEnd {
                        sequence,
                        remote_time_s,
                        exit_code,
                        ..
                    } if c.status.phase == IntegrationPhase::AwaitingReady => {
                        Some(WireEvent::End {
                            sequence: *sequence,
                            remote_time_s: *remote_time_s,
                            exit_code: *exit_code,
                        })
                    }
                    WireEvent::ActionBegin { sequence, .. }
                    | WireEvent::ActionEnd { sequence, .. } => Some(WireEvent::Gap {
                        sequence: *sequence,
                        reason: "action_mismatch".into(),
                    }),
                    WireEvent::Gap { sequence, reason }
                        if matches!(
                            reason.as_str(),
                            "legacy_history_fidelity"
                                | "legacy_unobserved_command"
                                | "legacy_async_command"
                        ) && c.status.phase != IntegrationPhase::AwaitingReady =>
                    {
                        Some(WireEvent::Gap {
                            sequence: *sequence,
                            reason: "action_mismatch".into(),
                        })
                    }
                    _ => Some(event.clone()),
                };
                if let Some(event) = log_event {
                    events.push(event);
                }
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
            WireEvent::ReceiverReady { sequence, session_id, generation, action_id, digest } => {
                let matches = c.pending.as_ref().is_some_and(|p| p.sequence == *sequence && p.action_id == *action_id && p.payload_digest == *digest) && c.status.session.as_ref().is_some_and(|s| s.session_id == *session_id && s.generation == *generation);
                if !c.hello || c.status.phase != AwaitingReceiver || !matches { self.unknown(c, "SHELL_RECEIVER_MISMATCH", "命令接收握手不匹配，自动输入已停止。"); return; }
                c.status.phase = Receiving; self.publish(c);
            }
            WireEvent::ActionBegin { sequence, session_id, generation, action_id, digest, command, .. } => {
                let matches = c.pending.as_ref().is_some_and(|p| p.sequence == *sequence && p.action_id == *action_id && p.payload_digest == *digest && p.command == *command)
                    && c.status.session.as_ref().is_some_and(|s| s.session_id == *session_id && s.generation == *generation);
                if !c.hello || c.status.phase != AwaitingBegin || c.active_sequence.is_some() || c.status.sequence.checked_add(1) != Some(*sequence) || !matches {
                    self.unknown(c, "SHELL_ACTION_BEGIN_MISMATCH", "命令开始信息与当前动作不匹配，自动执行已停止。"); return;
                }
                c.status.sequence = *sequence; c.active_sequence = Some(*sequence); c.status.phase = Running; self.publish(c);
            }
            WireEvent::ActionEnd { sequence, session_id, generation, action_id, digest, exit_code, .. } => {
                let matches = c.pending.as_ref().is_some_and(|p| p.sequence == *sequence && p.action_id == *action_id && p.payload_digest == *digest)
                    && c.status.session.as_ref().is_some_and(|s| s.session_id == *session_id && s.generation == *generation);
                if !c.hello || c.status.phase != Running || c.active_sequence != Some(*sequence) || !matches {
                    self.unknown(c, "SHELL_ACTION_END_MISMATCH", "命令结束信息与当前动作不匹配，结果未知。"); return;
                }
                c.active_sequence = None; c.status.phase = AwaitingReady;
                c.status.completed = Some(CommandCompletion { action_id: action_id.clone(), sequence: *sequence, exit_code: *exit_code }); self.publish(c);
            }
            WireEvent::Hello { shell_version } => {
                if c.status.phase != Installing || c.hello || !(shell_version.starts_with("4.2.") || shell_version.starts_with("5.1.") || shell_version.starts_with("5.2.")) { self.unknown(c, "SHELL_HANDSHAKE_INVALID", "The current-shell handshake did not match the supported integration."); return; }
                c.hello = true;
                c.status.manual_tracking = Some(if shell_version.starts_with("4.2.") { "prompt_gap_only" } else { "exact" }.into());
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
                if !c.hello || !matches!(c.status.phase, AwaitingReady | Ready) || c.active_sequence.is_some() || (c.status.phase == Ready && c.pending.is_some()) || *sequence != c.status.sequence { self.unknown(c, "SHELL_READY_ORDER", "An out-of-order shell-ready event was discarded; automatic input stopped."); return; }
                c.deadline = None;
                c.status.phase = Ready;
                c.status.shadowed_commands = shadowed_commands.clone();
                c.status.active_action_id = None;
                c.pending = None;
                self.publish(c);
            }
            WireEvent::Begin { sequence, .. } => {
                if !c.hello || !matches!(c.status.phase, Ready | AwaitingBegin) || c.active_sequence.is_some() || c.status.sequence.checked_add(1) != Some(*sequence) { self.unknown(c, "SHELL_BEGIN_ORDER", "Command start correlation was lost; automatic input stopped."); return; }
                if c.pending.is_some() { self.unknown(c, "SHELL_INPUT_CONFLICT", "The observed command did not match the prepared action. Manual/AI input may have overlapped; automatic input stopped."); return; }
                if c.pending.is_none() { c.status.completed = None; }
                c.status.sequence = *sequence;
                c.active_sequence = Some(*sequence);
                c.status.phase = Running;
                self.publish(c);
            }
            WireEvent::End { sequence, .. } => {
                if !c.hello || c.pending.is_some() || c.status.phase != Running || c.active_sequence != Some(*sequence) { self.unknown(c, "SHELL_END_ORDER", "Command completion correlation was lost; automatic input stopped."); return; }
                c.active_sequence = None;
                c.status.phase = AwaitingReady;
                self.publish(c);
            }
            WireEvent::Closed => { c.status.phase = Disconnected; c.deadline = None; c.pending = None; c.active_sequence = None; self.publish(c); }
            WireEvent::Gap { sequence, reason } if matches!(reason.as_str(), "legacy_history_fidelity" | "legacy_unobserved_command" | "legacy_async_command") && c.pending.is_none() && c.active_sequence.is_none() && c.status.phase == Ready && (*sequence == c.status.sequence || c.status.sequence.checked_add(1) == Some(*sequence)) => {
                // This is only a lost manual observation, never an AI outcome.
                // Invalidate prior ready bindings and wait for an independent R.
                c.status.sequence = *sequence;
                c.status.completed = None;
                c.status.active_action_id = None;
                c.status.phase = AwaitingReady;
                self.publish(c);
            }
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
