//! Optional, local, plaintext history. Nothing in this module connects to SSH,
//! starts a model, restores a run, or creates/consumes an approval.
//!
//! The host must obtain an explicit save-history preference before `open` and
//! revoke this store's preference immediately when that choice changes. A
//! `RedactedChat` can only be built through a mandatory host redaction callback:
//! redact every known model/SSH/task secret BEFORE SQLite sees any text. An
//! identity callback is not a sanitizer. Unknown or transformed secrets remain
//! a risk; SQLite and owner-private files are NOT encryption.
//!
//! Blocking API: call on a host blocking worker. DELETE rollback journals avoid
//! persistent WAL copies; temporary data stays in memory. PrivateDirectory
//! checks ownership, local filesystems, links and permissions. SQLite still uses
//! a pathname inside that private directory. Administrators, malicious same-user
//! processes, filesystem drivers, backups and storage-device remanence are not
//! protected. `forget` removes logical records, not every historical disk copy.
//!
//! Integration: register this module and the storage module in the host core,
//! add rusqlite 0.37 with `bundled` and `limits`, and run these blocking calls off
//! the UI thread. The host owns a separate, explicit preference (absence means
//! MemoryOnly); database presence is never consent. Track `StoredChat.revision`
//! separately from live TaskManager revisions. Restore only `to_snapshot()` into
//! fresh inert task records, with empty runtime ownership/approval maps. Surface
//! Conflict, Corrupt, UnsupportedSchema and LimitReached without trimming/reset;
//! re-read after SaveUncertain rather than automatically retrying a write.
//! Do not broadcast archives or audit records as executable commands/events.
use crate::{
    ssh::ApiError,
    storage::{private_files::PrivateDirectory, StorageError},
    tasks::{
        ChatMessage, ChatSnapshot, ChatSource, MessageRole, MessageState, MAX_ASSISTANT_BYTES,
        MAX_CHATS, MAX_MESSAGE_BYTES,
    },
};
use rusqlite::{
    config::DbConfig, limits::Limit, params, Connection, ErrorCode, OpenFlags, OptionalExtension,
    TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const DATABASE_NAME: &str = "history.sqlite3";
pub const MAX_DATABASE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_AUDIT_EVENTS: usize = 4096;
const MAX_JOURNAL_BYTES: usize = MAX_DATABASE_BYTES + 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;
const PAGE_SIZE: usize = 4096;
const APPLICATION_ID: i64 = 0x58595a48;
// v3 separates durable message entries from display pages and model memory.
// v1/v2 readers must refuse this archive; legacy payloads migrate atomically.
const SCHEMA_VERSION: i64 = 3;
pub const ARCHIVE_PAGE_MESSAGES: usize = 128;
pub const ARCHIVE_PAGE_BYTES: usize = 256 * 1024;
const MAX_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
// Keep the original startup lock name so initialization and every subsequent
// operation participate in one protocol. Never replace/delete this lock file.
const ACCESS_LOCK: &str = "history.initialize";
const CHAT_SCHEMA: &str = "CREATE TABLE chats (chat_id TEXT PRIMARY KEY NOT NULL, task_id TEXT UNIQUE NOT NULL, revision INTEGER NOT NULL CHECK(revision > 0), listed INTEGER NOT NULL CHECK(listed IN (0,1)), updated_ms INTEGER NOT NULL CHECK(updated_ms >= 0), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB)) <= 2097152)) STRICT";
const METADATA_SCHEMA: &str = "CREATE TABLE metadata (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL CHECK(revision >= 0)) STRICT";
const AUDIT_SCHEMA: &str = "CREATE TABLE audit (event_id TEXT PRIMARY KEY NOT NULL, chat_id TEXT NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE, run_id TEXT, kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND 8), created_ms INTEGER NOT NULL CHECK(created_ms >= 0)) STRICT";

const ENTRY_SCHEMA: &str = "CREATE TABLE entries (chat_id TEXT NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE, sequence INTEGER NOT NULL CHECK(sequence > 0), message_id TEXT NOT NULL, payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB)) <= 524288), PRIMARY KEY(chat_id,sequence), UNIQUE(chat_id,message_id)) STRICT";
// Migration backups are retained until explicit forget; they are never replayed.
const BACKUP_SCHEMA: &str = "CREATE TABLE legacy_backups (chat_id TEXT PRIMARY KEY NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE, payload TEXT NOT NULL) STRICT";

const COMPACTION_SCHEMA: &str = "CREATE TABLE compactions (event_id TEXT PRIMARY KEY NOT NULL, chat_id TEXT NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE, payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB)) <= 262144)) STRICT";
const COMPACTION_SOURCE_SCHEMA: &str = "CREATE TABLE compaction_sources (event_id TEXT NOT NULL REFERENCES compactions(event_id) ON DELETE CASCADE, ordinal INTEGER NOT NULL CHECK(ordinal >= 0), source_id TEXT NOT NULL, PRIMARY KEY(event_id,ordinal)) STRICT";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionArchiveRecord {
    pub event_id: String,
    pub context_revision: String,
    pub summary_text: String,
    pub source_ids: Vec<String>,
    pub omitted_ranges: Vec<String>,
    pub mode: String,
    pub reason: String,
    pub before_tokens: u64,
    pub after_tokens: u64,
    pub before_bytes: usize,
    pub after_bytes: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionedCompaction {
    version: u32,
    record: CompactionArchiveRecord,
}
#[derive(Clone, Debug, Serialize)]
pub struct CompactionArchivePage {
    pub records: Vec<CompactionArchiveRecord>,
    pub next_before: Option<u64>,
}
pub(crate) struct RedactedCompaction(CompactionArchiveRecord);
impl RedactedCompaction {
    pub(crate) fn new(
        record: &CompactionArchiveRecord,
        mut redact: impl FnMut(&str) -> String,
    ) -> Result<Self> {
        validate_compaction(record)?;
        let mut record = record.clone();
        record.summary_text = if record.mode == "llm" {
            // Structured summaries are redacted before JSON re-encoding. Keys
            // are protocol, and quoted/newline secrets must be matched decoded.
            let mut value: serde_json::Value = serde_json::from_str(&record.summary_text)
                .map_err(|_| HistoryError::InvalidRecord)?;
            if !value.is_object() {
                return Err(HistoryError::InvalidRecord);
            }
            redact_json_leaves(&mut value, &mut redact);
            serde_json::to_string(&value).map_err(|_| HistoryError::InvalidRecord)?
        } else {
            redact(&record.summary_text)
        };
        for text in &mut record.omitted_ranges {
            *text = redact(text);
        }
        validate_compaction(&record)?;
        Ok(Self(record))
    }
    pub(crate) fn redact_again(&self, redact: impl FnMut(&str) -> String) -> Result<Self> {
        Self::new(&self.0, redact)
    }
    pub(crate) fn into_record(self) -> CompactionArchiveRecord {
        self.0
    }
    pub(crate) fn memory_bytes(&self) -> usize {
        self.0.summary_text.capacity()
            + self
                .0
                .source_ids
                .iter()
                .chain(&self.0.omitted_ranges)
                .map(|s| s.capacity() + 64)
                .sum::<usize>()
            + 2048
    }
}
fn redact_json_leaves(value: &mut serde_json::Value, redact: &mut impl FnMut(&str) -> String) {
    match value {
        serde_json::Value::String(text) => *text = redact(text),
        serde_json::Value::Array(values) => {
            for value in values {
                redact_json_leaves(value, redact);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                redact_json_leaves(value, redact);
            }
        }
        _ => {}
    }
}
fn validate_compaction(record: &CompactionArchiveRecord) -> Result<()> {
    require_id(&record.event_id)?;
    if record
        .context_revision
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .is_none()
        || record.summary_text.is_empty()
        || record.summary_text.len() > MAX_ASSISTANT_BYTES
        || record.summary_text.contains('\0')
        || !["quick", "llm"].contains(&record.mode.as_str())
        || !["manual", "token_pressure", "hard_byte_limit", "both"]
            .contains(&record.reason.as_str())
        || record.source_ids.is_empty()
        || record.source_ids.len() > 4096
        || record.omitted_ranges.len() > 4096
        || record.source_ids.iter().any(|id| {
            id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_:./".contains(&b))
        })
        || record
            .omitted_ranges
            .iter()
            .any(|text| text.len() > 1024 || text.contains('\0'))
        || record.after_tokens > record.before_tokens
        || record.after_bytes > record.before_bytes
        || (record.after_tokens == record.before_tokens
            && record.after_bytes == record.before_bytes)
    {
        return Err(HistoryError::InvalidRecord);
    }
    if record.source_ids.iter().collect::<BTreeSet<_>>().len() != record.source_ids.len() {
        return Err(HistoryError::InvalidRecord);
    }
    Ok(())
}
fn decode_compaction(payload: &str) -> Result<CompactionArchiveRecord> {
    let value: VersionedCompaction =
        serde_json::from_str(payload).map_err(|_| HistoryError::Corrupt)?;
    if value.version != 1 {
        return Err(HistoryError::UnsupportedSchema);
    }
    validate_compaction(&value.record).map_err(|_| HistoryError::Corrupt)?;
    Ok(value.record)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HistoryPreference {
    #[default]
    MemoryOnly,
    /// Set only after the user explicitly chooses to save redacted text locally.
    SaveRedactedText,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryError {
    Disabled,
    InvalidRecord,
    InvalidPath,
    UnsafeFile,
    Unavailable,
    Busy,
    Conflict,
    NotFound,
    LimitReached,
    UnsupportedSchema,
    Corrupt,
    SaveUncertain,
}
pub type Result<T> = std::result::Result<T, HistoryError>;
impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for HistoryError {}
impl From<rusqlite::Error> for HistoryError {
    fn from(error: rusqlite::Error) -> Self {
        // Test diagnostics contain numeric SQLite codes only, never its raw
        // message (which can contain SQL, paths or stored text).
        #[cfg(test)]
        if let rusqlite::Error::SqliteFailure(code, _) = &error {
            eprintln!(
                "history SQLite failure: primary={:?}, extended={}",
                code.code, code.extended_code
            );
        }
        // Never leak SQL, data, paths, credential values or raw SQLite errors.
        match error.sqlite_error_code() {
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => Self::Busy,
            Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => Self::Corrupt,
            Some(ErrorCode::DiskFull | ErrorCode::TooBig) => Self::LimitReached,
            Some(ErrorCode::ConstraintViolation) => Self::Conflict,
            _ => Self::Unavailable,
        }
    }
}
impl From<StorageError> for HistoryError {
    fn from(error: StorageError) -> Self {
        #[cfg(test)]
        eprintln!("history private-file failure: {error:?}");
        match error {
            StorageError::InvalidPath => Self::InvalidPath,
            StorageError::UnsafeFile | StorageError::UnsupportedFilesystem => Self::UnsafeFile,
            StorageError::TooLarge => Self::LimitReached,
            StorageError::Busy => Self::Busy,
            StorageError::Conflict => Self::Conflict,
            StorageError::SaveUncertain => Self::SaveUncertain,
            _ => Self::Unavailable,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArchivedMessageState {
    Complete,
    Cancelled,
    Error,
    Interrupted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedMessage {
    pub id: String,
    pub role: MessageRole,
    pub text: String,
    pub state: ArchivedMessageState,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedChat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ChatSource>,
    pub chat_id: String,
    pub task_id: String,
    pub title: String,
    pub messages: Vec<ArchivedMessage>,
}

/// A write capability with private contents and no Deserialize implementation.
/// Only the mandatory caller redactor below can construct one. It is deliberately
/// not a proof that every secret is known or that a caller supplied a real filter.
pub struct RedactedChat(ArchivedChat);
impl RedactedChat {
    pub(crate) fn retained_memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.0.chat_id.capacity()
            + self.0.task_id.capacity()
            + self.0.title.capacity()
            + self.0.messages.capacity() * std::mem::size_of::<ArchivedMessage>()
            + self
                .0
                .messages
                .iter()
                .map(|m| m.id.capacity() + m.text.capacity())
                .sum::<usize>()
            + 1024
    }
    /// Refresh filtering immediately before a queued write, since the host may
    /// have learned additional secret values after taking the snapshot.
    pub(crate) fn redact_again(
        &self,
        mut redact_known_secrets: impl FnMut(&str) -> String,
    ) -> Result<Self> {
        let mut chat = self.0.clone();
        chat.title = redact_known_secrets(&chat.title);
        redact_source(&mut chat.source, &mut redact_known_secrets);
        for message in &mut chat.messages {
            message.text = redact_known_secrets(&message.text);
        }
        validate_chat(&chat)?;
        Ok(Self(chat))
    }
    /// The host's callback must redact all known secrets in each supplied field.
    /// No raw run/error/model/endpoint/approval/terminal fields are copied.
    pub fn from_snapshot(
        snapshot: &ChatSnapshot,
        mut redact_known_secrets: impl FnMut(&str) -> String,
    ) -> Result<Self> {
        if snapshot.title.len() > 256 {
            return Err(HistoryError::InvalidRecord);
        }
        let mut total = 0usize;
        let mut messages = Vec::with_capacity(snapshot.messages.len());
        for message in &snapshot.messages {
            total = total
                .checked_add(message.text.len())
                .ok_or(HistoryError::InvalidRecord)?;
            if total > MAX_CAPTURE_BYTES || message.text.len() > text_limit(&message.role) {
                return Err(HistoryError::InvalidRecord);
            }
            messages.push(ArchivedMessage {
                id: message.id.clone(),
                role: message.role.clone(),
                text: redact_known_secrets(&message.text),
                state: match message.state {
                    MessageState::Complete => ArchivedMessageState::Complete,
                    MessageState::Streaming => ArchivedMessageState::Interrupted,
                    MessageState::Cancelled
                        if message
                            .error
                            .as_ref()
                            .is_some_and(|error| error.code == "HISTORY_INTERRUPTED") =>
                    {
                        ArchivedMessageState::Interrupted
                    }
                    MessageState::Cancelled => ArchivedMessageState::Cancelled,
                    MessageState::Error => ArchivedMessageState::Error,
                },
            });
        }
        let mut source = snapshot.source.clone();
        redact_source(&mut source, &mut redact_known_secrets);
        let chat = ArchivedChat {
            source,
            chat_id: snapshot.chat_id.clone(),
            task_id: snapshot.task_id.clone(),
            title: redact_known_secrets(&snapshot.title),
            messages,
        };
        validate_chat(&chat)?;
        Ok(Self(chat))
    }
}

#[derive(Clone, Debug)]
pub struct StoredChat {
    /// Separate SQLite CAS revision, not an authorization or a live-run token.
    pub revision: i64,
    pub listed: bool,
    pub updated_ms: i64,
    pub chat: ArchivedChat,
    pub message_count: usize,
    pub next_before: Option<u64>,
}
impl StoredChat {
    /// Inert transcript only. The host must initialize every runtime handle,
    /// permit, cancellation sender, terminal owner and approval map afresh.
    pub fn to_snapshot(&self) -> ChatSnapshot {
        ChatSnapshot {
            source: self.chat.source.clone(),
            restored_read_only: true,
            chat_id: self.chat.chat_id.clone(), task_id: self.chat.task_id.clone(),
            title: self.chat.title.clone(), active_run_id: None,
            revision: self.revision.to_string(),
            messages: self.chat.messages.iter().map(|message| {
                let (state, error) = match message.state {
                    ArchivedMessageState::Complete => (MessageState::Complete, None),
                    ArchivedMessageState::Cancelled => (MessageState::Cancelled, None),
                    ArchivedMessageState::Error => (MessageState::Error, Some(ApiError::new(
                        "HISTORY_PREVIOUS_ERROR", "This historical message ended with an error."))),
                    ArchivedMessageState::Interrupted => (MessageState::Cancelled, Some(ApiError::new(
                        "HISTORY_INTERRUPTED", "This message was interrupted. No run or remote action was resumed."))),
                };
                ChatMessage {
                    id: message.id.clone(), role: message.role.clone(), text: message.text.clone(),
                    state, error, run_id: String::new(), destination: None, model: None,
                }
            }).collect(),
        }
    }
}

/// Finite event vocabulary only. No free-text details, commands, raw output,
/// secrets, keyring references, destination strings or approval tokens.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[repr(i64)]
pub enum AuditKind {
    RunStarted = 0,
    RunInterrupted = 1,
    RunCompleted = 2,
    ApprovalRequested = 3,
    ApprovalGranted = 4,
    ApprovalDenied = 5,
    TerminalWriteRequested = 6,
    TerminalWriteFinished = 7,
    RemoteOutcomeUnknown = 8,
}
impl AuditKind {
    fn from_i64(value: i64) -> Result<Self> {
        match value {
            0 => Ok(Self::RunStarted),
            1 => Ok(Self::RunInterrupted),
            2 => Ok(Self::RunCompleted),
            3 => Ok(Self::ApprovalRequested),
            4 => Ok(Self::ApprovalGranted),
            5 => Ok(Self::ApprovalDenied),
            6 => Ok(Self::TerminalWriteRequested),
            7 => Ok(Self::TerminalWriteFinished),
            8 => Ok(Self::RemoteOutcomeUnknown),
            _ => Err(HistoryError::Corrupt),
        }
    }
}
#[derive(Clone, Debug)]
pub struct AuditRecord {
    pub event_id: String,
    pub chat_id: String,
    pub run_id: Option<String>,
    pub kind: AuditKind,
    pub created_ms: i64,
}

pub struct HistoryStore {
    // Drop connection before releasing the directory handles/ancestor pins.
    connection: Connection,
    directory: Arc<PrivateDirectory>,
    preference: HistoryPreference,
    schema_cookie: i64,
}
impl HistoryStore {
    /// MemoryOnly fails before touching the filesystem, even for an invalid path.
    /// Existing corrupt/unknown databases are never renamed, deleted or reset.
    pub fn open(path: &Path, preference: HistoryPreference) -> Result<Self> {
        if preference != HistoryPreference::SaveRedactedText {
            return Err(HistoryError::Disabled);
        }
        let initialization = PrivateDirectory::open_or_create(path)?;
        // Serialize startup and runtime file checks/transactions together.
        // Otherwise another instance can delete its rollback journal while we
        // inspect its metadata, before SQLite's own busy handler can help.
        initialization.update(ACCESS_LOCK, 0, |_| {
            let directory = PrivateDirectory::open_or_create(path)?;
            Ok((Self::open_initialized(path, directory, preference), None))
        })?
    }

    fn open_initialized(
        path: &Path,
        directory: PrivateDirectory,
        preference: HistoryPreference,
    ) -> Result<Self> {
        check_sidecars(&directory)?;
        let created = directory.prepare_database_file(DATABASE_NAME, MAX_DATABASE_BYTES)?;
        check_files(&directory)?;
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = Connection::open_with_flags(path.join(DATABASE_NAME), flags)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        let mode: String = connection.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if mode != "delete" {
            return Err(HistoryError::UnsupportedSchema);
        }
        connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_PAYLOAD_BYTES as i32 + 4096)?;
        connection.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 16 * 1024)?;
        connection.set_limit(Limit::SQLITE_LIMIT_COLUMN, 32)?;
        connection.set_limit(Limit::SQLITE_LIMIT_EXPR_DEPTH, 32)?;
        connection.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
        connection.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA temp_store=MEMORY; PRAGMA mmap_size=0; PRAGMA secure_delete=ON; PRAGMA synchronous=EXTRA;")?;
        let mut store = Self {
            connection,
            directory: Arc::new(directory),
            preference,
            schema_cookie: 0,
        };
        store.initialize(created)?;
        check_files(&store.directory)?;
        Ok(store)
    }

    /// Revoking consent blocks every later read/write through this handle. It
    /// does not silently delete existing history. The host owns the persisted
    /// preference and must update every open handle when it changes.
    pub fn set_preference(&mut self, preference: HistoryPreference) {
        self.preference = preference;
    }

    fn initialize(&mut self, created: bool) -> Result<()> {
        let mode: String = self
            .connection
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if mode != "delete" {
            return Err(HistoryError::UnsupportedSchema);
        }
        // Enforce the independent disk quota before migration allocates pages.
        self.connection.pragma_update(
            None,
            "max_page_count",
            (MAX_DATABASE_BYTES / PAGE_SIZE) as i64,
        )?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let app: i64 = tx.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        if version == 0 && app == 0 && created {
            let objects: i64 =
                tx.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
            if objects != 0 {
                return Err(HistoryError::Corrupt);
            }
            tx.execute_batch(CHAT_SCHEMA)?;
            tx.execute_batch(AUDIT_SCHEMA)?;
            tx.execute_batch(METADATA_SCHEMA)?;
            tx.execute_batch(ENTRY_SCHEMA)?;
            tx.execute_batch(BACKUP_SCHEMA)?;
            tx.execute_batch(COMPACTION_SCHEMA)?;
            tx.execute_batch(COMPACTION_SOURCE_SCHEMA)?;
            tx.execute("INSERT INTO metadata(id,revision) VALUES(1,0)", [])?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if (version == 1 || version == 2) && app == APPLICATION_ID {
            // Verify the complete old layout and every row before changing its
            // format marker. Invalid/future archives are left untouched.
            verify_schema_version(&tx, false)?;
            {
                let mut statement = tx.prepare(
                    "SELECT chat_id,task_id,revision,listed,updated_ms,payload FROM chats",
                )?;
                let rows = statement.query_map([], raw_chat)?;
                for row in rows {
                    let row = row?;
                    let value: serde_json::Value =
                        serde_json::from_str(&row.5).map_err(|_| HistoryError::Corrupt)?;
                    let fields = value.as_object().ok_or(HistoryError::Corrupt)?;
                    if version == 1
                        && (fields.len() != 4
                            || fields.keys().any(|key| {
                                !["chat_id", "task_id", "title", "messages"].contains(&key.as_str())
                            }))
                    {
                        return Err(HistoryError::UnsupportedSchema);
                    }
                    decode(row)?;
                }
            }
            tx.execute_batch(ENTRY_SCHEMA)?;
            tx.execute_batch(BACKUP_SCHEMA)?;
            tx.execute_batch(COMPACTION_SCHEMA)?;
            tx.execute_batch(COMPACTION_SOURCE_SCHEMA)?;
            {
                let mut statement = tx.prepare(
                    "SELECT chat_id,task_id,revision,listed,updated_ms,payload FROM chats",
                )?;
                let rows = statement.query_map([], raw_chat)?;
                for row in rows {
                    let raw = row?;
                    let backup = raw.5.clone();
                    let decoded = decode(raw)?;
                    tx.execute(
                        "INSERT INTO legacy_backups(chat_id,payload) VALUES(?1,?2)",
                        params![decoded.chat.chat_id, backup],
                    )?;
                    write_entries(&tx, &decoded.chat)?;
                    let payload = encode(&RedactedChat(decoded.chat.clone()))?;
                    tx.execute(
                        "UPDATE chats SET payload=?1 WHERE chat_id=?2",
                        params![payload, decoded.chat.chat_id],
                    )?;
                }
            }
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if version != SCHEMA_VERSION || app != APPLICATION_ID {
            return Err(if version == 0 && app == 0 {
                HistoryError::Corrupt
            } else {
                HistoryError::UnsupportedSchema
            });
        }
        verify_schema(&tx)?;
        let check: String = tx.query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(HistoryError::Corrupt);
        }
        let page_size: i64 = tx.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        if page_size != PAGE_SIZE as i64 {
            return Err(HistoryError::UnsupportedSchema);
        }
        let pages: i64 = tx.query_row("PRAGMA max_page_count=8192", [], |r| r.get(0))?;
        if pages != (MAX_DATABASE_BYTES / PAGE_SIZE) as i64 {
            return Err(HistoryError::LimitReached);
        }
        check_counts(&tx)?;
        let foreign_key_violation = tx.prepare("PRAGMA foreign_key_check")?.exists([])?;
        if foreign_key_violation {
            return Err(HistoryError::Corrupt);
        }
        validate_entries(&tx)?;
        validate_compactions(&tx)?;
        audit_rows(&tx, None)?;
        self.schema_cookie = tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
        tx.commit().map_err(commit_error)?;
        // Verify all bounded historical rows now, instead of silently skipping
        // a damaged record and telling the user their transcript is empty.
        self.list_locked(true)?;
        self.validate_all_audit()?;
        validate_entries(&self.connection)?;
        Ok(())
    }

    // The private-file checks must precede SQLite access, but SQLite's locks do
    // not cover those checks. Hold a separate stable cross-process file lock
    // until each transaction/statement is dropped, including reads and startup.
    // This prevents a cooperating writer's journal lifecycle from racing the
    // metadata checks, independently of OS deletion semantics. SQLite IMMEDIATE
    // transactions and expected-revision checks remain authoritative for CAS.
    // The access lock has PrivateDirectory's bounded five-second wait; SQLite
    // retains its independent two-second busy timeout for foreign connections.
    // Foreign/noncooperating file changes still fail closed; never retry an
    // uncertain commit or turn a filesystem error into a revision conflict.
    fn with_access<T>(&self, operation: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        if self.preference != HistoryPreference::SaveRedactedText {
            return Err(HistoryError::Disabled);
        }
        self.directory
            .update(ACCESS_LOCK, 0, |_| Ok((operation(self), None)))?
    }

    fn with_access_mut<T>(&mut self, operation: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.preference != HistoryPreference::SaveRedactedText {
            return Err(HistoryError::Disabled);
        }
        let directory = Arc::clone(&self.directory);
        directory.update(ACCESS_LOCK, 0, |_| Ok((operation(self), None)))?
    }

    fn guard(&self) -> Result<()> {
        if self.preference != HistoryPreference::SaveRedactedText {
            return Err(HistoryError::Disabled);
        }
        check_files(&self.directory)?;
        Ok(())
    }

    pub fn create(&mut self, prepared: &RedactedChat) -> Result<StoredChat> {
        self.with_access_mut(|store| store.create_locked(prepared))
    }

    fn create_locked(&mut self, prepared: &RedactedChat) -> Result<StoredChat> {
        self.guard()?;
        let payload = encode(prepared)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_schema_state(&tx, self.schema_cookie)?;
        let count: i64 = tx.query_row("SELECT count(*) FROM chats", [], |r| r.get(0))?;
        if count >= MAX_CHATS as i64 {
            return Err(HistoryError::LimitReached);
        }
        let now = now_ms()?;
        let revision = allocate_revision(&tx)?;
        tx.execute("INSERT INTO chats(chat_id, task_id, revision, listed, updated_ms, payload) VALUES (?1,?2,?3,1,?4,?5)",
            params![prepared.0.chat_id, prepared.0.task_id, revision, now, payload])?;
        write_entries(&tx, &prepared.0)?;
        let total_messages = message_count(&tx, &prepared.0.chat_id)?;
        tx.commit().map_err(commit_error)?;
        Ok(StoredChat {
            revision,
            listed: true,
            updated_ms: now,
            chat: prepared.0.clone(),
            message_count: total_messages,
            next_before: None,
        })
    }

    pub fn read(&self, chat_id: &str) -> Result<StoredChat> {
        self.with_access(|store| store.read_locked(chat_id))
    }

    fn read_locked(&self, chat_id: &str) -> Result<StoredChat> {
        self.guard()?;
        require_id(chat_id)?;
        let tx = self.connection.unchecked_transaction()?;
        check_schema_state(&tx, self.schema_cookie)?;
        let raw = tx.query_row("SELECT chat_id,task_id,revision,listed,updated_ms,payload FROM chats WHERE chat_id=?1", [chat_id], raw_chat).optional()?;
        let mut row = decode(raw.ok_or(HistoryError::NotFound)?)?;
        load_page(&tx, &mut row, None, ARCHIVE_PAGE_MESSAGES)?;
        Ok(row)
    }

    /// Reads a display-only bounded page, newest page by default. Cursor is an
    /// exclusive stable sequence boundary; this never changes model memory.
    pub fn page(
        &self,
        chat_id: &str,
        expected_revision: i64,
        before: Option<u64>,
        limit: usize,
    ) -> Result<StoredChat> {
        self.with_access(|store| {
            store.guard()?;
            require_id(chat_id)?;
            let tx = store.connection.unchecked_transaction()?;
            check_schema_state(&tx, store.schema_cookie)?;
            let mut row = checked_target(&tx, chat_id, expected_revision)?;
            load_page(&tx, &mut row, before, limit)?;
            Ok(row)
        })
    }

    /// Delisted chats still count toward the cap until explicitly forgotten.
    pub fn list(&self, include_delisted: bool) -> Result<Vec<StoredChat>> {
        self.with_access(|store| store.list_locked(include_delisted))
    }

    fn list_locked(&self, include_delisted: bool) -> Result<Vec<StoredChat>> {
        self.guard()?;
        let tx = self.connection.unchecked_transaction()?;
        check_schema_state(&tx, self.schema_cookie)?;
        let mut statement = tx.prepare("SELECT chat_id,task_id,revision,listed,updated_ms,payload FROM chats WHERE listed=1 OR ?1 ORDER BY updated_ms,chat_id LIMIT ?2")?;
        let rows =
            statement.query_map(params![include_delisted, MAX_CHATS as i64 + 1], raw_chat)?;
        let mut result = Vec::new();
        for row in rows {
            let mut row = decode(row?)?;
            row.message_count = message_count(&tx, &row.chat.chat_id)?;
            result.push(row);
        }
        if result.len() > MAX_CHATS {
            return Err(HistoryError::LimitReached);
        }
        Ok(result)
    }

    pub fn replace(
        &mut self,
        prepared: &RedactedChat,
        expected_revision: i64,
    ) -> Result<StoredChat> {
        self.with_access_mut(|store| store.replace_locked(prepared, expected_revision))
    }

    fn replace_locked(
        &mut self,
        prepared: &RedactedChat,
        expected_revision: i64,
    ) -> Result<StoredChat> {
        self.guard()?;
        valid_revision(expected_revision)?;
        let payload = encode(prepared)?;
        let now = now_ms()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_schema_state(&tx, self.schema_cookie)?;
        let previous = checked_target(&tx, &prepared.0.chat_id, expected_revision)?;
        if previous.chat.task_id != prepared.0.task_id {
            return Err(HistoryError::Conflict);
        }
        let revision = allocate_revision(&tx)?;
        let changed = tx.execute("UPDATE chats SET payload=?1,revision=?2,updated_ms=?3 WHERE chat_id=?4 AND task_id=?5 AND revision=?6",
            params![payload,revision,now,prepared.0.chat_id,prepared.0.task_id,expected_revision])?;
        if changed != 1 {
            return Err(HistoryError::Conflict);
        }
        let listed: bool = tx.query_row(
            "SELECT listed FROM chats WHERE chat_id=?1",
            [&prepared.0.chat_id],
            |r| r.get(0),
        )?;
        write_entries(&tx, &prepared.0)?;
        let total_messages = message_count(&tx, &prepared.0.chat_id)?;
        tx.commit().map_err(commit_error)?;
        Ok(StoredChat {
            revision,
            listed,
            updated_ms: now,
            chat: prepared.0.clone(),
            message_count: total_messages,
            next_before: None,
        })
    }

    /// Hide/show without discarding text; a stale caller cannot change visibility.
    pub fn set_listed(
        &mut self,
        chat_id: &str,
        expected_revision: i64,
        listed: bool,
    ) -> Result<i64> {
        self.with_access_mut(|store| store.set_listed_locked(chat_id, expected_revision, listed))
    }

    fn set_listed_locked(
        &mut self,
        chat_id: &str,
        expected_revision: i64,
        listed: bool,
    ) -> Result<i64> {
        self.guard()?;
        require_id(chat_id)?;
        valid_revision(expected_revision)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_schema_state(&tx, self.schema_cookie)?;
        checked_target(&tx, chat_id, expected_revision)?;
        let revision = allocate_revision(&tx)?;
        let changed = tx.execute(
            "UPDATE chats SET listed=?1,revision=?2,updated_ms=?3 WHERE chat_id=?4 AND revision=?5",
            params![listed, revision, now_ms()?, chat_id, expected_revision],
        )?;
        if changed != 1 {
            return Err(HistoryError::Conflict);
        }
        tx.commit().map_err(commit_error)?;
        Ok(revision)
    }

    /// Explicit logical removal, including this chat's audit records. Secure
    /// delete reduces old SQLite page contents, but is not guaranteed erasure
    /// from journals, snapshots, backups, SSDs or other storage layers.
    pub fn forget(&mut self, chat_id: &str, expected_revision: i64) -> Result<()> {
        self.with_access_mut(|store| store.forget_locked(chat_id, expected_revision))
    }

    fn forget_locked(&mut self, chat_id: &str, expected_revision: i64) -> Result<()> {
        self.guard()?;
        require_id(chat_id)?;
        valid_revision(expected_revision)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_schema_state(&tx, self.schema_cookie)?;
        checked_target(&tx, chat_id, expected_revision)?;
        audit_rows(&tx, Some(chat_id))?;
        let changed = tx.execute(
            "DELETE FROM chats WHERE chat_id=?1 AND revision=?2",
            params![chat_id, expected_revision],
        )?;
        if changed != 1 {
            return Err(HistoryError::Conflict);
        }
        tx.commit().map_err(commit_error)?;
        Ok(())
    }

    /// Factual audit only; records are not usable as approvals or execution input.
    /// Full retention fails visibly at the cap; no records are silently evicted.
    pub fn append_audit(
        &mut self,
        chat_id: &str,
        expected_revision: i64,
        run_id: Option<&str>,
        kind: AuditKind,
    ) -> Result<AuditRecord> {
        self.with_access_mut(|store| {
            store.append_audit_locked(chat_id, expected_revision, run_id, kind)
        })
    }

    fn append_audit_locked(
        &mut self,
        chat_id: &str,
        expected_revision: i64,
        run_id: Option<&str>,
        kind: AuditKind,
    ) -> Result<AuditRecord> {
        self.guard()?;
        require_id(chat_id)?;
        if let Some(run_id) = run_id {
            require_id(run_id)?;
        }
        valid_revision(expected_revision)?;
        let record = AuditRecord {
            event_id: uuid::Uuid::new_v4().to_string(),
            chat_id: chat_id.into(),
            run_id: run_id.map(str::to_owned),
            kind,
            created_ms: now_ms()?,
        };
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_schema_state(&tx, self.schema_cookie)?;
        checked_target(&tx, chat_id, expected_revision)?;
        let count: i64 = tx.query_row("SELECT count(*) FROM audit", [], |r| r.get(0))?;
        if count >= MAX_AUDIT_EVENTS as i64 {
            return Err(HistoryError::LimitReached);
        }
        tx.execute(
            "INSERT INTO audit(event_id,chat_id,run_id,kind,created_ms) VALUES(?1,?2,?3,?4,?5)",
            params![
                record.event_id,
                record.chat_id,
                record.run_id,
                record.kind as i64,
                record.created_ms
            ],
        )?;
        tx.commit().map_err(commit_error)?;
        Ok(record)
    }

    pub(crate) fn append_compaction(
        &mut self,
        chat_id: &str,
        prepared: &RedactedCompaction,
    ) -> Result<()> {
        self.with_access_mut(|store| {
            store.guard()?;
            require_id(chat_id)?;
            validate_compaction(&prepared.0)?;
            let payload = serde_json::to_string(&VersionedCompaction {
                version: 1,
                record: prepared.0.clone(),
            })
            .map_err(|_| HistoryError::InvalidRecord)?;
            if payload.len() > 256 * 1024 {
                return Err(HistoryError::LimitReached);
            }
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            check_schema_state(&tx, store.schema_cookie)?;
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM chats WHERE chat_id=?1)",
                [chat_id],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(HistoryError::NotFound);
            }
            let count: i64 = tx.query_row("SELECT count(*) FROM compactions", [], |r| r.get(0))?;
            if count >= MAX_AUDIT_EVENTS as i64 {
                return Err(HistoryError::LimitReached);
            }
            tx.execute(
                "INSERT INTO compactions(event_id,chat_id,payload) VALUES(?1,?2,?3)",
                params![prepared.0.event_id, chat_id, payload],
            )?;
            for (ordinal, source) in prepared.0.source_ids.iter().enumerate() {
                tx.execute(
                    "INSERT INTO compaction_sources(event_id,ordinal,source_id) VALUES(?1,?2,?3)",
                    params![prepared.0.event_id, ordinal as i64, source],
                )?;
            }
            tx.commit().map_err(commit_error)
        })
    }
    /// Inert historical compaction facts only. This never restores a ledger.
    pub fn compaction_page(
        &self,
        chat_id: &str,
        before: Option<u64>,
        limit: usize,
    ) -> Result<CompactionArchivePage> {
        self.with_access(|store| {
            store.guard()?; require_id(chat_id)?;
            if limit==0||limit>128||before.is_some_and(|n|n==0||n>i64::MAX as u64){return Err(HistoryError::InvalidRecord);}
            let tx=store.connection.unchecked_transaction()?;check_schema_state(&tx,store.schema_cookie)?;
            let mut statement=tx.prepare("SELECT rowid,payload FROM compactions WHERE chat_id=?1 AND rowid < ?2 ORDER BY rowid DESC LIMIT ?3")?;
            let mut rows=statement.query(params![chat_id,before.unwrap_or(i64::MAX as u64) as i64,limit as i64])?;
            let mut records=Vec::new();let mut bytes=0;let mut first=None;
            while let Some(row)=rows.next()? {
                let payload:String=row.get(1)?;
                if !records.is_empty() && bytes+payload.len()>256*1024 {break;}
                bytes+=payload.len();first=Some(row.get::<_,i64>(0)?);records.push(decode_compaction(&payload)?);
            }
            let next_before=if let Some(first)=first {let more:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM compactions WHERE chat_id=?1 AND rowid < ?2)",params![chat_id,first],|r|r.get(0))?;more.then_some(first as u64)}else{None};
            records.reverse();Ok(CompactionArchivePage{records,next_before})
        })
    }

    pub fn read_audit(&self, chat_id: &str) -> Result<Vec<AuditRecord>> {
        self.with_access(|store| store.read_audit_locked(chat_id))
    }

    fn read_audit_locked(&self, chat_id: &str) -> Result<Vec<AuditRecord>> {
        self.guard()?;
        require_id(chat_id)?;
        self.audit_rows(Some(chat_id))
    }
    fn validate_all_audit(&self) -> Result<()> {
        self.audit_rows(None).map(|_| ())
    }
    fn audit_rows(&self, chat_id: Option<&str>) -> Result<Vec<AuditRecord>> {
        let tx = self.connection.unchecked_transaction()?;
        check_schema_state(&tx, self.schema_cookie)?;
        audit_rows(&tx, chat_id)
    }
}

fn validate_compactions(connection: &Connection) -> Result<()> {
    let count: i64 = connection.query_row("SELECT count(*) FROM compactions", [], |r| r.get(0))?;
    if count > MAX_AUDIT_EVENTS as i64 {
        return Err(HistoryError::LimitReached);
    }
    let mut statement = connection.prepare("SELECT event_id,payload FROM compactions")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let event_id: String = row.get(0)?;
        let record = decode_compaction(&row.get::<_, String>(1)?)?;
        if record.event_id != event_id {
            return Err(HistoryError::Corrupt);
        }
        let mut sources = connection.prepare(
            "SELECT ordinal,source_id FROM compaction_sources WHERE event_id=?1 ORDER BY ordinal",
        )?;
        let mut indexed = sources.query([&event_id])?;
        let mut ordinal = 0;
        while let Some(source) = indexed.next()? {
            if source.get::<_, i64>(0)? != ordinal as i64
                || record.source_ids.get(ordinal) != Some(&source.get::<_, String>(1)?)
            {
                return Err(HistoryError::Corrupt);
            }
            ordinal += 1;
        }
        if ordinal != record.source_ids.len() {
            return Err(HistoryError::Corrupt);
        }
    }
    Ok(())
}

fn message_count(connection: &Connection, chat_id: &str) -> Result<usize> {
    Ok(connection.query_row(
        "SELECT count(*) FROM entries WHERE chat_id=?1",
        [chat_id],
        |r| r.get::<_, i64>(0),
    )? as usize)
}
fn decode_entry(payload: &str, id: &str) -> Result<ArchivedMessage> {
    let message: ArchivedMessage =
        serde_json::from_str(payload).map_err(|_| HistoryError::Corrupt)?;
    if message.id != id
        || require_id(id).is_err()
        || message.text.contains('\0')
        || message.text.len() > text_limit(&message.role)
    {
        return Err(HistoryError::Corrupt);
    }
    Ok(message)
}
fn validate_entries(connection: &Connection) -> Result<()> {
    let mut statement = connection.prepare("SELECT message_id,payload FROM entries")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        decode_entry(&row.get::<_, String>(1)?, &row.get::<_, String>(0)?)?;
    }
    Ok(())
}
fn write_entries(connection: &Connection, chat: &ArchivedChat) -> Result<()> {
    let mut sequence: i64 = connection.query_row(
        "SELECT coalesce(max(sequence),0) FROM entries WHERE chat_id=?1",
        [&chat.chat_id],
        |r| r.get(0),
    )?;
    for message in &chat.messages {
        let payload = serde_json::to_string(message).map_err(|_| HistoryError::InvalidRecord)?;
        let old: Option<String> = connection
            .query_row(
                "SELECT payload FROM entries WHERE chat_id=?1 AND message_id=?2",
                params![chat.chat_id, message.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = old {
            let previous = decode_entry(&old, &message.id)?;
            if previous.role != message.role {
                return Err(HistoryError::Conflict);
            }
            // A streaming assistant keeps its stable ID. Captures can finish or
            // refresh redaction, but omitting an ID never deletes its original.
            if old != payload {
                connection.execute(
                    "UPDATE entries SET payload=?1 WHERE chat_id=?2 AND message_id=?3",
                    params![payload, chat.chat_id, message.id],
                )?;
            }
        } else {
            sequence = sequence.checked_add(1).ok_or(HistoryError::LimitReached)?;
            connection.execute(
                "INSERT INTO entries(chat_id,sequence,message_id,payload) VALUES(?1,?2,?3,?4)",
                params![chat.chat_id, sequence, message.id, payload],
            )?;
        }
    }
    Ok(())
}
fn load_page(
    connection: &Connection,
    row: &mut StoredChat,
    before: Option<u64>,
    limit: usize,
) -> Result<()> {
    if limit == 0
        || limit > ARCHIVE_PAGE_MESSAGES
        || before.is_some_and(|n| n == 0 || n > i64::MAX as u64)
    {
        return Err(HistoryError::InvalidRecord);
    }
    let mut statement = connection.prepare("SELECT sequence,message_id,payload FROM entries WHERE chat_id=?1 AND sequence < ?2 ORDER BY sequence DESC LIMIT ?3")?;
    let mut rows = statement.query(params![
        row.chat.chat_id,
        before.unwrap_or(i64::MAX as u64) as i64,
        limit as i64
    ])?;
    let mut messages = Vec::new();
    let mut bytes = 0usize;
    let mut first = None;
    while let Some(entry) = rows.next()? {
        let sequence: i64 = entry.get(0)?;
        let message = decode_entry(&entry.get::<_, String>(2)?, &entry.get::<_, String>(1)?)?;
        if !messages.is_empty() && bytes + message.text.len() > ARCHIVE_PAGE_BYTES {
            break;
        }
        bytes += message.text.len();
        messages.push(message);
        first = Some(sequence);
    }
    messages.reverse();
    row.chat.messages = messages;
    row.message_count = message_count(connection, &row.chat.chat_id)?;
    row.next_before = if let Some(first) = first {
        let older: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM entries WHERE chat_id=?1 AND sequence < ?2)",
            params![row.chat.chat_id, first],
            |r| r.get(0),
        )?;
        older.then_some(first as u64)
    } else {
        None
    };
    Ok(())
}

fn audit_rows(connection: &Connection, chat_id: Option<&str>) -> Result<Vec<AuditRecord>> {
    let mut statement = connection.prepare("SELECT event_id,chat_id,run_id,kind,created_ms FROM audit WHERE ?1 IS NULL OR chat_id=?1 ORDER BY rowid LIMIT ?2")?;
    let rows = statement.query_map(params![chat_id, MAX_AUDIT_EVENTS as i64 + 1], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (event_id, chat_id, run_id, kind, created_ms) = row?;
        if require_id(&event_id).is_err()
            || require_id(&chat_id).is_err()
            || run_id.as_deref().is_some_and(|id| require_id(id).is_err())
            || created_ms < 0
        {
            return Err(HistoryError::Corrupt);
        }
        result.push(AuditRecord {
            event_id,
            chat_id,
            run_id,
            kind: AuditKind::from_i64(kind)?,
            created_ms,
        });
    }
    if result.len() > MAX_AUDIT_EVENTS {
        return Err(HistoryError::LimitReached);
    }
    Ok(result)
}

fn check_schema_state(connection: &Connection, expected_cookie: i64) -> Result<()> {
    let cookie: i64 = connection.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let app: i64 = connection.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    if cookie != expected_cookie || version != SCHEMA_VERSION || app != APPLICATION_ID {
        return Err(HistoryError::UnsupportedSchema);
    }
    checked_counter(connection)?;
    Ok(())
}
fn checked_counter(connection: &Connection) -> Result<i64> {
    let counter: Option<i64> = connection
        .query_row("SELECT revision FROM metadata WHERE id=1", [], |r| r.get(0))
        .optional()?;
    let max_revision: i64 =
        connection.query_row("SELECT coalesce(max(revision),0) FROM chats", [], |r| {
            r.get(0)
        })?;
    counter
        .filter(|value| *value >= max_revision && *value >= 0)
        .ok_or(HistoryError::Corrupt)
}
fn allocate_revision(connection: &Connection) -> Result<i64> {
    // The counter carries no chat identifiers and survives forget, so a
    // re-created UUID never accepts a stale pre-forget CAS revision.
    let next = checked_counter(connection)?
        .checked_add(1)
        .ok_or(HistoryError::LimitReached)?;
    connection.execute("UPDATE metadata SET revision=?1 WHERE id=1", [next])?;
    Ok(next)
}
fn checked_target(
    connection: &Connection,
    chat_id: &str,
    expected_revision: i64,
) -> Result<StoredChat> {
    let raw = connection
        .query_row(
            "SELECT chat_id,task_id,revision,listed,updated_ms,payload FROM chats WHERE chat_id=?1",
            [chat_id],
            raw_chat,
        )
        .optional()?;
    let record = decode(raw.ok_or(HistoryError::Conflict)?)?;
    if record.revision != expected_revision {
        return Err(HistoryError::Conflict);
    }
    Ok(record)
}

fn check_sidecars(directory: &PrivateDirectory) -> Result<()> {
    // Never touch or remove foreign WAL state. Refuse even an empty sidecar so
    // SQLite cannot open it before the host knows which journal mode it uses.
    for name in ["history.sqlite3-wal", "history.sqlite3-shm"] {
        if directory.validate_database_file(name, MAX_JOURNAL_BYTES)? {
            return Err(HistoryError::UnsafeFile);
        }
    }
    // A private hot rollback journal is valid crash recovery; SQLite recovers
    // only local database transactions, never any remote command/run.
    directory.validate_database_file("history.sqlite3-journal", MAX_JOURNAL_BYTES)?;
    Ok(())
}
fn check_files(directory: &PrivateDirectory) -> Result<()> {
    if !directory.validate_database_file(DATABASE_NAME, MAX_DATABASE_BYTES)? {
        return Err(HistoryError::Corrupt);
    }
    check_sidecars(directory)
}
fn verify_schema(connection: &Connection) -> Result<()> {
    verify_schema_version(connection, true)
}
fn verify_schema_version(connection: &Connection, paged: bool) -> Result<()> {
    let mut statement =
        connection.prepare("SELECT type,name,sql FROM sqlite_schema ORDER BY name")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut tables = BTreeSet::new();
    for row in rows {
        let (kind, name, sql) = row?;
        match (kind.as_str(), name.as_str(), sql.as_deref()) {
            ("table", "compactions", Some(COMPACTION_SCHEMA)) if paged => {
                tables.insert("compactions");
            }
            ("table", "compaction_sources", Some(COMPACTION_SOURCE_SCHEMA)) if paged => {
                tables.insert("compaction_sources");
            }
            (
                "index",
                "sqlite_autoindex_compactions_1" | "sqlite_autoindex_compaction_sources_1",
                None,
            ) if paged => {}

            ("table", "entries", Some(ENTRY_SCHEMA)) if paged => {
                tables.insert("entries");
            }
            ("table", "legacy_backups", Some(BACKUP_SCHEMA)) if paged => {
                tables.insert("legacy_backups");
            }
            (
                "index",
                "sqlite_autoindex_entries_1"
                | "sqlite_autoindex_entries_2"
                | "sqlite_autoindex_legacy_backups_1",
                None,
            ) if paged => {}
            ("table", "chats", Some(CHAT_SCHEMA)) => {
                tables.insert("chats");
            }
            ("table", "audit", Some(AUDIT_SCHEMA)) => {
                tables.insert("audit");
            }
            ("table", "metadata", Some(METADATA_SCHEMA)) => {
                tables.insert("metadata");
            }
            (
                "index",
                "sqlite_autoindex_chats_1"
                | "sqlite_autoindex_chats_2"
                | "sqlite_autoindex_audit_1",
                None,
            ) => {}
            _ => return Err(HistoryError::UnsupportedSchema),
        }
    }
    if tables.len() != if paged { 7 } else { 3 } {
        return Err(HistoryError::Corrupt);
    }
    Ok(())
}
fn check_counts(connection: &Connection) -> Result<()> {
    let (chats, audit): (i64, i64) = connection.query_row(
        "SELECT (SELECT count(*) FROM chats),(SELECT count(*) FROM audit)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if chats > MAX_CHATS as i64 || audit > MAX_AUDIT_EVENTS as i64 {
        return Err(HistoryError::LimitReached);
    }
    checked_counter(connection)?;
    Ok(())
}
fn now_ms() -> Result<i64> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| HistoryError::Unavailable)?
            .as_millis(),
    )
    .map_err(|_| HistoryError::Unavailable)
}
fn require_id(value: &str) -> Result<()> {
    if value.len() == 36
        && uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
    {
        Ok(())
    } else {
        Err(HistoryError::InvalidRecord)
    }
}
fn valid_revision(value: i64) -> Result<()> {
    if value < 1 {
        Err(HistoryError::InvalidRecord)
    } else {
        Ok(())
    }
}

fn text_limit(role: &MessageRole) -> usize {
    match role {
        MessageRole::User => MAX_MESSAGE_BYTES,
        MessageRole::Assistant => MAX_ASSISTANT_BYTES,
    }
}
fn redact_source(source: &mut Option<ChatSource>, redact: &mut impl FnMut(&str) -> String) {
    if let Some(source) = source {
        source.host = redact(&source.host);
        source.username = redact(&source.username);
    }
}
fn validate_chat(chat: &ArchivedChat) -> Result<()> {
    if let Some(source) = &chat.source {
        if source.host.is_empty()
            || source.host.len() > 253
            || source.host.chars().any(char::is_control)
            || source.username.is_empty()
            || source.username.len() > 128
            || source.username.chars().any(char::is_control)
            || source.port == 0
            || require_id(&source.session_id).is_err()
            || source.generation.is_empty()
            || source.generation.len() > 32
            || !source.generation.bytes().all(|c| c.is_ascii_digit())
        {
            return Err(HistoryError::InvalidRecord);
        }
    }
    require_id(&chat.chat_id)?;
    require_id(&chat.task_id)?;
    if chat.title.trim().is_empty()
        || chat.title.len() > 256
        || chat.title.chars().any(char::is_control)
    {
        return Err(HistoryError::InvalidRecord);
    }
    let mut ids = BTreeSet::new();
    let mut total = 0usize;
    for message in &chat.messages {
        require_id(&message.id)?;
        if !ids.insert(&message.id)
            || message.text.contains('\0')
            || message.text.len() > text_limit(&message.role)
        {
            return Err(HistoryError::InvalidRecord);
        }
        total = total
            .checked_add(message.text.len())
            .ok_or(HistoryError::InvalidRecord)?;
    }
    if total > MAX_CAPTURE_BYTES {
        return Err(HistoryError::InvalidRecord);
    }
    Ok(())
}
fn encode(chat: &RedactedChat) -> Result<String> {
    validate_chat(&chat.0)?;
    let metadata = ArchivedChat {
        source: chat.0.source.clone(),
        chat_id: chat.0.chat_id.clone(),
        task_id: chat.0.task_id.clone(),
        title: chat.0.title.clone(),
        messages: Vec::new(),
    };
    let payload = serde_json::to_string(&metadata).map_err(|_| HistoryError::InvalidRecord)?;
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(HistoryError::LimitReached);
    }
    Ok(payload)
}
type RawChat = (String, String, i64, i64, i64, String);
fn raw_chat(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawChat> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}
fn decode(
    (chat_id, task_id, revision, listed, updated_ms, payload): RawChat,
) -> Result<StoredChat> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(HistoryError::LimitReached);
    }
    let chat: ArchivedChat = serde_json::from_str(&payload).map_err(|_| HistoryError::Corrupt)?;
    validate_chat(&chat).map_err(|_| HistoryError::Corrupt)?;
    if chat.chat_id != chat_id
        || chat.task_id != task_id
        || revision < 1
        || ![0, 1].contains(&listed)
        || updated_ms < 0
    {
        return Err(HistoryError::Corrupt);
    }
    Ok(StoredChat {
        revision,
        listed: listed == 1,
        updated_ms,
        message_count: chat.messages.len(),
        next_before: None,
        chat,
    })
}
fn commit_error(error: rusqlite::Error) -> HistoryError {
    match HistoryError::from(error) {
        HistoryError::Busy => HistoryError::Busy,
        _ => HistoryError::SaveUncertain,
    }
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
