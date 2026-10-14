//! Host-only immutable task credentials. No value-bearing type implements
//! Serialize or Debug. Only names/presence and random revisions cross IPC.
use crate::{
    observation::ObservationBuffer,
    ssh::ApiError,
    storage::task_credentials::{
        SecretFieldRequest, TaskCredentialStatus, TaskCredentialStore, TaskSecretSnapshot,
    },
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use zeroize::Zeroizing;

pub struct TaskSecrets {
    store: TaskCredentialStore,
    snapshots: Mutex<BTreeMap<String, Arc<TaskSecretSnapshot>>>,
    statuses: Mutex<BTreeMap<String, TaskCredentialStatus>>,
    epoch: Arc<AtomicU64>,
}
pub(crate) struct SecretInput {
    pub bytes: Zeroizing<Vec<u8>>,
    pub validate: Arc<dyn Fn() -> Result<(), ApiError> + Send + Sync>,
    pub snapshot_id: String,
    pub current: Arc<dyn Fn() -> bool + Send + Sync>,
}
fn unavailable() -> ApiError {
    ApiError::new("TASK_SECRET_STALE", "Task credentials are missing, changed, or unavailable. Reload the private file and approve a new input; nothing was sent.")
}
impl TaskSecrets {
    pub fn open(root: &Path) -> Result<Self, ApiError> {
        Ok(Self {
            store: TaskCredentialStore::open(root).map_err(|_| unavailable())?,
            snapshots: Mutex::new(BTreeMap::new()),
            statuses: Mutex::new(BTreeMap::new()),
            epoch: Arc::new(AtomicU64::new(0)),
        })
    }
    pub fn local_edit_path(&self, task_id: &str) -> Result<PathBuf, ApiError> {
        self.store
            .local_edit_path(task_id)
            .map_err(|_| unavailable())
    }
    pub fn ensure(
        &self,
        task_id: &str,
        fields: &[SecretFieldRequest],
        expected_snapshot_id: Option<&str>,
    ) -> Result<TaskCredentialStatus, ApiError> {
        let mut snapshots = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        self.epoch.fetch_add(1, Ordering::AcqRel);
        let status = self
            .store
            .ensure_template(task_id, fields, expected_snapshot_id)
            .map_err(|_| unavailable())?;
        snapshots.remove(task_id);
        self.statuses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(task_id.into(), status.clone());
        Ok(status)
    }
    pub fn reload(
        &self,
        task_id: &str,
        observation: &ObservationBuffer,
    ) -> Result<TaskCredentialStatus, ApiError> {
        let mut snapshots = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        self.epoch.fetch_add(1, Ordering::AcqRel);
        snapshots.remove(task_id);
        let snapshot = Arc::new(
            self.store
                .load_snapshot(task_id)
                .map_err(|_| unavailable())?,
        );
        register(&snapshot, task_id, observation)?;
        let status = snapshot.status();
        snapshots.insert(task_id.to_owned(), snapshot);
        self.statuses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(task_id.into(), status.clone());
        Ok(status)
    }
    pub fn status(&self, task_id: &str) -> Option<TaskCredentialStatus> {
        self.statuses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(task_id)
            .cloned()
    }
    pub(crate) fn prepare_context(
        &self,
        task_id: &str,
        observation: &ObservationBuffer,
    ) -> Result<(), ApiError> {
        // Read an existing private task file before beginning a model capture.
        // A missing file is ordinary; an existing unreadable/unsafe file blocks.
        if self
            .local_edit_path(task_id)?
            .try_exists()
            .map_err(|_| unavailable())?
        {
            self.reload(task_id, observation)?;
        }
        Ok(())
    }
    pub(crate) fn register(
        &self,
        task_id: &str,
        observation: &ObservationBuffer,
    ) -> Result<(), ApiError> {
        if let Some(snapshot) = self
            .snapshots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(task_id)
        {
            register(snapshot, task_id, observation)?;
        }
        Ok(())
    }
    pub(crate) fn input(
        self: &Arc<Self>,
        task_id: &str,
        name: &str,
        observation: &ObservationBuffer,
        redactor: &ObservationBuffer,
    ) -> Result<SecretInput, ApiError> {
        let snapshots = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = snapshots.get(task_id).ok_or_else(unavailable)?;
        let reference = snapshot.reference(name).map_err(|_| unavailable())?;
        self.store
            .validate_current_reference(task_id, &reference)
            .map_err(|_| unavailable())?;
        let value = snapshot
            .resolve_for_approved_adapter(task_id, &reference)
            .map_err(|_| unavailable())?;
        // No multiline, terminal controls, invisible controls, or shell interpolation.
        crate::execution::validate_command(value).map_err(|_| ApiError::new("TASK_SECRET_INVALID", "Secret input must be one nonempty line without control characters, at most 8192 bytes."))?;
        register(snapshot, task_id, observation)?;
        register(snapshot, task_id, redactor)?;
        let mut bytes = Zeroizing::new(value.as_bytes().to_vec());
        bytes.push(b'\r');
        let snapshot_id = snapshot.status().snapshot_id;
        let epoch = self.epoch.clone();
        let expected_epoch = epoch.load(Ordering::Acquire);
        let current: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || epoch.load(Ordering::Acquire) == expected_epoch);
        let task_id = task_id.to_owned();
        let this = self.clone();
        let still_current = current.clone();
        Ok(SecretInput {
            bytes,
            snapshot_id,
            current,
            validate: Arc::new(move || {
                if !still_current() {
                    return Err(unavailable());
                }
                this.store
                    .validate_current_reference(&task_id, &reference)
                    .map_err(|_| unavailable())?;
                if !still_current() {
                    return Err(unavailable());
                }
                Ok(())
            }),
        })
    }
}
fn register(
    snapshot: &TaskSecretSnapshot,
    task_id: &str,
    observer: &ObservationBuffer,
) -> Result<(), ApiError> {
    for field in snapshot.status().fields.into_iter().filter(|f| f.present) {
        let reference = snapshot.reference(&field.name).map_err(|_| unavailable())?;
        observer.add_known_secret(
            snapshot
                .resolve_for_approved_adapter(task_id, &reference)
                .map_err(|_| unavailable())?,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        execution::{ExecutionGate, Owner},
        ssh::SessionRef,
    };
    use uuid::Uuid;
    struct Fixture {
        root: PathBuf,
        secrets: Arc<TaskSecrets>,
        task: String,
        observation: ObservationBuffer,
    }
    impl Fixture {
        fn new(value: &str) -> Self {
            let root = std::env::temp_dir().join(format!("xyz-secret-adapter-{}", Uuid::new_v4()));
            let secrets = Arc::new(TaskSecrets::open(&root).unwrap());
            let task = Uuid::new_v4().to_string();
            secrets
                .ensure(
                    &task,
                    &[SecretFieldRequest {
                        name: "password".into(),
                        required: true,
                    }],
                    None,
                )
                .unwrap();
            let f = Self {
                root,
                secrets,
                task,
                observation: ObservationBuffer::new(),
            };
            f.edit(value);
            f.secrets.reload(&f.task, &f.observation).unwrap();
            f
        }
        fn edit(&self, value: &str) {
            let quoted = serde_json::to_string(value).unwrap();
            std::fs::write(self.secrets.local_edit_path(&self.task).unwrap(), format!("schema_version = 1\n[task]\nid = \"{}\"\n[secrets.password]\nrequired = true\nvalue = {quoted}\n", self.task)).unwrap();
        }
        fn owner(&self) -> Owner {
            Owner {
                task_id: self.task.clone(),
                run_id: Uuid::new_v4().to_string(),
                session: SessionRef {
                    session_id: "fixture-session".into(),
                    generation: "1".into(),
                },
            }
        }
        fn input(&self) -> SecretInput {
            self.secrets
                .input(
                    &self.task,
                    "password",
                    &self.observation,
                    &self.observation.detached_redactor(),
                )
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn only_safe_metadata_serializes_and_registered_echo_is_redacted() {
        let f = Fixture::new("synthetic-secret-7391");
        let owner = f.owner();
        let gate = ExecutionGate::default();
        gate.acquire(owner.clone(), true).unwrap();
        let action = gate
            .prepare_secret(&owner, "password", "remote test prompt", f.input())
            .unwrap();
        let serialized = serde_json::to_string(&action).unwrap();
        assert!(!serialized.contains("synthetic-secret-7391"));
        assert!(!serde_json::to_string(&f.secrets.status(&f.task))
            .unwrap()
            .contains("synthetic-secret-7391"));
        assert_ne!(
            action.digest,
            format!("{:x}", sha2::Sha256::digest(b"synthetic-secret-7391\r"))
        );
        f.observation.begin(owner.session.clone(), vec![]);
        f.observation.push(&owner.session, b"synthetic-secret-");
        assert!(f.observation.read().text.is_empty());
        f.observation.push(&owner.session, b"7391!");
        assert_eq!(f.observation.read().text, "[REDACTED]!");
    }
    use sha2::Digest;
    #[tokio::test]
    async fn secret_requires_display_approval_and_rejects_queue_edit_reload_cancel_session_and_replay(
    ) {
        for case in ["edit", "reload", "cancel", "session", "replay"] {
            let f = Fixture::new("synthetic-value");
            let owner = f.owner();
            let gate = ExecutionGate::default();
            gate.acquire(owner.clone(), true).unwrap();
            let action = gate
                .prepare_secret(&owner, "password", "fixture purpose", f.input())
                .unwrap();
            assert!(gate.queue(&owner).is_err());
            assert!(gate
                .approve(
                    &owner,
                    &action.action_id,
                    action.approval_id.as_ref().unwrap()
                )
                .is_err());
            gate.acknowledge(&owner, &action.action_id, &action.display_revision)
                .unwrap();
            assert!(gate.queue(&owner).is_err());
            gate.approve(
                &owner,
                &action.action_id,
                action.approval_id.as_ref().unwrap(),
            )
            .unwrap();
            let permit = gate.queue(&owner).unwrap();
            assert!(gate.queue(&owner).is_err());
            let mut session = owner.session.clone();
            match case {
                "edit" => f.edit("replacement-value"),
                "reload" => {
                    f.secrets.reload(&f.task, &f.observation).unwrap();
                }
                "cancel" => {
                    gate.cancel(&owner);
                }
                "session" => session.generation = "2".into(),
                "replay" => {}
                _ => unreachable!(),
            }
            let validation = permit.validate_before_begin().await;
            if case == "edit" || case == "reload" {
                assert!(validation.is_err());
            }
            if case == "replay" {
                permit.begin(&session).unwrap();
            }
            assert!(permit.begin(&session).is_err(), "{case}");
        }
    }
    #[test]
    fn failed_reload_and_ensure_revoke_prior_inputs_without_queue_state_mutation() {
        for operation in ["reload", "ensure"] {
            let f = Fixture::new("synthetic");
            let owner = f.owner();
            let gate = ExecutionGate::default();
            gate.acquire(owner.clone(), true).unwrap();
            let action = gate
                .prepare_secret(&owner, "password", "fixture", f.input())
                .unwrap();
            if operation == "reload" {
                std::fs::write(
                    f.secrets.local_edit_path(&f.task).unwrap(),
                    "invalid config",
                )
                .unwrap();
                assert!(f.secrets.reload(&f.task, &f.observation).is_err());
            } else {
                assert!(f.secrets.ensure(&f.task, &[], Some("stale")).is_err());
            }
            gate.acknowledge(&owner, &action.action_id, &action.display_revision)
                .unwrap();
            gate.approve(
                &owner,
                &action.action_id,
                action.approval_id.as_ref().unwrap(),
            )
            .unwrap();
            assert!(gate.queue(&owner).is_err());
            assert_eq!(gate.action(&owner).unwrap().delivery, "not_sent");
            assert!(!gate.cancel(&owner));
            assert!(gate.acquire(owner, true).is_ok());
        }
    }
    #[test]
    fn multiline_control_and_cross_task_values_never_become_payloads() {
        for value in [
            "line\nbreak",
            "carriage\rreturn",
            "escape\u{1b}value",
            "tab\tvalue",
            "bidi\u{202e}value",
        ] {
            let f = Fixture::new(value);
            assert!(f
                .secrets
                .input(&f.task, "password", &f.observation, &f.observation)
                .is_err());
        }
        let f = Fixture::new("synthetic");
        assert!(f
            .secrets
            .input(
                &Uuid::new_v4().to_string(),
                "password",
                &f.observation,
                &f.observation
            )
            .is_err());
    }
}
