//! Host-owned single-writer authority. A queued AI frame is revalidated by the
//! SSH actor immediately before its first write; UI/model text grants no power.
use crate::ssh::{ApiError, SessionRef};
use crate::task_secrets::SecretInput;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;
use zeroize::Zeroizing;

const ACTION_TTL: Duration = Duration::from_secs(120);
pub const MAX_COMMAND_BYTES: usize = 8192;
const AUTO_READ_COMMANDS: &[&str] = &[
    "pwd", "whoami", "id", "uname", "uname -a", "ls", "ls -la", "hostname", "uptime", "date",
    "df -h", "free -h",
];
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedAction {
    #[serde(default = "command_kind")]
    pub kind: String,
    #[serde(default)]
    pub secret_field: Option<String>,
    pub action_id: String,
    pub display_revision: String,
    pub command_text: String,
    pub append_enter: bool,
    pub digest: String,
    pub policy: String,
    pub reason: String,
    pub approval_id: Option<String>,
    pub expires_at_unix_ms: u64,
    pub displayed: bool,
    pub delivery: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Owner {
    pub task_id: String,
    pub run_id: String,
    pub session: SessionRef,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Prepared,
    Queued,
    Started,
    Settled,
}
struct Pending {
    action: PreparedAction,
    deadline: Instant,
    approved: bool,
    stage: Stage,
    secret: Option<SecretInput>,
}
struct Lease {
    owner: Owner,
    cancelled: bool,
    ready: bool,
    pending: Option<Pending>,
}
#[derive(Default)]
struct State {
    lease: Option<Lease>,
    uncertain: Option<Owner>,
}
#[derive(Clone, Default)]
pub(crate) struct ExecutionGate {
    state: Arc<Mutex<State>>,
}
/// Cannot be constructed from IPC or deserialized. Carries exact immutable
/// bytes and authorization identity, never a frontend `approved` flag.
pub(crate) struct DispatchPermit {
    gate: ExecutionGate,
    owner: Owner,
    action_id: String,
    digest: String,
    pub(crate) bytes: Zeroizing<Vec<u8>>,
    pub(crate) started: Arc<AtomicBool>,
    pub(crate) on_started: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    disk_validate: Option<Arc<dyn Fn() -> Result<(), ApiError> + Send + Sync>>,
    disk_validated: AtomicBool,
}
fn command_kind() -> String {
    "command".into()
}
fn stale() -> ApiError {
    ApiError::new(
        "ACTION_STALE",
        "This action or run is no longer current. Nothing was sent.",
    )
}
fn lease<'a>(s: &'a mut State, owner: &Owner) -> Result<&'a mut Lease, ApiError> {
    s.lease
        .as_mut()
        .filter(|l| &l.owner == owner)
        .ok_or_else(stale)
}
impl ExecutionGate {
    pub(crate) fn acquire(&self, owner: Owner, readiness: bool) -> Result<(), ApiError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.lease.is_some() {
            return Err(ApiError::new(
                "AI_OWNER_BUSY",
                "Another terminal task owns AI input. Stop it or finish its review first.",
            ));
        }
        if s.uncertain
            .as_ref()
            .is_some_and(|u| u.session == owner.session)
        {
            return Err(ApiError::new("TERMINAL_OUTCOME_UNKNOWN", "Inspect the terminal and explicitly confirm readiness on the previous task before another AI write."));
        }
        if !readiness {
            return Err(ApiError::new(
                "TERMINAL_READINESS_REQUIRED",
                "Confirm that the current terminal is at an empty shell prompt before starting.",
            ));
        }
        // A new physical PTY cannot inherit uncertainty or approval from an old one.
        s.uncertain = None;
        s.lease = Some(Lease {
            owner,
            cancelled: false,
            ready: true,
            pending: None,
        });
        Ok(())
    }
    pub(crate) fn prepare(&self, owner: &Owner, command: &str) -> Result<PreparedAction, ApiError> {
        validate_command(command)?;
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let l = lease(&mut s, owner)?;
        if l.cancelled || !l.ready || l.pending.is_some() {
            return Err(stale());
        }
        let auto = AUTO_READ_COMMANDS.contains(&command);
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(b'\r');
        let action = PreparedAction {
            kind: "command".into(), secret_field: None,
            action_id: Uuid::new_v4().to_string(), display_revision: "1".into(), command_text: command.into(), append_enter: true,
            digest: format!("{:x}", Sha256::digest(&bytes)),
            policy: if auto { "auto_read_only" } else { "require_approval" }.into(),
            reason: if auto { "Exact small read-only command allowlist; shell aliases and manual interference remain possible." } else { "Command effects are not established as read-only. Review its full effect, current terminal, and target before allowing this one command." }.into(),
            approval_id: (!auto).then(|| Uuid::new_v4().to_string()),
            expires_at_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64 + ACTION_TTL.as_millis() as u64,
            displayed: false, delivery: "not_sent".into(),
        };
        l.pending = Some(Pending {
            action: action.clone(),
            deadline: Instant::now() + ACTION_TTL,
            approved: false,
            stage: Stage::Prepared,
            secret: None,
        });
        Ok(action)
    }
    /// Only a host UI-confirmed remote prompt can prepare this adapter. No
    /// command substitution or model-selected bytes are supported.
    pub(crate) fn prepare_secret(
        &self,
        owner: &Owner,
        field: &str,
        purpose: &str,
        secret: SecretInput,
    ) -> Result<PreparedAction, ApiError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let l = lease(&mut s, owner)?;
        if l.cancelled
            || l.pending
                .as_ref()
                .is_some_and(|p| p.stage != Stage::Settled)
        {
            return Err(stale());
        }
        if !(secret.current)() {
            return Err(stale());
        }
        let action = PreparedAction {
            kind: "secret_input".into(), secret_field: Some(field.into()),
            action_id: Uuid::new_v4().to_string(), display_revision: secret.snapshot_id.clone(),
            command_text: format!("[Secret input: {field}]"), append_enter: true,
            // Random opaque identity, not a password hash/guessing oracle.
            digest: Uuid::new_v4().to_string(), policy: "require_approval".into(),
            reason: format!("Send field {field} for: {purpose}. Confirm the visible remote program is waiting for this secret. The remote host receives the value and may echo, record, or misuse it. One Enter is appended. This never verifies remote echo settings or completion."),
            approval_id: Some(Uuid::new_v4().to_string()),
            expires_at_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64 + ACTION_TTL.as_millis() as u64,
            displayed: false, delivery: "not_sent".into(),
        };
        l.ready = true;
        l.pending = Some(Pending {
            action: action.clone(),
            deadline: Instant::now() + ACTION_TTL,
            approved: false,
            stage: Stage::Prepared,
            secret: Some(secret),
        });
        Ok(action)
    }
    pub(crate) fn acknowledge(
        &self,
        owner: &Owner,
        id: &str,
        revision: &str,
    ) -> Result<(), ApiError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let l = lease(&mut s, owner)?;
        if l.cancelled {
            return Err(stale());
        }
        let p = l
            .pending
            .as_mut()
            .filter(|p| {
                p.action.action_id == id
                    && p.action.display_revision == revision
                    && p.stage == Stage::Prepared
                    && Instant::now() < p.deadline
            })
            .ok_or_else(stale)?;
        p.action.displayed = true;
        Ok(())
    }
    pub(crate) fn approve(&self, owner: &Owner, id: &str, approval: &str) -> Result<(), ApiError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let l = lease(&mut s, owner)?;
        if l.cancelled {
            return Err(stale());
        }
        let p = l
            .pending
            .as_mut()
            .filter(|p| {
                p.action.action_id == id
                    && p.action.approval_id.as_deref() == Some(approval)
                    && p.action.displayed
                    && p.stage == Stage::Prepared
                    && !p.approved
                    && Instant::now() < p.deadline
            })
            .ok_or_else(stale)?;
        p.approved = true;
        Ok(())
    }
    pub(crate) fn queue(&self, owner: &Owner) -> Result<DispatchPermit, ApiError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let l = lease(&mut s, owner)?;
        if l.cancelled || !l.ready {
            return Err(stale());
        }
        let p = l
            .pending
            .as_mut()
            .filter(|p| {
                p.stage == Stage::Prepared
                    && p.action.displayed
                    && (p.action.approval_id.is_none() || p.approved)
                    && Instant::now() < p.deadline
            })
            .ok_or_else(stale)?;
        let bytes = if let Some(secret) = &p.secret {
            if !(secret.current)() {
                return Err(stale());
            }
            secret.bytes.clone()
        } else {
            let mut bytes = Zeroizing::new(p.action.command_text.as_bytes().to_vec());
            bytes.push(b'\r');
            bytes
        };
        p.stage = Stage::Queued;
        p.action.delivery = "queued".into();
        Ok(DispatchPermit {
            gate: self.clone(),
            owner: owner.clone(),
            action_id: p.action.action_id.clone(),
            digest: p.action.digest.clone(),
            bytes,
            on_started: None,
            disk_validate: p.secret.as_ref().map(|secret| secret.validate.clone()),
            disk_validated: AtomicBool::new(false),
            started: Arc::new(AtomicBool::new(false)),
        })
    }
    pub(crate) fn action(&self, owner: &Owner) -> Option<PreparedAction> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.lease
            .as_ref()
            .filter(|l| &l.owner == owner)
            .and_then(|l| l.pending.as_ref())
            .map(|p| p.action.clone())
    }
    /// Returns true while a frame has crossed its dispatch boundary. Its owner
    /// stays held until the actor reports/drops the receipt.
    pub(crate) fn cancel(&self, owner: &Owner) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(l) = lease(&mut s, owner) else {
            return false;
        };
        l.cancelled = true;
        let started = l
            .pending
            .as_ref()
            .is_some_and(|p| p.stage == Stage::Started);
        if !started {
            s.lease = None;
        }
        started
    }
    pub(crate) fn settle(&self, owner: &Owner, delivery: &str) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(l) = lease(&mut s, owner) else {
            return;
        };
        if let Some(p) = l.pending.as_mut() {
            p.stage = Stage::Settled;
            p.action.delivery = delivery.into();
        }
        let cancelled = l.cancelled;
        if delivery == "not_sent" && s.uncertain.as_ref() == Some(owner) {
            s.uncertain = None;
        }
        if cancelled {
            s.lease = None;
        }
    }
    pub(crate) fn is_owner(&self, owner: &Owner) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lease
            .as_ref()
            .is_some_and(|l| &l.owner == owner)
    }
    pub(crate) fn release(&self, owner: &Owner) {
        self.cancel(owner);
    }
    pub(crate) fn confirm_unwritten_ready(&self, owner: &Owner) -> Result<bool, ApiError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let l = lease(&mut s, owner)?;
        if l.cancelled || !l.ready || l.pending.is_some() {
            return Err(stale());
        }
        Ok(true)
    }
    pub(crate) fn confirm_ready(&self, owner: &Owner) -> Result<bool, ApiError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.uncertain.as_ref() != Some(owner) {
            return Err(stale());
        }
        if let Some(l) = s.lease.as_mut() {
            if &l.owner != owner
                || l.pending
                    .as_ref()
                    .is_some_and(|p| p.stage == Stage::Started || p.stage == Stage::Queued)
            {
                return Err(stale());
            }
            l.ready = true;
            l.pending = None;
            s.uncertain = None;
            return Ok(true);
        }
        s.uncertain = None;
        Ok(false)
    }
}
impl DispatchPermit {
    /// Disk reads run on the blocking pool, outside both SSH-state and execution
    /// locks. The actor then rechecks all in-memory authority at begin().
    pub(crate) async fn validate_before_begin(&self) -> Result<(), ApiError> {
        if let Some(validate) = self.disk_validate.clone() {
            tokio::task::spawn_blocking(move || validate())
                .await
                .map_err(|_| stale())??;
            self.disk_validated.store(true, Ordering::Release);
        }
        Ok(())
    }
    pub(crate) fn begin(&self, actual_session: &SessionRef) -> Result<(), ApiError> {
        let mut s = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        let l = lease(&mut s, &self.owner)?;
        let p = l.pending.as_mut().ok_or_else(stale)?;
        if actual_session != &self.owner.session
            || l.cancelled
            || !l.ready
            || p.stage != Stage::Queued
            || !p.action.displayed
            || Instant::now() >= p.deadline
            || p.action.action_id != self.action_id
            || p.action.digest != self.digest
            || (p.secret.is_none()
                && format!("{:x}", Sha256::digest(&self.bytes[..])) != self.digest)
            || p.secret
                .as_ref()
                .is_some_and(|secret| secret.bytes.as_slice() != self.bytes.as_slice())
            || (p.action.approval_id.is_some() && !p.approved)
        {
            return Err(stale());
        }
        if let Some(secret) = &p.secret {
            if !self.disk_validated.load(Ordering::Acquire) || !(secret.current)() {
                return Err(stale());
            }
        }
        p.approved = false;
        p.stage = Stage::Started;
        p.action.delivery = "unknown".into();
        l.ready = false;
        s.uncertain = Some(self.owner.clone());
        self.started.store(true, Ordering::Release);
        Ok(())
    }
    /// Called only after the SSH-state and execution locks have been released.
    pub(crate) fn notify_started(&self) -> Result<(), ApiError> {
        if self.on_started.as_ref().is_some_and(|notify| !notify()) {
            return Err(ApiError::new(
                "NOT_SENT",
                "The action display is no longer available. Nothing was sent.",
            ));
        }
        Ok(())
    }
}
pub(crate) fn validate_command(command: &str) -> Result<(), ApiError> {
    if command.trim().is_empty() || command.len() > MAX_COMMAND_BYTES || command.chars().any(|c| c.is_control() || matches!(c, '\u{00ad}' | '\u{034f}' | '\u{180e}' | '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}' | '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) {
        return Err(ApiError::new("ACTION_INVALID", "A shell command must be one nonempty line, at most 8192 UTF-8 bytes, without control or bidi characters. Nothing was sent."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner() -> Owner {
        Owner {
            task_id: "task".into(),
            run_id: "run".into(),
            session: SessionRef {
                session_id: "session".into(),
                generation: "1".into(),
            },
        }
    }
    fn prepared(command: &str) -> (ExecutionGate, Owner, PreparedAction) {
        let g = ExecutionGate::default();
        let o = owner();
        g.acquire(o.clone(), true).unwrap();
        let a = g.prepare(&o, command).unwrap();
        (g, o, a)
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_disk_validation_never_holds_the_execution_gate() {
        let gate = ExecutionGate::default();
        let owner = owner();
        gate.acquire(owner.clone(), true).unwrap();
        let entered = Arc::new(AtomicBool::new(false));
        let entered_worker = entered.clone();
        let (release, receiver) = std::sync::mpsc::channel();
        let receiver = Mutex::new(receiver);
        let secret = SecretInput {
            bytes: Zeroizing::new(b"synthetic\r".to_vec()),
            snapshot_id: "revision".into(),
            current: Arc::new(|| true),
            validate: Arc::new(move || {
                entered_worker.store(true, Ordering::Release);
                receiver.lock().unwrap().recv().unwrap();
                Ok(())
            }),
        };
        let action = gate
            .prepare_secret(&owner, "password", "fixture", secret)
            .unwrap();
        gate.acknowledge(&owner, &action.action_id, &action.display_revision)
            .unwrap();
        gate.approve(
            &owner,
            &action.action_id,
            action.approval_id.as_ref().unwrap(),
        )
        .unwrap();
        let permit = gate.queue(&owner).unwrap();
        let validation = tokio::spawn(async move {
            permit.validate_before_begin().await.unwrap();
            permit
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !entered.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let cancelled_gate = gate.clone();
        let cancelled_owner = owner.clone();
        let cancellation = tokio::time::timeout(
            Duration::from_secs(1),
            tokio::task::spawn_blocking(move || cancelled_gate.cancel(&cancelled_owner)),
        )
        .await;
        release.send(()).unwrap();
        assert!(!cancellation
            .expect("cancellation must not wait for disk validation")
            .unwrap());
        let permit = validation.await.unwrap();
        assert!(permit.begin(&owner.session).is_err());
    }
    #[test]
    fn approval_display_and_replay_are_enforced() {
        let (g, o, a) = prepared("rm -rf /tmp/example");
        assert!(g.queue(&o).is_err());
        assert!(g
            .approve(&o, &a.action_id, a.approval_id.as_ref().unwrap())
            .is_err());
        g.acknowledge(&o, &a.action_id, &a.display_revision)
            .unwrap();
        assert!(g.queue(&o).is_err());
        g.approve(&o, &a.action_id, a.approval_id.as_ref().unwrap())
            .unwrap();
        assert!(g
            .approve(&o, &a.action_id, a.approval_id.as_ref().unwrap())
            .is_err());
        let p = g.queue(&o).unwrap();
        assert!(g.queue(&o).is_err());
        p.begin(&o.session).unwrap();
        assert!(p.begin(&o.session).is_err());
        g.settle(&o, "write_accepted");
        assert!(g.prepare(&o, "pwd").is_err());
        g.confirm_ready(&o).unwrap();
        assert!(g.prepare(&o, "pwd").is_ok());
    }
    #[test]
    fn queued_cancel_is_not_sent_and_unknown_survives_owner_release() {
        let (g, o, a) = prepared("pwd");
        g.acknowledge(&o, &a.action_id, "1").unwrap();
        let p = g.queue(&o).unwrap();
        assert!(!g.cancel(&o));
        assert!(p.begin(&o.session).is_err());
        g.acquire(o.clone(), true).unwrap();
        let a = g.prepare(&o, "pwd").unwrap();
        g.acknowledge(&o, &a.action_id, "1").unwrap();
        let p = g.queue(&o).unwrap();
        p.begin(&o.session).unwrap();
        assert!(g.cancel(&o));
        assert!(g.acquire(owner(), true).is_err());
        g.settle(&o, "unknown");
        assert_eq!(
            g.acquire(owner(), true).unwrap_err().code,
            "TERMINAL_OUTCOME_UNKNOWN"
        );
        assert!(!g.confirm_ready(&o).unwrap());
        g.acquire(owner(), true).unwrap();
    }
    #[test]
    fn expiry_session_stale_and_concurrent_owner_fail_closed() {
        let (g, o, a) = prepared("pwd");
        assert!(g.acquire(o.clone(), true).is_err());
        g.acknowledge(&o, &a.action_id, "1").unwrap();
        let p = g.queue(&o).unwrap();
        let mut wrong = o.session.clone();
        wrong.generation = "2".into();
        assert!(p.begin(&wrong).is_err());
        g.state
            .lock()
            .unwrap()
            .lease
            .as_mut()
            .unwrap()
            .pending
            .as_mut()
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);
        assert!(p.begin(&o.session).is_err());
    }
    #[test]
    fn allowlist_is_exact_and_control_injection_rejected() {
        for command in ["pwd; rm /tmp/x", "pwd\n", "echo $(id)", "ls -l", "sudo id"] {
            if command.contains('\n') {
                assert!(validate_command(command).is_err());
            } else {
                assert_eq!(prepared(command).2.policy, "require_approval");
            }
        }
        for command in AUTO_READ_COMMANDS {
            assert_eq!(prepared(command).2.policy, "auto_read_only");
            for modified in [
                format!("{command}; id"),
                format!("{command} $(id)"),
                format!("{command} > out"),
                format!("{command} "),
            ] {
                assert_eq!(prepared(&modified).2.policy, "require_approval");
            }
        }
    }
    #[test]
    fn old_cancelled_readiness_cannot_clear_another_runs_unknown() {
        let (g, first, a) = prepared("pwd");
        g.acknowledge(&first, &a.action_id, "1").unwrap();
        g.queue(&first).unwrap().begin(&first.session).unwrap();
        g.cancel(&first);
        g.settle(&first, "unknown");
        g.confirm_ready(&first).unwrap();
        let mut second = first.clone();
        second.run_id = "second".into();
        g.acquire(second.clone(), true).unwrap();
        let a = g.prepare(&second, "pwd").unwrap();
        g.acknowledge(&second, &a.action_id, "1").unwrap();
        g.queue(&second).unwrap().begin(&second.session).unwrap();
        g.cancel(&second);
        g.settle(&second, "unknown");
        assert!(g.confirm_ready(&first).is_err());
        assert_eq!(
            g.acquire(first.clone(), true).unwrap_err().code,
            "TERMINAL_OUTCOME_UNKNOWN"
        );
        g.confirm_ready(&second).unwrap();
        assert!(g.confirm_ready(&second).is_err());
    }
    #[test]
    fn invisible_preview_controls_are_rejected_but_unicode_paths_remain_valid() {
        for hidden in [
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202e}', '\u{2066}', '\u{200b}', '\u{00ad}',
            '\u{034f}', '\u{180e}', '\u{feff}',
        ] {
            assert!(validate_command(&format!("echo safe{hidden}rm")).is_err());
        }
        assert!(validate_command("ls /tmp/中文文件").is_ok());
    }
    #[test]
    fn concurrent_acquisition_has_exactly_one_writer() {
        let gate = ExecutionGate::default();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let joins: Vec<_> = (0..8)
            .map(|index| {
                let gate = gate.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut o = owner();
                    o.run_id = index.to_string();
                    barrier.wait();
                    gate.acquire(o, true).is_ok()
                })
            })
            .collect();
        assert_eq!(
            joins
                .into_iter()
                .map(|j| usize::from(j.join().unwrap()))
                .sum::<usize>(),
            1
        );
    }
}
