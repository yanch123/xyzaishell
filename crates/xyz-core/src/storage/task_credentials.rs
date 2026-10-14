//! Per-stable-task plaintext files for user editing. These are private files,
//! not a vault: editors/backups and compromised same-user processes remain risks.
//! Only schema/field presence leaves this module through a serializable DTO.
use super::{private_files::PrivateDirectory, valid_id, Result, StorageError as E};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Mutex,
};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};
const MAX_BYTES: usize = 1024 * 1024;
const MAX_FIELDS: usize = 64;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecretFieldRequest {
    pub name: String,
    #[serde(default = "required")]
    pub required: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct SecretFieldStatus {
    pub name: String,
    pub present: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct TaskCredentialStatus {
    pub task_id: String,
    pub snapshot_id: String,
    pub fields: Vec<SecretFieldStatus>,
}
/// Not serializable. A model cannot construct another task's authority by
/// returning this object's fields; the host creates it from its bound snapshot.
#[derive(Debug, Clone)]
pub struct TaskSecretRef {
    task_id: String,
    field_name: String,
    snapshot_id: Uuid,
}
impl TaskSecretRef {
    pub fn field_name(&self) -> &str {
        &self.field_name
    }
}
pub struct TaskSecretSnapshot {
    task_id: String,
    snapshot_id: Uuid,
    fields: BTreeMap<String, Zeroizing<String>>,
}
impl std::fmt::Debug for TaskSecretSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskSecretSnapshot")
            .field("task_id", &self.task_id)
            .field("snapshot_id", &self.snapshot_id)
            .field("values", &"[REDACTED]")
            .finish()
    }
}
impl TaskSecretSnapshot {
    pub fn status(&self) -> TaskCredentialStatus {
        TaskCredentialStatus {
            task_id: self.task_id.clone(),
            snapshot_id: self.snapshot_id.to_string(),
            fields: self
                .fields
                .iter()
                .map(|(name, value)| SecretFieldStatus {
                    name: name.clone(),
                    present: !value.is_empty(),
                })
                .collect(),
        }
    }
    pub fn reference(&self, field_name: &str) -> Result<TaskSecretRef> {
        if self
            .fields
            .get(field_name)
            .ok_or(E::UnknownField)?
            .is_empty()
        {
            return Err(E::SecretMissing);
        }
        Ok(TaskSecretRef {
            task_id: self.task_id.clone(),
            field_name: field_name.to_owned(),
            snapshot_id: self.snapshot_id,
        })
    }
    /// Host-only adapter boundary; intentionally no shell interpolation method.
    /// The caller must additionally verify task owner/run/approval at dispatch.
    pub fn resolve_for_approved_adapter(
        &self,
        owner_task_id: &str,
        reference: &TaskSecretRef,
    ) -> Result<&str> {
        if self.task_id != owner_task_id || reference.task_id != self.task_id {
            return Err(E::TaskMismatch);
        }
        if reference.snapshot_id != self.snapshot_id {
            return Err(E::SnapshotStale);
        }
        let value = self
            .fields
            .get(&reference.field_name)
            .ok_or(E::UnknownField)?;
        if value.is_empty() {
            return Err(E::SecretMissing);
        }
        Ok(value)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    task: Task,
    #[serde(default)]
    secrets: BTreeMap<String, Field>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Task {
    id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Field {
    #[serde(default = "required")]
    required: bool,
    value: String,
}
impl Drop for Field {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}
fn required() -> bool {
    true
}
fn valid_field(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}
fn filename(task_id: &str) -> Result<String> {
    if !valid_id(task_id) {
        return Err(E::InvalidPath);
    }
    Ok(format!("{task_id}.toml"))
}
fn decode(task_id: &str, data: &[u8]) -> Result<Document> {
    if data.len() > MAX_BYTES {
        return Err(E::TooLarge);
    }
    let document: Document =
        toml::from_str(std::str::from_utf8(data).map_err(|_| E::InvalidConfig)?)
            .map_err(|_| E::InvalidConfig)?;
    if document.schema_version != 1 {
        return Err(E::UnsupportedSchema);
    }
    if document.task.id != task_id || !valid_id(&document.task.id) {
        return Err(E::TaskMismatch);
    }
    if document.secrets.len() > MAX_FIELDS {
        return Err(E::TooLarge);
    }
    for (name, field) in &document.secrets {
        if !valid_field(name) {
            return Err(E::UnknownField);
        }
        if field.value.len() > 16 * 1024 {
            return Err(E::TooLarge);
        }
    }
    Ok(document)
}
fn encode(document: &Document) -> Result<Zeroizing<Vec<u8>>> {
    let text = Zeroizing::new(toml::to_string(document).map_err(|_| E::InvalidConfig)?);
    if text.len() > MAX_BYTES {
        return Err(E::TooLarge);
    }
    Ok(Zeroizing::new(text.as_bytes().to_vec()))
}
struct Version {
    id: Uuid,
    bytes: Zeroizing<Vec<u8>>,
}
/// A bounded host-owned version cache, so UI revisions are random identifiers,
/// never hashes that could act as offline password guessing oracles.
pub struct TaskCredentialStore {
    directory: PrivateDirectory,
    local_path: PathBuf,
    versions: Mutex<HashMap<String, Version>>,
}
impl TaskCredentialStore {
    pub fn open(root: &Path) -> Result<Self> {
        Ok(Self {
            directory: PrivateDirectory::open_or_create(root)?.child("tasks")?,
            local_path: root.join("tasks"),
            versions: Mutex::new(HashMap::new()),
        })
    }
    /// Settings UI only. Never include this path in model tool results.
    pub fn local_edit_path(&self, task_id: &str) -> Result<PathBuf> {
        Ok(self.local_path.join(filename(task_id)?))
    }
    /// The host supplies its stable generated task ID, not a model-selected ID.
    /// Existing files require a previously loaded snapshot, and are only
    /// extended with missing requested names. Existing values never change.
    pub fn ensure_template(
        &self,
        task_id: &str,
        requests: &[SecretFieldRequest],
        expected_snapshot_id: Option<&str>,
    ) -> Result<TaskCredentialStatus> {
        let filename = filename(task_id)?;
        if requests.len() > MAX_FIELDS || requests.iter().any(|r| !valid_field(&r.name)) {
            return Err(E::UnknownField);
        }
        let mut versions = self.versions.lock().map_err(|_| E::Unavailable)?;
        if versions.len() >= 128 && !versions.contains_key(task_id) {
            return Err(E::TooLarge);
        }
        let expected = match expected_snapshot_id {
            Some(id) => {
                let version = versions.get(task_id).ok_or(E::SnapshotStale)?;
                if version.id.to_string() != id {
                    return Err(E::SnapshotStale);
                }
                Some(version.bytes.as_slice())
            }
            None => None,
        };
        let (document, bytes) = self.directory.update(&filename, MAX_BYTES, |old| {
            if old != expected {
                return Err(E::Conflict);
            }
            let mut document = if let Some(bytes) = old {
                decode(task_id, bytes)?
            } else {
                Document {
                    schema_version: 1,
                    task: Task {
                        id: task_id.to_owned(),
                    },
                    secrets: BTreeMap::new(),
                }
            };
            for request in requests {
                document
                    .secrets
                    .entry(request.name.clone())
                    .or_insert_with(|| Field {
                        required: request.required,
                        value: String::new(),
                    });
            }
            if document.secrets.len() > MAX_FIELDS {
                return Err(E::TooLarge);
            }
            let bytes = encode(&document)?;
            let replace = if Some(bytes.as_slice()) == old {
                None
            } else {
                Some(Zeroizing::new(bytes.to_vec()))
            };
            Ok(((document, bytes), replace))
        })?;
        let id = Uuid::new_v4();
        let status = status(&document, id);
        versions.insert(task_id.to_owned(), Version { id, bytes });
        Ok(status)
    }
    /// Explicit reload only. Returns a fresh immutable snapshot with a new ID.
    /// References from an old snapshot cannot resolve against this new one.
    /// The host must atomically replace its active snapshot and invalidate old
    /// prepared actions/approvals; old Arc/snapshot values are not revoked here.
    pub fn load_snapshot(&self, task_id: &str) -> Result<TaskSecretSnapshot> {
        let bytes = self
            .directory
            .read(&filename(task_id)?, MAX_BYTES)?
            .ok_or(E::Unavailable)?;
        let mut doc = decode(task_id, &bytes)?;
        let id = Uuid::new_v4();
        let mut versions = self.versions.lock().map_err(|_| E::Unavailable)?;
        if versions.len() >= 128 && !versions.contains_key(task_id) {
            return Err(E::TooLarge);
        }
        let fields = doc
            .secrets
            .iter_mut()
            .map(|(name, field)| {
                (
                    name.clone(),
                    Zeroizing::new(std::mem::take(&mut field.value)),
                )
            })
            .collect();
        versions.insert(task_id.to_owned(), Version { id, bytes });
        Ok(TaskSecretSnapshot {
            task_id: task_id.to_owned(),
            snapshot_id: id,
            fields,
        })
    }
    /// Final-dispatch guard for the host's serialized owner/approval boundary.
    /// Checks both the latest explicitly loaded revision and the current bounded
    /// file bytes without loading new values. Any edit/reload requires fresh
    /// preparation/approval; callers must not interpret this as authorization.
    /// Noncooperative editors can still race after this call, so the host uses
    /// only the approved immutable snapshot values, never a later disk value.
    pub fn validate_current_snapshot(&self, task_id: &str, snapshot_id: &str) -> Result<()> {
        let filename = filename(task_id)?;
        let versions = self.versions.lock().map_err(|_| E::Unavailable)?;
        let version = versions.get(task_id).ok_or(E::SnapshotStale)?;
        if version.id.to_string() != snapshot_id {
            return Err(E::SnapshotStale);
        }
        let bytes = self
            .directory
            .read(&filename, MAX_BYTES)?
            .ok_or(E::SnapshotStale)?;
        if bytes.as_slice() != version.bytes.as_slice() {
            return Err(E::SnapshotStale);
        }
        Ok(())
    }
    /// The host must additionally check active run, connection generation,
    /// one-time approval, supported adapter and target immediately before use.
    pub fn validate_current_reference(
        &self,
        owner_task_id: &str,
        reference: &TaskSecretRef,
    ) -> Result<()> {
        if reference.task_id != owner_task_id {
            return Err(E::TaskMismatch);
        }
        self.validate_current_snapshot(owner_task_id, &reference.snapshot_id.to_string())
    }
    /// Releases cached source bytes after the host no longer needs editing
    /// conflict detection; does not delete user files or active snapshots.
    pub fn forget_cached_version(&self, task_id: &str) -> Result<()> {
        self.versions
            .lock()
            .map_err(|_| E::Unavailable)?
            .remove(task_id);
        Ok(())
    }
}
fn status(doc: &Document, id: Uuid) -> TaskCredentialStatus {
    TaskCredentialStatus {
        task_id: doc.task.id.clone(),
        snapshot_id: id.to_string(),
        fields: doc
            .secrets
            .iter()
            .map(|(name, field)| SecretFieldStatus {
                name: name.clone(),
                present: !field.value.is_empty(),
            })
            .collect(),
    }
}
