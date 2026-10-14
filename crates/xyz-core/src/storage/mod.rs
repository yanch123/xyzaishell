//! Host-only configuration and credential persistence. Public listing DTOs never
//! contain a secret value, a reusable ciphertext, or a keyring reference.
//! Blocking filesystem/keyring operations belong on a host blocking worker.
pub mod credentials;
pub(crate) mod private_files;
pub mod profiles;
pub mod task_credentials;
pub mod trust;

use serde::Serialize;

pub type Result<T> = std::result::Result<T, StorageError>;

/// Fixed, redacted errors: never construct these from raw OS or parser text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageError {
    InvalidPath,
    UnsafeFile,
    UnsupportedFilesystem,
    Unavailable,
    TooLarge,
    InvalidConfig,
    UnsupportedSchema,
    Conflict,
    Busy,
    SaveUncertain,
    BackendMismatch,
    CredentialUnavailable,
    CredentialLocked,
    CredentialAccessDenied,
    CredentialMissing,
    CredentialInvalid,
    CredentialTargetChanged,
    SessionModeRequired,
    SessionCredentialUpdateRequired,
    TaskMismatch,
    SnapshotStale,
    UnknownField,
    SecretMissing,
    HostTrustApprovalRequired,
    HostTrustChallengeStale,
}
impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for StorageError {}

pub(crate) fn valid_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
pub(crate) fn valid_text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests;
