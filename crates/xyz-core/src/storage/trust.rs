//! Application-owned persisted host trust, exact hostname + port identity.
//! It is a private TOML trust database named `known_hosts`, not OpenSSH syntax.
use super::{private_files::PrivateDirectory, profiles::validate_host, Result, StorageError as E};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;
const MAX_BYTES: usize = 1024 * 1024;
const FILE: &str = "known_hosts";
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HostTrustCheck {
    Trusted,
    ApprovalRequired {
        challenge_id: String,
        host: String,
        port: u16,
        fingerprint: String,
        previous_fingerprint: Option<String>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    revision: u64,
    hosts: Vec<Entry>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    host: String,
    port: u16,
    fingerprint: String,
}
struct Challenge {
    revision: u64,
    entry: Entry,
    previous: Option<String>,
    expires: Instant,
}
pub struct KnownHostsStore {
    directory: PrivateDirectory,
    pending: Mutex<HashMap<String, Challenge>>,
}
fn fingerprint_valid(value: &str) -> bool {
    use base64::Engine;
    let Some(digest) = value.strip_prefix("SHA256:") else {
        return false;
    };
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(digest)
        .is_ok_and(|b| b.len() == 32)
}
fn decode(bytes: Option<&[u8]>) -> Result<Document> {
    let Some(bytes) = bytes else {
        return Ok(Document {
            schema_version: 1,
            revision: 0,
            hosts: Vec::new(),
        });
    };
    let doc: Document = toml::from_str(std::str::from_utf8(bytes).map_err(|_| E::InvalidConfig)?)
        .map_err(|_| E::InvalidConfig)?;
    if doc.schema_version != 1 {
        return Err(E::UnsupportedSchema);
    }
    if doc.hosts.len() > 4096 {
        return Err(E::TooLarge);
    }
    let mut seen = HashSet::new();
    for entry in &doc.hosts {
        if !validate_host(&entry.host)
            || entry.port == 0
            || !fingerprint_valid(&entry.fingerprint)
            || !seen.insert((&entry.host, entry.port))
        {
            return Err(E::InvalidConfig);
        }
    }
    Ok(doc)
}
impl KnownHostsStore {
    pub fn open(root: &Path) -> Result<Self> {
        Ok(Self {
            directory: PrivateDirectory::open_or_create(root)?,
            pending: Mutex::new(HashMap::new()),
        })
    }
    /// Unknown and changed fingerprints always produce a specific challenge.
    pub fn check(&self, host: &str, port: u16, fingerprint: &str) -> Result<HostTrustCheck> {
        if !validate_host(host) || port == 0 || !fingerprint_valid(fingerprint) {
            return Err(E::InvalidConfig);
        }
        let bytes = self.directory.read(FILE, MAX_BYTES)?;
        let doc = decode(bytes.as_deref().map(Vec::as_slice))?;
        let previous = doc
            .hosts
            .iter()
            .find(|e| e.host == host && e.port == port)
            .map(|e| e.fingerprint.clone());
        if previous.as_deref() == Some(fingerprint) {
            return Ok(HostTrustCheck::Trusted);
        }
        let mut pending = self.pending.lock().map_err(|_| E::Unavailable)?;
        pending.retain(|_, c| c.expires > Instant::now());
        if pending.len() >= 128 {
            return Err(E::Busy);
        }
        let challenge_id = Uuid::new_v4().to_string();
        pending.insert(
            challenge_id.clone(),
            Challenge {
                revision: doc.revision,
                entry: Entry {
                    host: host.to_owned(),
                    port,
                    fingerprint: fingerprint.to_owned(),
                },
                previous: previous.clone(),
                expires: Instant::now() + Duration::from_secs(300),
            },
        );
        Ok(HostTrustCheck::ApprovalRequired {
            challenge_id,
            host: host.to_owned(),
            port,
            fingerprint: fingerprint.to_owned(),
            previous_fingerprint: previous,
        })
    }
    /// Dedicated, actual user trust confirmation only. Registering this as a
    /// model tool or accepting a model supplied approval boolean is forbidden.
    /// A challenge is single use even if disk commit is unknown/fails.
    pub fn accept_from_trust_ui(
        &self,
        challenge_id: &str,
        host: &str,
        port: u16,
        expected_fingerprint: &str,
    ) -> Result<()> {
        let challenge = self
            .pending
            .lock()
            .map_err(|_| E::Unavailable)?
            .remove(challenge_id)
            .ok_or(E::HostTrustChallengeStale)?;
        if challenge.expires <= Instant::now()
            || challenge.entry.host != host
            || challenge.entry.port != port
            || challenge.entry.fingerprint != expected_fingerprint
        {
            return Err(E::HostTrustChallengeStale);
        }
        self.directory.update(FILE, MAX_BYTES, |old| {
            let mut doc = decode(old)?;
            if doc.revision != challenge.revision {
                return Err(E::Conflict);
            }
            let existing = doc
                .hosts
                .iter()
                .position(|e| e.host == host && e.port == port);
            if existing.map(|i| doc.hosts[i].fingerprint.as_str()) != challenge.previous.as_deref()
            {
                return Err(E::Conflict);
            }
            if let Some(index) = existing {
                doc.hosts[index] = challenge.entry;
            } else {
                doc.hosts.push(challenge.entry);
            }
            if doc.hosts.len() > 4096 {
                return Err(E::TooLarge);
            }
            doc.revision = doc.revision.checked_add(1).ok_or(E::InvalidConfig)?;
            let bytes = toml::to_string(&doc)
                .map_err(|_| E::InvalidConfig)?
                .into_bytes();
            Ok(((), Some(zeroize::Zeroizing::new(bytes))))
        })
    }
    pub fn reject_from_trust_ui(&self, challenge_id: &str) -> Result<()> {
        self.pending
            .lock()
            .map_err(|_| E::Unavailable)?
            .remove(challenge_id)
            .ok_or(E::HostTrustChallengeStale)?;
        Ok(())
    }
}
