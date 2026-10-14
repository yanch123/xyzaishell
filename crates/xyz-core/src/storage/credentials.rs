//! Native credential adapters. These methods block; call on a host worker.
//! Values/descriptors deliberately do not implement Serialize or Display.
use super::{valid_id, Result, StorageError as E};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Mutex};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMode {
    DpapiCurrentUser,
    LinuxSecretService,
    SessionOnly,
}
impl CredentialMode {
    pub fn validate_platform(self) -> Result<()> {
        if self == Self::SessionOnly
            || (cfg!(windows) && self == Self::DpapiCurrentUser)
            || (cfg!(target_os = "linux") && self == Self::LinuxSecretService)
        {
            Ok(())
        } else {
            Err(E::BackendMismatch)
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialPurpose {
    SshPassword,
    SshKeyPassphrase,
}
impl CredentialPurpose {
    fn name(self) -> &'static str {
        match self {
            Self::SshPassword => "ssh_password",
            Self::SshKeyPassphrase => "ssh_key_passphrase",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialAvailability {
    Available,
    Locked,
    Unavailable,
    AccessDenied,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CredentialStatus {
    pub credential_backend: CredentialMode,
    pub credential_availability: CredentialAvailability,
    pub persistent_credentials: bool,
}
/// Host-only secret bytes. Debug prints no value; dropping zeroizes allocation.
pub struct SecretValue(Zeroizing<Vec<u8>>);
impl SecretValue {
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        let value = Zeroizing::new(bytes);
        if value.is_empty() || value.len() > 16 * 1024 {
            return Err(E::CredentialInvalid);
        }
        Ok(Self(value))
    }
    /// Only an authentication/approved input adapter should call this method.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}
impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretValue([REDACTED])")
    }
}
/// Internal disk descriptor, never a frontend/model response.
#[derive(Clone)]
pub struct StoredCredential {
    pub(crate) mode: CredentialMode,
    pub(crate) encoded: String,
}
impl std::fmt::Debug for StoredCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoredCredential([REDACTED])")
    }
}
pub struct CredentialStore {
    mode: CredentialMode,
    session: Mutex<HashMap<(String, CredentialPurpose), SecretValue>>,
}
impl CredentialStore {
    /// SessionOnly is an explicit caller/UI choice, never a fallback.
    pub fn new(mode: CredentialMode) -> Result<Self> {
        mode.validate_platform()?;
        Ok(Self {
            mode,
            session: Mutex::new(HashMap::new()),
        })
    }
    pub fn mode(&self) -> CredentialMode {
        self.mode
    }
    pub fn status(&self) -> CredentialStatus {
        let availability = match self.mode {
            CredentialMode::SessionOnly => Ok(()),
            #[cfg(windows)]
            CredentialMode::DpapiCurrentUser => Ok(()),
            #[cfg(target_os = "linux")]
            CredentialMode::LinuxSecretService => linux::status(),
            _ => Err(E::CredentialUnavailable),
        };
        CredentialStatus {
            credential_backend: self.mode,
            credential_availability: match availability {
                Ok(()) => CredentialAvailability::Available,
                Err(E::CredentialLocked) => CredentialAvailability::Locked,
                Err(E::CredentialAccessDenied) => CredentialAvailability::AccessDenied,
                _ => CredentialAvailability::Unavailable,
            },
            persistent_credentials: self.mode != CredentialMode::SessionOnly,
        }
    }
    pub fn set_session(
        &self,
        connection_id: &str,
        purpose: CredentialPurpose,
        value: SecretValue,
    ) -> Result<()> {
        if self.mode != CredentialMode::SessionOnly {
            return Err(E::SessionModeRequired);
        }
        if !valid_id(connection_id) {
            return Err(E::InvalidConfig);
        }
        let mut session = self.session.lock().map_err(|_| E::Unavailable)?;
        let key = (connection_id.to_owned(), purpose);
        if session.len() >= 512 && !session.contains_key(&key) {
            return Err(E::TooLarge);
        }
        session.insert(key, value);
        Ok(())
    }
    pub fn clear_session(&self, connection_id: &str, purpose: CredentialPurpose) -> Result<()> {
        self.session
            .lock()
            .map_err(|_| E::Unavailable)?
            .remove(&(connection_id.to_owned(), purpose));
        Ok(())
    }
    pub fn session_present(&self, connection_id: &str, purpose: CredentialPurpose) -> bool {
        self.session
            .lock()
            .is_ok_and(|s| s.contains_key(&(connection_id.to_owned(), purpose)))
    }
    pub fn store(
        &self,
        connection_id: &str,
        purpose: CredentialPurpose,
        value: &SecretValue,
    ) -> Result<StoredCredential> {
        if !valid_id(connection_id) {
            return Err(E::InvalidConfig);
        }
        let encoded = match self.mode {
            #[cfg(windows)]
            CredentialMode::DpapiCurrentUser => {
                windows::protect(connection_id, purpose, value.expose())?
            }
            #[cfg(target_os = "linux")]
            CredentialMode::LinuxSecretService => {
                linux::store(connection_id, purpose, value.expose())?
            }
            CredentialMode::SessionOnly => return Err(E::SessionModeRequired),
            _ => return Err(E::BackendMismatch),
        };
        Ok(StoredCredential {
            mode: self.mode,
            encoded,
        })
    }
    /// Explicit host-authorized removal only. Never call as rollback for a
    /// failed/unknown config commit, since another instance may still use it.
    pub fn remove(
        &self,
        connection_id: &str,
        purpose: CredentialPurpose,
        stored: Option<&StoredCredential>,
    ) -> Result<()> {
        if !valid_id(connection_id) {
            return Err(E::InvalidConfig);
        }
        if self.mode == CredentialMode::SessionOnly {
            if stored.is_some() {
                return Err(E::BackendMismatch);
            }
            return self.clear_session(connection_id, purpose);
        }
        let stored = stored.ok_or(E::CredentialMissing)?;
        if stored.mode != self.mode {
            return Err(E::BackendMismatch);
        }
        validate_descriptor(connection_id, purpose, stored)?;
        match self.mode {
            #[cfg(windows)]
            CredentialMode::DpapiCurrentUser => Ok(()), // Remove ciphertext from profile separately.
            #[cfg(target_os = "linux")]
            CredentialMode::LinuxSecretService => {
                linux::remove(connection_id, purpose, &stored.encoded)
            }
            _ => Err(E::BackendMismatch),
        }
    }
    pub fn resolve(
        &self,
        connection_id: &str,
        purpose: CredentialPurpose,
        stored: Option<&StoredCredential>,
    ) -> Result<SecretValue> {
        if !valid_id(connection_id) {
            return Err(E::InvalidConfig);
        }
        if self.mode == CredentialMode::SessionOnly {
            if stored.is_some() {
                return Err(E::BackendMismatch);
            }
            let session = self.session.lock().map_err(|_| E::Unavailable)?;
            return SecretValue::new(
                session
                    .get(&(connection_id.to_owned(), purpose))
                    .ok_or(E::CredentialMissing)?
                    .expose()
                    .to_vec(),
            );
        }
        let stored = stored.ok_or(E::CredentialMissing)?;
        if stored.mode != self.mode {
            return Err(E::BackendMismatch);
        }
        validate_descriptor(connection_id, purpose, stored)?;
        let bytes = match self.mode {
            #[cfg(windows)]
            CredentialMode::DpapiCurrentUser => {
                windows::unprotect(connection_id, purpose, &stored.encoded)?
            }
            #[cfg(target_os = "linux")]
            CredentialMode::LinuxSecretService => {
                linux::resolve(connection_id, purpose, &stored.encoded)?
            }
            _ => return Err(E::BackendMismatch),
        };
        SecretValue::new(bytes)
    }
}
pub(crate) fn validate_descriptor(
    id: &str,
    purpose: CredentialPurpose,
    stored: &StoredCredential,
) -> Result<()> {
    match stored.mode {
        CredentialMode::LinuxSecretService => {
            reference_token(id, purpose, &stored.encoded)?;
        }
        CredentialMode::DpapiCurrentUser => {
            use base64::Engine;
            let data = stored
                .encoded
                .strip_prefix("dpapi:v1:")
                .ok_or(E::CredentialInvalid)?;
            if data.len() > 64 * 1024 {
                return Err(E::CredentialInvalid);
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| E::CredentialInvalid)?;
            // DPAPI version and provider GUID. Reject fake/base64 plaintext before native unprotect.
            const HEADER: &[u8] = &[
                1, 0, 0, 0, 0xd0, 0x8c, 0x9d, 0xdf, 1, 0x15, 0xd1, 0x11, 0x8c, 0x7a, 0, 0xc0, 0x4f,
                0xc2, 0x97, 0xeb,
            ];
            if !bytes.starts_with(HEADER) || bytes.len() < 64 {
                return Err(E::CredentialInvalid);
            }
        }
        CredentialMode::SessionOnly => return Err(E::CredentialInvalid),
    }
    Ok(())
}
fn reference_token<'a>(
    id: &str,
    purpose: CredentialPurpose,
    reference: &'a str,
) -> Result<&'a str> {
    let prefix = format!("xyzaishell:v1:{id}:{}:", purpose.name());
    let token = reference
        .strip_prefix(&prefix)
        .ok_or(E::CredentialInvalid)?;
    if !valid_id(id) || !valid_id(token) {
        return Err(E::CredentialInvalid);
    }
    Ok(token)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use secret_service::{blocking::SecretService, EncryptionType, Error};
    fn error(error: Error) -> E {
        // Never expose the D-Bus message body (providers may include data).
        match error {
            Error::Locked => E::CredentialLocked,
            Error::Prompt => E::CredentialAccessDenied,
            Error::NoResult => E::CredentialMissing,
            Error::Zbus(zbus::Error::MethodError(name, _, _))
                if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied" =>
            {
                E::CredentialAccessDenied
            }
            Error::ZbusFdo(zbus::fdo::Error::AccessDenied(_)) => E::CredentialAccessDenied,
            _ => E::CredentialUnavailable,
        }
    }
    fn attributes<'a>(id: &'a str, purpose: &'a str, token: &'a str) -> HashMap<&'a str, &'a str> {
        HashMap::from([
            ("application", "xyzaishell"),
            ("schema", "1"),
            ("connection_id", id),
            ("purpose", purpose),
            ("credential_id", token),
        ])
    }
    pub fn status() -> Result<()> {
        let service = SecretService::connect(EncryptionType::Dh).map_err(error)?;
        let collection = service.get_default_collection().map_err(|e| {
            if matches!(e, Error::NoResult) {
                E::CredentialUnavailable
            } else {
                error(e)
            }
        })?;
        collection.ensure_unlocked().map_err(error)
    }
    pub fn store(id: &str, purpose: CredentialPurpose, bytes: &[u8]) -> Result<String> {
        let service = SecretService::connect(EncryptionType::Dh).map_err(error)?;
        let collection = service.get_default_collection().map_err(error)?;
        collection.ensure_unlocked().map_err(error)?;
        let token = uuid::Uuid::new_v4().to_string();
        // Always create a new opaque item. Never replace/delete the old item
        // before the separate TOML commit is confirmed. Failed commits may leave
        // app-owned orphans for explicit future cleanup, never plaintext files.
        let item = collection
            .create_item(
                "xyzaishell SSH credential",
                attributes(id, purpose.name(), &token),
                bytes,
                false,
                "application/octet-stream",
            )
            .map_err(error)?;
        if item.get_attributes().map_err(error)?
            != attributes(id, purpose.name(), &token)
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect()
        {
            return Err(E::CredentialInvalid);
        }
        Ok(format!("xyzaishell:v1:{id}:{}:{token}", purpose.name()))
    }
    pub fn remove(id: &str, purpose: CredentialPurpose, reference: &str) -> Result<()> {
        let token = reference_token(id, purpose, reference)?;
        let service = SecretService::connect(EncryptionType::Dh).map_err(error)?;
        let attrs = attributes(id, purpose.name(), token);
        let found = service.search_items(attrs.clone()).map_err(error)?;
        if !found.locked.is_empty() {
            return Err(E::CredentialLocked);
        }
        if found.unlocked.len() != 1 {
            return Err(E::CredentialMissing);
        }
        let item = &found.unlocked[0];
        let actual = item.get_attributes().map_err(error)?;
        if attrs
            .iter()
            .any(|(k, v)| actual.get(*k).map(String::as_str) != Some(*v))
        {
            return Err(E::CredentialInvalid);
        }
        item.ensure_unlocked().map_err(error)?;
        item.delete().map_err(error)
    }
    pub fn resolve(id: &str, purpose: CredentialPurpose, reference: &str) -> Result<Vec<u8>> {
        let token = reference_token(id, purpose, reference)?;
        let service = SecretService::connect(EncryptionType::Dh).map_err(error)?;
        let attrs = attributes(id, purpose.name(), token);
        let found = service.search_items(attrs.clone()).map_err(error)?;
        if !found.locked.is_empty() {
            return Err(E::CredentialLocked);
        }
        if found.unlocked.is_empty() {
            return Err(E::CredentialMissing);
        }
        if found.unlocked.len() != 1 {
            return Err(E::CredentialInvalid);
        }
        let item = &found.unlocked[0];
        let actual = item.get_attributes().map_err(error)?;
        if attrs
            .iter()
            .any(|(k, v)| actual.get(*k).map(String::as_str) != Some(*v))
        {
            return Err(E::CredentialInvalid);
        }
        item.ensure_unlocked().map_err(error)?;
        item.get_secret().map_err(error)
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use base64::Engine;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    use zeroize::Zeroize;
    fn transform(
        id: &str,
        purpose: CredentialPurpose,
        bytes: &[u8],
        protect: bool,
    ) -> Result<Vec<u8>> {
        let entropy = format!("xyzaishell:credential:v1:{id}:{}", purpose.name());
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len().try_into().map_err(|_| E::TooLarge)?,
            pbData: bytes.as_ptr().cast_mut(),
        };
        let binding = CRYPT_INTEGER_BLOB {
            cbData: entropy.len() as u32,
            pbData: entropy.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: null_mut(),
        };
        // CurrentUser is the default: NEVER set CRYPTPROTECT_LOCAL_MACHINE.
        // Optional entropy cryptographically binds the profile and purpose.
        let ok = unsafe {
            if protect {
                CryptProtectData(
                    &input,
                    null(),
                    &binding,
                    null(),
                    null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    null_mut(),
                    &binding,
                    null(),
                    null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            }
        };
        if ok == 0 {
            return Err(E::CredentialAccessDenied);
        }
        if output.pbData.is_null() {
            return Err(E::CredentialInvalid);
        }
        // SAFETY: DPAPI returns LocalAlloc bytes, valid for cbData until LocalFree.
        let result = unsafe {
            let data = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
            let result = if data.len() <= 64 * 1024 {
                Ok(data.to_vec())
            } else {
                Err(E::TooLarge)
            };
            data.zeroize();
            LocalFree(output.pbData.cast());
            result
        };
        result
    }
    pub fn protect(id: &str, purpose: CredentialPurpose, bytes: &[u8]) -> Result<String> {
        Ok(format!(
            "dpapi:v1:{}",
            base64::engine::general_purpose::STANDARD.encode(transform(id, purpose, bytes, true)?)
        ))
    }
    pub fn unprotect(id: &str, purpose: CredentialPurpose, encoded: &str) -> Result<Vec<u8>> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(
                encoded
                    .strip_prefix("dpapi:v1:")
                    .ok_or(E::CredentialInvalid)?,
            )
            .map_err(|_| E::CredentialInvalid)?;
        transform(id, purpose, &bytes, false)
    }
}
