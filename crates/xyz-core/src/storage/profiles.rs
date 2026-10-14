//! Shared TOML SSH schema with a deliberately separate redacted listing DTO.
use super::{
    credentials::{
        validate_descriptor, CredentialMode, CredentialPurpose, CredentialStore, SecretValue,
        StoredCredential,
    },
    private_files::PrivateDirectory,
    valid_id, valid_text, Result, StorageError as E,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;
const MAX_BYTES: usize = 1024 * 1024;
const FILE: &str = "connections.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    Password,
    PrivateKey,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileDraft {
    pub id: String,
    pub label: String,
    pub host: String,
    #[serde(default = "ssh_port")]
    pub port: u16,
    pub username: String,
    pub auth_kind: AuthKind,
    pub private_key_path: Option<PathBuf>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ProfileSummary {
    pub id: String,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_kind: AuthKind,
    pub private_key_path: Option<PathBuf>,
    pub credential_present: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct ProfileListing {
    pub revision: String,
    pub mode: CredentialMode,
    pub connections: Vec<ProfileSummary>,
}
/// Only a dedicated settings/credential UI may construct this operation.
pub enum CredentialChange {
    Keep,
    Clear,
    Set(SecretValue),
}
/// Host-only material. This cannot be serialized into a UI/model response.
pub struct ResolvedProfile {
    pub profile: ProfileDraft,
    pub credential: Option<SecretValue>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    revision: u64,
    storage: Storage,
    connections: Vec<Profile>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Storage {
    secret_protection: CredentialMode,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    id: String,
    label: String,
    host: String,
    #[serde(default = "ssh_port")]
    port: u16,
    username: String,
    auth: Auth,
}
#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Auth {
    kind: Option<AuthKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    private_key_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    protected_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password_secret_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    protected_passphrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    passphrase_secret_ref: Option<String>,
}
fn ssh_port() -> u16 {
    22
}
fn purpose(kind: AuthKind) -> CredentialPurpose {
    match kind {
        AuthKind::Password => CredentialPurpose::SshPassword,
        AuthKind::PrivateKey => CredentialPurpose::SshKeyPassphrase,
    }
}
pub(crate) fn validate_host(host: &str) -> bool {
    if !valid_text(host, 253) || host.ends_with(".invalid") {
        return false;
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}
fn validate_draft(draft: &ProfileDraft) -> Result<()> {
    if !valid_id(&draft.id)
        || !valid_text(&draft.label, 256)
        || !validate_host(&draft.host)
        || draft.port == 0
        || !valid_text(&draft.username, 256)
    {
        return Err(E::InvalidConfig);
    }
    match draft.auth_kind {
        AuthKind::Password if draft.private_key_path.is_some() => return Err(E::InvalidConfig),
        AuthKind::PrivateKey => {
            let path = draft.private_key_path.as_ref().ok_or(E::InvalidConfig)?;
            if !path.is_absolute() || path.as_os_str().len() > 4096 {
                return Err(E::InvalidPath);
            }
            #[cfg(windows)]
            {
                use std::path::{Component, Prefix};
                if !matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
                {
                    return Err(E::InvalidPath);
                }
            }
        }
        _ => {}
    }
    Ok(())
}
impl Profile {
    fn draft(&self) -> Result<ProfileDraft> {
        Ok(ProfileDraft {
            id: self.id.clone(),
            label: self.label.clone(),
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            auth_kind: self.auth.kind.ok_or(E::InvalidConfig)?,
            private_key_path: self.auth.private_key_path.clone(),
        })
    }
    fn descriptor(&self, mode: CredentialMode) -> Result<Option<StoredCredential>> {
        let kind = self.auth.kind.ok_or(E::InvalidConfig)?;
        let a = &self.auth;
        if (kind == AuthKind::Password
            && (a.private_key_path.is_some()
                || a.protected_passphrase.is_some()
                || a.passphrase_secret_ref.is_some()))
            || (kind == AuthKind::PrivateKey
                && (a.protected_password.is_some() || a.password_secret_ref.is_some()))
        {
            return Err(E::InvalidConfig);
        }
        let protected = if kind == AuthKind::Password {
            &a.protected_password
        } else {
            &a.protected_passphrase
        };
        let reference = if kind == AuthKind::Password {
            &a.password_secret_ref
        } else {
            &a.passphrase_secret_ref
        };
        let encoded = match mode {
            CredentialMode::DpapiCurrentUser if reference.is_none() => protected.clone(),
            CredentialMode::LinuxSecretService if protected.is_none() => reference.clone(),
            CredentialMode::SessionOnly if protected.is_none() && reference.is_none() => None,
            _ => return Err(E::InvalidConfig),
        };
        // A saved destination may intentionally have no credential yet. Absence
        // is metadata, not permission to authenticate without a password:
        // resolve_for_authentication still returns CredentialMissing.
        let value = encoded.map(|encoded| StoredCredential { mode, encoded });
        if let Some(value) = &value {
            validate_descriptor(&self.id, purpose(kind), value)?;
        }
        Ok(value)
    }
    fn with_draft(draft: ProfileDraft, descriptor: Option<StoredCredential>) -> Self {
        let mut auth = Auth {
            kind: Some(draft.auth_kind),
            private_key_path: draft.private_key_path,
            ..Default::default()
        };
        if let Some(value) = descriptor {
            match (draft.auth_kind, value.mode) {
                (AuthKind::Password, CredentialMode::DpapiCurrentUser) => {
                    auth.protected_password = Some(value.encoded)
                }
                (AuthKind::PrivateKey, CredentialMode::DpapiCurrentUser) => {
                    auth.protected_passphrase = Some(value.encoded)
                }
                (AuthKind::Password, CredentialMode::LinuxSecretService) => {
                    auth.password_secret_ref = Some(value.encoded)
                }
                (AuthKind::PrivateKey, CredentialMode::LinuxSecretService) => {
                    auth.passphrase_secret_ref = Some(value.encoded)
                }
                _ => {}
            }
        }
        Self {
            id: draft.id,
            label: draft.label,
            host: draft.host,
            port: draft.port,
            username: draft.username,
            auth,
        }
    }
}
fn decode(bytes: &[u8]) -> Result<Document> {
    let text = std::str::from_utf8(bytes).map_err(|_| E::InvalidConfig)?;
    let doc: Document = toml::from_str(text).map_err(|_| E::InvalidConfig)?;
    validate(&doc)?;
    Ok(doc)
}
fn validate(doc: &Document) -> Result<()> {
    if doc.schema_version != 1 {
        return Err(E::UnsupportedSchema);
    }
    doc.storage.secret_protection.validate_platform()?;
    if doc.connections.len() > 256 {
        return Err(E::TooLarge);
    }
    let mut ids = HashSet::new();
    for p in &doc.connections {
        validate_draft(&p.draft()?)?;
        if !ids.insert(&p.id) {
            return Err(E::InvalidConfig);
        }
        p.descriptor(doc.storage.secret_protection)?;
    }
    Ok(())
}
fn encode(doc: &Document) -> Result<Zeroizing<Vec<u8>>> {
    validate(doc)?;
    let text = Zeroizing::new(toml::to_string(doc).map_err(|_| E::InvalidConfig)?);
    Ok(Zeroizing::new(text.as_bytes().to_vec()))
}
pub struct ProfileStore {
    directory: PrivateDirectory,
}
impl ProfileStore {
    pub fn open(root: &Path) -> Result<Self> {
        Ok(Self {
            directory: PrivateDirectory::open_or_create(root)?,
        })
    }
    /// Creates only when absent; choosing SessionOnly must be explicit in UI.
    pub fn initialize(&self, mode: CredentialMode) -> Result<()> {
        mode.validate_platform()?;
        self.directory.update(FILE, MAX_BYTES, |old| {
            if old.is_some() {
                return Err(E::Conflict);
            }
            let document = Document {
                schema_version: 1,
                revision: 0,
                storage: Storage {
                    secret_protection: mode,
                },
                connections: Vec::new(),
            };
            Ok(((), Some(encode(&document)?)))
        })
    }
    /// Read only the validated file-level mode; never probes or downgrades a backend.
    pub fn mode(&self) -> Result<CredentialMode> {
        let bytes = self
            .directory
            .read(FILE, MAX_BYTES)?
            .ok_or(E::Unavailable)?;
        Ok(decode(&bytes)?.storage.secret_protection)
    }
    pub fn list(&self, credentials: &CredentialStore) -> Result<ProfileListing> {
        let bytes = self
            .directory
            .read(FILE, MAX_BYTES)?
            .ok_or(E::Unavailable)?;
        let document = decode(&bytes)?;
        if credentials.mode() != document.storage.secret_protection {
            return Err(E::BackendMismatch);
        }
        let mut connections = Vec::new();
        for p in &document.connections {
            let draft = p.draft()?;
            let present = if credentials.mode() == CredentialMode::SessionOnly {
                credentials.session_present(&p.id, purpose(draft.auth_kind))
            } else {
                p.descriptor(credentials.mode())?.is_some()
            };
            connections.push(ProfileSummary {
                id: draft.id,
                label: draft.label,
                host: draft.host,
                port: draft.port,
                username: draft.username,
                auth_kind: draft.auth_kind,
                private_key_path: draft.private_key_path,
                credential_present: present,
            });
        }
        Ok(ProfileListing {
            revision: document.revision.to_string(),
            mode: credentials.mode(),
            connections,
        })
    }
    pub fn save(
        &self,
        expected_revision: u64,
        draft: ProfileDraft,
        change: CredentialChange,
        credentials: &CredentialStore,
    ) -> Result<u64> {
        validate_draft(&draft)?;
        // Session values have their own explicit API and lifetime. A profile
        // save must not claim that Clear changed in-memory credentials.
        if credentials.mode() == CredentialMode::SessionOnly
            && !matches!(change, CredentialChange::Keep)
        {
            return Err(E::SessionCredentialUpdateRequired);
        }
        self.directory.update(FILE, MAX_BYTES, |old| {
            let mut document = decode(old.ok_or(E::Unavailable)?)?;
            if document.revision != expected_revision {
                return Err(E::Conflict);
            }
            if document.storage.secret_protection != credentials.mode() {
                return Err(E::BackendMismatch);
            }
            let existing = document.connections.iter().position(|p| p.id == draft.id);
            let descriptor = match change {
                CredentialChange::Keep => match existing {
                    Some(index)
                        if document.connections[index].auth.kind == Some(draft.auth_kind) =>
                    {
                        document.connections[index].descriptor(credentials.mode())?
                    }
                    _ => None,
                },
                CredentialChange::Clear => None,
                CredentialChange::Set(value) => {
                    Some(credentials.store(&draft.id, purpose(draft.auth_kind), &value)?)
                }
            };
            let profile = Profile::with_draft(draft, descriptor);
            profile.descriptor(credentials.mode())?;
            if let Some(index) = existing {
                document.connections[index] = profile;
            } else {
                document.connections.push(profile);
            }
            document.revision = document.revision.checked_add(1).ok_or(E::InvalidConfig)?;
            Ok((document.revision, Some(encode(&document)?)))
        })
    }
    /// Settings-only session credential update, serialized with profile edits.
    pub fn set_session_credential(
        &self,
        id: &str,
        expected_revision: u64,
        value: Option<super::credentials::SecretValue>,
        credentials: &CredentialStore,
    ) -> Result<()> {
        self.directory.update(FILE, MAX_BYTES, |old| {
            let doc = decode(old.ok_or(E::Unavailable)?)?;
            if doc.revision != expected_revision {
                return Err(E::Conflict);
            }
            if doc.storage.secret_protection != CredentialMode::SessionOnly
                || credentials.mode() != CredentialMode::SessionOnly
            {
                return Err(E::SessionModeRequired);
            }
            let profile = doc
                .connections
                .iter()
                .find(|p| p.id == id)
                .ok_or(E::InvalidConfig)?;
            let purpose = purpose(profile.auth.kind.ok_or(E::InvalidConfig)?);
            match value {
                Some(value) => credentials.set_session(id, purpose, value)?,
                None => credentials.clear_session(id, purpose)?,
            }
            Ok(((), None))
        })
    }
    /// Explicitly clears disk descriptors, retaining old keyring entries.
    /// Does not silently select this mode in response to a backend error.
    pub fn switch_to_session_only(&self, expected_revision: u64) -> Result<u64> {
        self.directory.update(FILE, MAX_BYTES, |old| {
            let mut doc = decode(old.ok_or(E::Unavailable)?)?;
            if doc.revision != expected_revision {
                return Err(E::Conflict);
            }
            for p in &mut doc.connections {
                *p = Profile::with_draft(p.draft()?, None);
            }
            doc.storage.secret_protection = CredentialMode::SessionOnly;
            doc.revision = doc.revision.checked_add(1).ok_or(E::InvalidConfig)?;
            Ok((doc.revision, Some(encode(&doc)?)))
        })
    }
    /// Authentication-only boundary. Must not be registered as a model tool.
    pub fn resolve_for_authentication(
        &self,
        id: &str,
        expected_revision: u64,
        credentials: &CredentialStore,
    ) -> Result<ResolvedProfile> {
        let bytes = self
            .directory
            .read(FILE, MAX_BYTES)?
            .ok_or(E::Unavailable)?;
        let doc = decode(&bytes)?;
        if doc.revision != expected_revision {
            return Err(E::Conflict);
        }
        if credentials.mode() != doc.storage.secret_protection {
            return Err(E::BackendMismatch);
        }
        let p = doc
            .connections
            .iter()
            .find(|p| p.id == id)
            .ok_or(E::InvalidConfig)?;
        let profile = p.draft()?;
        let descriptor = p.descriptor(credentials.mode())?;
        let credential = if profile.auth_kind == AuthKind::Password
            || descriptor.is_some()
            || credentials.session_present(id, purpose(profile.auth_kind))
        {
            Some(credentials.resolve(id, purpose(profile.auth_kind), descriptor.as_ref())?)
        } else {
            None
        };
        Ok(ResolvedProfile {
            profile,
            credential,
        })
    }
}
