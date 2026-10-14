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
#[cfg(not(windows))]
use std::path::Path;
use std::{
    path::PathBuf,
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
        StorageError::CredentialTargetChanged => "CREDENTIAL_TARGET_CHANGED",
        StorageError::BackendMismatch => "CREDENTIAL_BACKEND_MISMATCH",
        StorageError::SessionModeRequired => "SESSION_MODE_REQUIRED",
        StorageError::SessionCredentialUpdateRequired => "SESSION_CREDENTIAL_UPDATE_REQUIRED",
        StorageError::SnapshotStale => "SECRET_SNAPSHOT_STALE",
        _ => "STORAGE_REJECTED",
    };
    let message = match error {
        StorageError::Conflict => "连接资料已被其他窗口修改。请重新读取并核对后再操作。",
        StorageError::SaveUncertain => "保存结果尚未确定。请重新读取核对，不能自动重试或继续连接。",
        StorageError::CredentialLocked => "系统凭据存储已锁定。请先解锁，或明确选择仅本次输入连接；原凭据引用保留。",
        StorageError::CredentialUnavailable => "系统凭据存储不可用。仍可保存连接资料并临时认证；不会回退为明文。",
        StorageError::CredentialAccessDenied => "系统拒绝访问凭据。请检查权限，或明确选择本次临时输入；原凭据引用保留。",
        StorageError::CredentialMissing => "此连接没有可用的已保存凭据，请输入本次密码或口令。",
        StorageError::CredentialTargetChanged => "认证目标已改变，不能自动复用原凭据。请为新目标输入凭据，或明确移除原引用。",
        StorageError::BackendMismatch => "此凭据由不同的平台保护。请在当前电脑重新输入；原引用会保留。",
        StorageError::UnsupportedSchema => "连接文件版本不受支持。原文件未覆盖，请核对版本和受限备份。",
        _ => "Secure storage could not complete this operation. Reload its status; never retry an uncertain save automatically or move credentials to plaintext storage.",
    };
    ApiError::new(code, message)
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
            profiles.migrate().map_err(storage_error)?;
            let credentials = CredentialStore::new(native_mode()).map_err(storage_error)?;
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
        _expected_revision: &str,
        _mode: CredentialMode,
    ) -> Result<ProfileStatus, ApiError> {
        Err(ApiError::new("CREDENTIAL_GLOBAL_MODE_RETIRED", "Credential choices now apply to individual profiles. Edit the intended profile; other profiles are never downgraded."))
    }
    pub fn delete(&self, id: &str, expected_revision: &str) -> Result<ProfileStatus, ApiError> {
        self.with(|s| {
            s.profiles
                .delete(id, revision(expected_revision)?)
                .map_err(storage_error)?;
            status(s)
        })
    }
    pub fn connection(
        &self,
        id: &str,
        expected_revision: &str,
        size: TerminalSize,
    ) -> Result<ConnectRequest, ApiError> {
        self.connection_with_credential(id, expected_revision, size, None, false)
    }
    /// Temporary input is bound to this exact saved revision and is never
    /// cached for another attempt. Persistent storage is an explicit choice.
    pub fn connection_with_credential(
        &self,
        id: &str,
        expected_revision: &str,
        size: TerminalSize,
        temporary: Option<CredentialText>,
        remember: bool,
    ) -> Result<ConnectRequest, ApiError> {
        self.with(|s| {
            let expected = revision(expected_revision)?;
            let mut applied_revision = expected;
            let resolved = if let Some(mut value) = temporary {
                let profile = s
                    .profiles
                    .draft_for_authentication(id, expected)
                    .map_err(storage_error)?;
                let credential = SecretValue::new(value.take_bytes()).map_err(storage_error)?;
                if profile.auth_kind == AuthKind::PrivateKey {
                    let path = profile
                        .private_key_path
                        .as_ref()
                        .and_then(|p| p.to_str())
                        .ok_or_else(|| storage_error(StorageError::InvalidPath))?;
                    let passphrase = std::str::from_utf8(credential.expose())
                        .map_err(|_| storage_error(StorageError::CredentialInvalid))?;
                    crate::ssh::read_private_key(path, Some(passphrase))?;
                }
                if remember {
                    applied_revision = s
                        .profiles
                        .save(
                            expected,
                            profile.clone(),
                            CredentialChange::Set(
                                SecretValue::new(credential.expose().to_vec())
                                    .map_err(storage_error)?,
                            ),
                            &s.credentials,
                        )
                        .map_err(storage_error)?;
                }
                crate::storage::profiles::ResolvedProfile {
                    profile,
                    credential: Some(credential),
                }
            } else {
                if remember {
                    return Err(storage_error(StorageError::CredentialMissing));
                }
                s.profiles
                    .resolve_for_authentication(id, expected, &s.credentials)
                    .map_err(storage_error)?
            };
            if resolved.profile.auth_kind == AuthKind::PrivateKey {
                let path = resolved
                    .profile
                    .private_key_path
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .ok_or_else(|| storage_error(StorageError::InvalidPath))?;
                let passphrase = resolved
                    .credential
                    .as_ref()
                    .map(|v| std::str::from_utf8(v.expose()))
                    .transpose()
                    .map_err(|_| storage_error(StorageError::CredentialInvalid))?;
                crate::ssh::read_private_key(path, passphrase)?;
            }
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
            // Detect a cross-process edit during slow keyring/key operations.
            s.profiles
                .draft_for_authentication(id, applied_revision)
                .map_err(storage_error)?;
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
    fn temporary_credentials_are_single_attempt_and_never_persist() {
        let (root, host) = fixture();
        let saved = host
            .save(SaveProfileRequest {
                expected_revision: "0".into(),
                profile: draft(),
                credential_update: CredentialUpdate::Keep,
            })
            .unwrap();
        let id = &saved.listing.connections[0].id;
        assert!(uuid::Uuid::parse_str(id).is_ok());
        assert_eq!(
            saved.listing.connections[0].credential_policy,
            crate::storage::profiles::CredentialPolicy::AskOnConnect
        );
        assert!(host
            .connection_with_credential(
                id,
                "1",
                TerminalSize { cols: 80, rows: 24 },
                Some("synthetic-one-attempt".into()),
                false
            )
            .is_ok());
        assert!(!host.status().unwrap().listing.connections[0].credential_present);
        assert_eq!(
            host.connection(id, "1", TerminalSize { cols: 80, rows: 24 })
                .err()
                .unwrap()
                .code,
            "CREDENTIAL_MISSING"
        );
        let disk = std::fs::read_to_string(root.join("connections.toml")).unwrap();
        assert!(!disk.contains("synthetic-one-attempt"));
        let serialized = serde_json::to_string(&host.status().unwrap()).unwrap();
        assert!(!serialized.contains("synthetic-one-attempt"));
        let reopened = HostStorage::from_root(Ok(root.clone()));
        assert_eq!(reopened.status().unwrap().listing.connections[0].id, *id);
        assert_eq!(
            host.set_mode("1", CredentialMode::SessionOnly)
                .err()
                .unwrap()
                .code,
            "CREDENTIAL_GLOBAL_MODE_RETIRED"
        );
        drop((host, reopened));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rejected_revision_does_not_store_temporary_or_persistent_input() {
        let (root, host) = fixture();
        assert_eq!(
            host.save(SaveProfileRequest {
                expected_revision: "99".into(),
                profile: draft(),
                credential_update: CredentialUpdate::Set {
                    value: "synthetic-rejected-secret".into()
                }
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
    fn temporary_credential_rejects_cross_instance_target_change() {
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
            host.connection_with_credential(
                &id,
                "1",
                TerminalSize { cols: 80, rows: 24 },
                Some("synthetic-not-for-new-target".into()),
                false
            )
            .err()
            .unwrap()
            .code,
            "CONFIG_CONFLICT"
        );
        assert!(!host.status().unwrap().listing.connections[0].credential_present);
        assert!(host
            .connection_with_credential(
                &id,
                "2",
                TerminalSize { cols: 80, rows: 24 },
                Some("synthetic-current-target".into()),
                false
            )
            .is_ok());
        drop((host, other));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn delete_is_revision_checked_and_does_not_remove_other_profiles() {
        let (root, host) = fixture();
        let first = host
            .save(SaveProfileRequest {
                expected_revision: "0".into(),
                profile: draft(),
                credential_update: CredentialUpdate::Keep,
            })
            .unwrap();
        let id = first.listing.connections[0].id.clone();
        host.save(SaveProfileRequest {
            expected_revision: "1".into(),
            profile: draft(),
            credential_update: CredentialUpdate::Keep,
        })
        .unwrap();
        assert_eq!(host.delete(&id, "1").err().unwrap().code, "CONFIG_CONFLICT");
        let remaining = host.delete(&id, "2").unwrap();
        assert_eq!(remaining.listing.connections.len(), 1);
        assert_ne!(remaining.listing.connections[0].id, id);
        drop(host);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn encrypted_key_prompts_only_when_needed_and_temporary_passphrase_is_not_saved() {
        let (root, host) = fixture();
        let directory =
            crate::storage::private_files::PrivateDirectory::open_or_create(&root).unwrap();
        let key =
            russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                .unwrap();
        let clear = key
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        directory
            .update("fixture-key", 1024 * 1024, |_| {
                Ok(((), Some(Zeroizing::new(clear.as_bytes().to_vec()))))
            })
            .unwrap();
        let mut input = draft();
        input.auth_kind = AuthKind::PrivateKey;
        input.private_key_path = Some(root.join("fixture-key"));
        let saved = host
            .save(SaveProfileRequest {
                expected_revision: "0".into(),
                profile: input,
                credential_update: CredentialUpdate::Keep,
            })
            .unwrap();
        let id = &saved.listing.connections[0].id;
        assert!(host
            .connection(id, "1", TerminalSize { cols: 80, rows: 24 })
            .is_ok());
        let encrypted = key
            .encrypt(&mut rand::rng(), "synthetic-key-passphrase")
            .unwrap()
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        directory
            .update("fixture-key", 1024 * 1024, |_| {
                Ok(((), Some(Zeroizing::new(encrypted.as_bytes().to_vec()))))
            })
            .unwrap();
        assert_eq!(
            host.connection(id, "1", TerminalSize { cols: 80, rows: 24 })
                .err()
                .unwrap()
                .code,
            "CREDENTIAL_PASSPHRASE_REQUIRED"
        );
        assert!(host
            .connection_with_credential(
                id,
                "1",
                TerminalSize { cols: 80, rows: 24 },
                Some("synthetic-key-passphrase".into()),
                false
            )
            .is_ok());
        assert!(!host.status().unwrap().listing.connections[0].credential_present);
        assert!(!std::fs::read_to_string(root.join("connections.toml"))
            .unwrap()
            .contains("synthetic-key-passphrase"));
        drop((host, directory));
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
