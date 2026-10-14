//! Dedicated settings/authentication boundary. Never register as model tools.
use crate::{
    ssh::{ApiError, Authentication, ConnectRequest, Secret, TerminalSize},
    storage::{
        credentials::{CredentialMode, CredentialStatus, CredentialStore, SecretValue},
        profiles::{AuthKind, CredentialChange, ProfileDraft, ProfileListing, ProfileStore},
        StorageError,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use zeroize::{Zeroize, Zeroizing};

pub fn storage_error(error: StorageError) -> ApiError {
    let code = match error {
        StorageError::Conflict => "CONFIG_CONFLICT",
        StorageError::SaveUncertain => "CONFIG_SAVE_UNCERTAIN",
        StorageError::CredentialLocked => "CREDENTIAL_LOCKED",
        StorageError::CredentialAccessDenied => "CREDENTIAL_ACCESS_DENIED",
        StorageError::CredentialUnavailable => "CREDENTIAL_UNAVAILABLE",
        StorageError::CredentialMissing => "CREDENTIAL_MISSING",
        StorageError::BackendMismatch => "CREDENTIAL_BACKEND_MISMATCH",
        StorageError::SessionModeRequired => "SESSION_MODE_REQUIRED",
        StorageError::SessionCredentialUpdateRequired => "SESSION_CREDENTIAL_UPDATE_REQUIRED",
        StorageError::SnapshotStale => "SECRET_SNAPSHOT_STALE",
        _ => "STORAGE_REJECTED",
    };
    ApiError::new(code, "Secure storage could not complete this operation. Reload its status; never retry an uncertain save automatically or move credentials to plaintext storage.")
}
pub fn resolve_storage_root() -> Result<PathBuf, ApiError> {
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
    root.filter(|p| p.is_absolute())
        .map(|p| p.join("xyzaishell"))
        .ok_or_else(|| {
            ApiError::new(
                "CONFIG_PATH_INVALID",
                "An absolute platform configuration directory is required.",
            )
        })
}
fn path_warning() -> Option<&'static str> {
    #[cfg(not(windows))]
    if std::env::var_os("XDG_CONFIG_HOME")
        .is_some_and(|p| !p.is_empty() && !Path::new(&p).is_absolute())
    {
        return Some(
            "Relative XDG_CONFIG_HOME was ignored; using the default HOME configuration directory.",
        );
    }
    None
}
fn native_mode() -> CredentialMode {
    if cfg!(windows) {
        CredentialMode::DpapiCurrentUser
    } else {
        CredentialMode::LinuxSecretService
    }
}
#[derive(Serialize)]
pub struct ProfileStatus {
    pub platform: &'static str,
    pub configuration_root: String,
    pub path_warning: Option<&'static str>,
    pub credentials: CredentialStatus,
    pub listing: ProfileListing,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    pub id: Option<String>,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_kind: AuthKind,
    pub private_key_path: Option<PathBuf>,
}
/// Authentication-input-only DTO. Early validation errors zeroize it too.
#[derive(Deserialize)]
#[serde(transparent)]
pub struct CredentialText(String);
impl Drop for CredentialText {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
impl From<&str> for CredentialText {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}
impl From<String> for CredentialText {
    fn from(value: String) -> Self {
        Self(value)
    }
}
impl CredentialText {
    fn take_bytes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0).into_bytes()
    }
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialUpdate {
    Keep,
    Clear,
    Set { value: CredentialText },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveProfileRequest {
    pub expected_revision: String,
    pub profile: ProfileInput,
    pub credential_update: CredentialUpdate,
}
struct Storage {
    root: PathBuf,
    profiles: ProfileStore,
    credentials: CredentialStore,
}
#[derive(Clone)]
pub struct HostStorage(Arc<Mutex<Result<Storage, ApiError>>>);
impl Default for HostStorage {
    fn default() -> Self {
        Self::new()
    }
}
impl HostStorage {
    pub fn new() -> Self {
        Self::from_root(resolve_storage_root())
    }
    pub fn from_root(root: Result<PathBuf, ApiError>) -> Self {
        let result = root.and_then(|root| {
            let profiles = ProfileStore::open(&root).map_err(storage_error)?;
            match profiles.initialize(native_mode()) {
                Ok(()) | Err(StorageError::Conflict) => {}
                Err(error) => return Err(storage_error(error)),
            }
            let mode = profiles.mode().map_err(storage_error)?;
            let credentials = CredentialStore::new(mode).map_err(storage_error)?;
            Ok(Storage {
                root,
                profiles,
                credentials,
            })
        });
        Self(Arc::new(Mutex::new(result)))
    }
    fn with<T>(&self, f: impl FnOnce(&mut Storage) -> Result<T, ApiError>) -> Result<T, ApiError> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| storage_error(StorageError::Unavailable))?;
        match guard.as_mut() {
            Ok(storage) => f(storage),
            Err(error) => Err(error.clone()),
        }
    }
    pub fn configuration_root(&self) -> Result<PathBuf, ApiError> {
        self.with(|s| Ok(s.root.clone()))
    }
    pub fn status(&self) -> Result<ProfileStatus, ApiError> {
        self.with(status)
    }
    pub fn save(&self, request: SaveProfileRequest) -> Result<ProfileStatus, ApiError> {
        self.with(|s| {
            let p = request.profile;
            let draft = ProfileDraft {
                id: p.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                label: p.label,
                host: p.host,
                port: p.port,
                username: p.username,
                auth_kind: p.auth_kind,
                private_key_path: p.private_key_path,
            };
            let change = match request.credential_update {
                CredentialUpdate::Keep => CredentialChange::Keep,
                CredentialUpdate::Clear => CredentialChange::Clear,
                CredentialUpdate::Set { mut value } => CredentialChange::Set(
                    SecretValue::new(value.take_bytes()).map_err(storage_error)?,
                ),
            };
            s.profiles
                .save(
                    revision(&request.expected_revision)?,
                    draft,
                    change,
                    &s.credentials,
                )
                .map_err(storage_error)?;
            status(s)
        })
    }
    pub fn set_session(
        &self,
        id: &str,
        expected_revision: &str,
        value: Option<CredentialText>,
    ) -> Result<ProfileStatus, ApiError> {
        let value = value.map(|mut v| Zeroizing::new(v.take_bytes()));
        self.with(|s| {
            let secret = value
                .map(|v| SecretValue::new(v.to_vec()))
                .transpose()
                .map_err(storage_error)?;
            s.profiles
                .set_session_credential(id, revision(expected_revision)?, secret, &s.credentials)
                .map_err(storage_error)?;
            status(s)
        })
    }
    pub fn set_mode(
        &self,
        expected_revision: &str,
        mode: CredentialMode,
    ) -> Result<ProfileStatus, ApiError> {
        self.with(|s| {
            if mode != CredentialMode::SessionOnly { return Err(ApiError::new("CREDENTIAL_MODE_MIGRATION_UNSUPPORTED", "Only explicit migration to session-only credentials is supported. Re-enter credentials after switching.")); }
            s.profiles.switch_to_session_only(revision(expected_revision)?).map_err(storage_error)?;
            s.credentials = CredentialStore::new(mode).map_err(storage_error)?;
            status(s)
        })
    }
    pub fn connection(
        &self,
        id: &str,
        expected_revision: &str,
        size: TerminalSize,
    ) -> Result<ConnectRequest, ApiError> {
        self.with(|s| {
            let resolved = s
                .profiles
                .resolve_for_authentication(id, revision(expected_revision)?, &s.credentials)
                .map_err(storage_error)?;
            let secret = resolved
                .credential
                .map(|value| {
                    std::str::from_utf8(value.expose())
                        .map(|v| Secret::new(v.to_owned()))
                        .map_err(|_| storage_error(StorageError::CredentialInvalid))
                })
                .transpose()?;
            let p = resolved.profile;
            let auth = match p.auth_kind {
                AuthKind::Password => Authentication::Password {
                    password: secret
                        .ok_or_else(|| storage_error(StorageError::CredentialMissing))?,
                },
                AuthKind::PrivateKey => Authentication::PrivateKey {
                    private_key_path: p
                        .private_key_path
                        .ok_or_else(|| storage_error(StorageError::InvalidPath))?
                        .to_str()
                        .ok_or_else(|| storage_error(StorageError::InvalidPath))?
                        .to_owned(),
                    passphrase: secret,
                },
            };
            // Resolved copies live only for this connection authentication. The
            // dedicated saved-profile path, not IPC, constructs this request.
            Ok(ConnectRequest {
                host: p.host,
                port: p.port,
                username: p.username,
                auth,
                session_only: true,
                size,
            })
        })
    }
}
fn status(s: &mut Storage) -> Result<ProfileStatus, ApiError> {
    Ok(ProfileStatus {
        platform: if cfg!(windows) { "windows" } else { "linux" },
        configuration_root: s.root.display().to_string(),
        path_warning: path_warning(),
        credentials: s.credentials.status(),
        listing: s.profiles.list(&s.credentials).map_err(storage_error)?,
    })
}
fn revision(value: &str) -> Result<u64, ApiError> {
    if value.is_empty() || !value.bytes().all(|v| v.is_ascii_digit()) {
        return Err(storage_error(StorageError::InvalidConfig));
    }
    value
        .parse()
        .map_err(|_| storage_error(StorageError::InvalidConfig))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, HostStorage) {
        let root = std::env::temp_dir().join(format!("xyz-host-storage-{}", uuid::Uuid::new_v4()));
        let store = ProfileStore::open(&root).unwrap();
        store.initialize(CredentialMode::SessionOnly).unwrap();
        let host = HostStorage::from_root(Ok(root.clone()));
        (root, host)
    }
    fn draft() -> ProfileInput {
        ProfileInput {
            id: None,
            label: "Synthetic".into(),
            host: "localhost".into(),
            port: 22,
            username: "test".into(),
            auth_kind: AuthKind::Password,
            private_key_path: None,
        }
    }
    #[test]
    fn explicit_session_profile_is_persisted_without_credential_and_restart_forgets_secret() {
        let (root, host) = fixture();
        let listing = host
            .save(SaveProfileRequest {
                expected_revision: "0".into(),
                profile: draft(),
                credential_update: CredentialUpdate::Keep,
            })
            .unwrap()
            .listing;
        let id = &listing.connections[0].id;
        assert!(uuid::Uuid::parse_str(id).is_ok());
        host.set_session(id, "1", Some("synthetic-private-password".into()))
            .unwrap();
        let status = host.status().unwrap();
        assert!(status.listing.connections[0].credential_present);
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains("synthetic-private-password"));
        let disk = std::fs::read_to_string(root.join("connections.toml")).unwrap();
        assert!(!disk.contains("synthetic-private-password"));
        assert!(host
            .connection(id, "1", TerminalSize { cols: 80, rows: 24 })
            .is_ok());
        assert_eq!(
            host.connection(id, "0", TerminalSize { cols: 80, rows: 24 })
                .err()
                .unwrap()
                .code,
            "CONFIG_CONFLICT"
        );
        let reopened = HostStorage::from_root(Ok(root.clone()));
        assert!(!reopened.status().unwrap().listing.connections[0].credential_present);
        assert_eq!(
            reopened
                .connection(id, "1", TerminalSize { cols: 80, rows: 24 })
                .err()
                .unwrap()
                .code,
            "CREDENTIAL_MISSING"
        );
        drop((host, reopened));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn session_profile_save_does_not_silently_store_secret_or_accept_revision_conflict() {
        let (root, host) = fixture();
        assert_eq!(
            host.save(SaveProfileRequest {
                expected_revision: "0".into(),
                profile: draft(),
                credential_update: CredentialUpdate::Set {
                    value: "synthetic".into()
                }
            })
            .err()
            .unwrap()
            .code,
            "SESSION_CREDENTIAL_UPDATE_REQUIRED"
        );
        assert_eq!(
            host.save(SaveProfileRequest {
                expected_revision: "99".into(),
                profile: draft(),
                credential_update: CredentialUpdate::Keep
            })
            .err()
            .unwrap()
            .code,
            "CONFIG_CONFLICT"
        );
        assert!(host.status().unwrap().listing.connections.is_empty());
        drop(host);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_metadata_only_password_profile_survives_restart_but_cannot_authenticate() {
        let root = std::env::temp_dir().join(format!("xyz-host-metadata-{}", uuid::Uuid::new_v4()));
        let host = HostStorage::from_root(Ok(root.clone()));
        let saved = host
            .save(SaveProfileRequest {
                expected_revision: "0".into(),
                profile: draft(),
                credential_update: CredentialUpdate::Keep,
            })
            .unwrap();
        let id = saved.listing.connections[0].id.clone();
        assert!(!saved.listing.connections[0].credential_present);
        let reopened = HostStorage::from_root(Ok(root.clone()));
        let restored = reopened.status().unwrap();
        assert_eq!(restored.listing.connections[0].id, id);
        assert!(!restored.listing.connections[0].credential_present);
        assert_eq!(
            reopened
                .connection(
                    &id,
                    &restored.listing.revision,
                    TerminalSize { cols: 80, rows: 24 }
                )
                .err()
                .unwrap()
                .code,
            "CREDENTIAL_MISSING"
        );
        let disk = std::fs::read_to_string(root.join("connections.toml")).unwrap();
        assert!(!disk.contains("protected_password"));
        assert!(!disk.contains("password_secret_ref"));
        drop((host, reopened));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn session_credential_update_rejects_cross_instance_profile_revision_change() {
        let (root, host) = fixture();
        let first = host
            .save(SaveProfileRequest {
                expected_revision: "0".into(),
                profile: draft(),
                credential_update: CredentialUpdate::Keep,
            })
            .unwrap();
        let id = first.listing.connections[0].id.clone();
        let other = HostStorage::from_root(Ok(root.clone()));
        let mut changed = draft();
        changed.id = Some(id.clone());
        changed.host = "127.0.0.2".into();
        other
            .save(SaveProfileRequest {
                expected_revision: "1".into(),
                profile: changed,
                credential_update: CredentialUpdate::Keep,
            })
            .unwrap();
        assert_eq!(
            host.set_session(&id, "1", Some("synthetic-not-for-changed-target".into()))
                .err()
                .unwrap()
                .code,
            "CONFIG_CONFLICT"
        );
        assert!(!host.status().unwrap().listing.connections[0].credential_present);
        host.set_session(&id, "2", Some("synthetic-explicit-current-target".into()))
            .unwrap();
        assert_eq!(
            host.set_session(&id, "1", None).err().unwrap().code,
            "CONFIG_CONFLICT"
        );
        assert!(host.status().unwrap().listing.connections[0].credential_present);
        drop((host, other));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_root_is_retained_as_failure_without_fallback() {
        let host = HostStorage::from_root(Err(ApiError::new(
            "PATH_FAILED",
            "Invalid configuration directory.",
        )));
        assert_eq!(host.status().err().unwrap().code, "PATH_FAILED");
        assert_eq!(host.configuration_root().unwrap_err().code, "PATH_FAILED");
    }
}
