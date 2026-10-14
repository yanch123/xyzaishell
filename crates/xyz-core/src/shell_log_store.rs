//! Independent bounded local plaintext shell log. Shell reports are untrusted,
//! never execution authority. Open only after explicit host consent. The host
//! invalidates queued captures on pause; this store never contacts a model.
//! Private files are not encryption or protection against same-user malware,
//! administrators, backups, or storage remanence.
use crate::{
    persistence::HistoryError as E,
    shell_log_types::*,
    storage::{private_files::PrivateDirectory, StorageError},
};
use rusqlite::{
    config::DbConfig, limits::Limit, params, Connection, OpenFlags, TransactionBehavior,
};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, E>;
pub const DATABASE_NAME: &str = "shell-log.sqlite3";
pub const MAX_DATABASE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_EXPORT_BYTES: usize = 64 * 1024 * 1024;
const MAX_PAYLOAD: usize = 128 * 1024;
const ACCESS_LOCK: &str = "shell-log.access";
const APP_ID: i64 = 0x58595a53;
const SCHEMA: &str = "CREATE TABLE shell_logs (sequence INTEGER PRIMARY KEY CHECK(sequence > 0), id TEXT NOT NULL UNIQUE, session_id TEXT NOT NULL, started_ms INTEGER NOT NULL CHECK(started_ms >= 0), command TEXT NOT NULL CHECK(length(CAST(command AS BLOB)) <= 16384), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB)) <= 131072)) STRICT";

pub struct ShellLogStore {
    connection: Connection,
    directory: Arc<PrivateDirectory>,
    root: PathBuf,
    schema_cookie: i64,
}
impl ShellLogStore {
    pub fn open(root: &Path) -> Result<Self> {
        let directory = PrivateDirectory::open_or_create(root)?;
        directory.update(ACCESS_LOCK, 0, |_| Ok((Self::open_locked(root), None)))?
    }
    fn open_locked(root: &Path) -> Result<Self> {
        let directory = PrivateDirectory::open_or_create(root)?;
        check_sidecars(&directory)?;
        let created = directory.prepare_database_file(DATABASE_NAME, MAX_DATABASE_BYTES)?;
        check_files(&directory)?;
        let connection = Connection::open_with_flags(
            root.join(DATABASE_NAME),
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        let mode: String = connection.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if mode != "delete" {
            return Err(E::UnsupportedSchema);
        }
        connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, (MAX_PAYLOAD + 32768) as i32)?;
        connection.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 16384)?;
        connection.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
        connection.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA temp_store=MEMORY; PRAGMA mmap_size=0; PRAGMA secure_delete=ON; PRAGMA synchronous=EXTRA;")?;
        let mut store = Self {
            connection,
            directory: Arc::new(directory),
            root: root.to_owned(),
            schema_cookie: 0,
        };
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (version, app): (i64, i64) = (
            tx.query_row("PRAGMA user_version", [], |r| r.get(0))?,
            tx.query_row("PRAGMA application_id", [], |r| r.get(0))?,
        );
        if created && version == 0 && app == 0 {
            let count: i64 =
                tx.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
            if count != 0 {
                return Err(E::Corrupt);
            }
            tx.execute_batch(SCHEMA)?;
            tx.pragma_update(None, "application_id", APP_ID)?;
            tx.pragma_update(None, "user_version", 1)?;
        } else if version != 1 || app != APP_ID {
            return Err(E::UnsupportedSchema);
        }
        verify_schema(&tx)?;
        let quick: String = tx.query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
        if quick != "ok" {
            return Err(E::Corrupt);
        }
        let page_size: i64 = tx.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        if page_size != 4096 {
            return Err(E::UnsupportedSchema);
        }
        let pages: i64 = tx.query_row("PRAGMA max_page_count=16384", [], |r| r.get(0))?;
        if pages != 16384 {
            return Err(E::LimitReached);
        }
        let count: i64 = tx.query_row("SELECT count(*) FROM shell_logs", [], |r| r.get(0))?;
        if count > MAX_LOG_ROWS as i64 {
            return Err(E::LimitReached);
        }
        store.schema_cookie = tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
        tx.commit().map_err(commit_error)?;
        check_files(&store.directory)?;
        // Validate persisted rows without retaining the complete archive in RAM.
        let mut stmt = store
            .connection
            .prepare("SELECT sequence,id,session_id,started_ms,command,payload FROM shell_logs")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            decode(row)?;
        }
        drop(rows);
        drop(stmt);
        Ok(store)
    }
    fn access<T>(&self, operation: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        self.directory
            .update(ACCESS_LOCK, 0, |_| Ok((operation(self), None)))?
    }
    fn access_mut<T>(&mut self, operation: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let directory = self.directory.clone();
        directory.update(ACCESS_LOCK, 0, |_| Ok((operation(self), None)))?
    }
    fn guard(&self) -> Result<()> {
        check_files(&self.directory)?;
        let cookie: i64 = self
            .connection
            .query_row("PRAGMA schema_version", [], |r| r.get(0))?;
        let app: i64 = self
            .connection
            .query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version: i64 = self
            .connection
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let mode: String = self
            .connection
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if cookie != self.schema_cookie || app != APP_ID || version != 1 || mode != "delete" {
            return Err(E::UnsupportedSchema);
        }
        Ok(())
    }
    pub fn append(
        &mut self,
        entry: &ShellLogEntry,
        mut redact: impl FnMut(&str) -> String,
    ) -> Result<ShellLogEntry> {
        let mut entry = entry.clone();
        entry.sequence = "1".into();
        validate(&entry)?;
        redact_entry(&mut entry, &mut redact);
        validate(&entry)?;
        self.access_mut(|s| {
            s.guard()?;
            let tx = s.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (count, max): (i64, i64) = tx.query_row("SELECT count(*),coalesce(max(sequence),0) FROM shell_logs", [], |r| Ok((r.get(0)?,r.get(1)?)))?;
            if count >= MAX_LOG_ROWS as i64 { return Err(E::LimitReached); }
            let sequence = max.checked_add(1).ok_or(E::LimitReached)?;
            entry.sequence = sequence.to_string();
            let payload = encode(&entry)?;
            tx.execute("INSERT INTO shell_logs(sequence,id,session_id,started_ms,command,payload) VALUES(?1,?2,?3,?4,?5,?6)", params![sequence,entry.id,entry.session_id,entry.started_at_ms,entry.command,payload])?;
            tx.commit().map_err(commit_error)?;
            Ok(entry)
        })
    }
    /// Update completion metadata only. Cannot alter the command or identity.
    /// Receipt clocks can move backwards; clamping is not a measured duration.
    pub fn finish(
        &mut self,
        id: &str,
        status: &CommandStatus,
        exit_code: Option<i32>,
        finished_ms: i64,
    ) -> Result<()> {
        valid_id(id)?;
        if *status == CommandStatus::Running {
            return Err(E::InvalidRecord);
        }
        self.access_mut(|s| {
            s.guard()?;
            let tx = s.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut stmt = tx.prepare("SELECT sequence,id,session_id,started_ms,command,payload FROM shell_logs WHERE id=?1")?;
            let mut rows = stmt.query([id])?;
            let mut entry = decode(rows.next()?.ok_or(E::NotFound)?)?;
            drop(rows); drop(stmt);
            if entry.status != CommandStatus::Running { return Err(E::Conflict); }
            entry.status = status.clone(); entry.exit_code = exit_code; entry.finished_at_ms = Some(finished_ms.max(entry.started_at_ms));
            validate(&entry)?;
            tx.execute("UPDATE shell_logs SET payload=?1 WHERE id=?2", params![encode(&entry)?,id])?;
            tx.commit().map_err(commit_error)?;
            Ok(())
        })
    }
    pub fn count(&self) -> Result<usize> {
        self.access(|s| {
            s.guard()?;
            let count: i64 =
                s.connection
                    .query_row("SELECT count(*) FROM shell_logs", [], |r| r.get(0))?;
            if count > MAX_LOG_ROWS as i64 {
                return Err(E::LimitReached);
            }
            Ok(count as usize)
        })
    }
    pub fn query(
        &self,
        query: &ShellLogQuery,
        mut redact: impl FnMut(&str) -> String,
    ) -> Result<ShellLogPage> {
        validate_query(query)?;
        self.access(|s| {
            s.guard()?;
            let tx = s.connection.unchecked_transaction()?;
            let snapshot = snapshot(&tx, query)?;
            let mut entries = collect_page(
                &tx,
                query,
                snapshot,
                query.limit.unwrap_or(100) as usize + 1,
            )?;
            let limit = query.limit.unwrap_or(100) as usize;
            let more = entries.len() > limit;
            entries.truncate(limit);
            let next_before_seq = if more {
                entries.last().map(|e| e.sequence.clone())
            } else {
                None
            };
            for entry in &mut entries {
                redact_entry(entry, &mut redact);
                validate(entry)?;
            }
            Ok(ShellLogPage {
                entries,
                next_before_seq,
                snapshot_max_seq: snapshot.to_string(),
            })
        })
    }
    /// Bounded atomic export into an owner-private fixed directory. Random names
    /// plus the stable per-file lock refuse an existing destination. The same-user
    /// malicious-writer limitation of PrivateDirectory still applies.
    pub fn export(
        &self,
        query: &ShellLogQuery,
        format: ExportFormat,
        mut redact: impl FnMut(&str) -> String,
    ) -> Result<ShellLogExport> {
        validate_query(query)?;
        self.access(|s| {
            s.guard()?;
            let tx = s.connection.unchecked_transaction()?;
            let max = snapshot(&tx, query)?;
            let mut query = query.clone(); query.before_seq = None;
            let mut output = BoundedOutput(Vec::new());
            if matches!(format, ExportFormat::Json) {
                let exported_at_ms = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| E::Unavailable)?.as_millis()).map_err(|_| E::Unavailable)?;
                write!(output, "{{\"schema_version\":1,\"exported_at_ms\":{exported_at_ms},\"snapshot_max_seq\":\"{max}\",\"entries\":[").map_err(|_| E::LimitReached)?;
            }
            else { output.write_all(b"id,sequence,session_id,generation,shell_id,shell_sequence,host,port,username,started_at_ms,finished_at_ms,remote_started_at,command,cwd,status,exit_code,redacted\r\n").map_err(|_| E::LimitReached)?; }
            let mut count = 0;
            loop {
                let entries = collect_page(&tx, &query, max, 200)?;
                if entries.is_empty() { break; }
                query.before_seq = entries.last().map(|e| e.sequence.clone());
                for mut entry in entries {
                    count += 1; if count > MAX_LOG_ROWS { return Err(E::LimitReached); }
                    redact_entry(&mut entry, &mut redact); validate(&entry)?;
                    match format {
                        ExportFormat::Json => {
                            if count > 1 { output.write_all(b",").map_err(|_| E::LimitReached)?; }
                            serde_json::to_writer(&mut output, &entry).map_err(|_| E::LimitReached)?;
                        }
                        ExportFormat::Csv => write_csv(&mut output, &entry)?,
                    }
                }
            }
            if matches!(format, ExportFormat::Json) { output.write_all(b"]}\n").map_err(|_| E::LimitReached)?; }
            let directory = s.directory.child("shell-log-exports")?;
            let filename = format!("shell-log-{}.{}", uuid::Uuid::new_v4(), if matches!(format, ExportFormat::Json) { "json" } else { "csv" });
            let bytes = output.0.len();
            let path = s.root.join("shell-log-exports").join(&filename).to_str().ok_or(E::InvalidPath)?.to_owned();
            directory.update(&filename, MAX_EXPORT_BYTES, |existing| {
                if existing.is_some() { return Err(StorageError::Conflict); }
                Ok(((),Some(Zeroizing::new(output.0))))
            })?;
            Ok(ShellLogExport { path, records: count, bytes, format })
        })
    }
}
fn check_sidecars(directory: &PrivateDirectory) -> Result<()> {
    for name in ["shell-log.sqlite3-wal", "shell-log.sqlite3-shm"] {
        if directory.validate_database_file(name, MAX_DATABASE_BYTES + 1024 * 1024)? {
            return Err(E::UnsafeFile);
        }
    }
    directory.validate_database_file(
        "shell-log.sqlite3-journal",
        MAX_DATABASE_BYTES + 1024 * 1024,
    )?;
    Ok(())
}
fn check_files(directory: &PrivateDirectory) -> Result<()> {
    if !directory.validate_database_file(DATABASE_NAME, MAX_DATABASE_BYTES)? {
        return Err(E::Corrupt);
    }
    check_sidecars(directory)
}
fn verify_schema(connection: &Connection) -> Result<()> {
    let mut stmt = connection.prepare("SELECT type,name,sql FROM sqlite_schema ORDER BY name")?;
    let actual: Vec<(String, String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let expected = vec![
        ("table".into(), "shell_logs".into(), Some(SCHEMA.into())),
        ("index".into(), "sqlite_autoindex_shell_logs_1".into(), None),
    ];
    if actual != expected {
        return Err(E::UnsupportedSchema);
    }
    Ok(())
}
fn commit_error(error: rusqlite::Error) -> E {
    match E::from(error) {
        E::Busy => E::Busy,
        _ => E::SaveUncertain,
    }
}
fn valid_id(value: &str) -> Result<()> {
    if value.len() == 36
        && uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
    {
        Ok(())
    } else {
        Err(E::InvalidRecord)
    }
}
fn number(value: &str, allow_zero: bool) -> Result<i64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(E::InvalidRecord);
    }
    let value: i64 = value.parse().map_err(|_| E::InvalidRecord)?;
    if value < if allow_zero { 0 } else { 1 } {
        Err(E::InvalidRecord)
    } else {
        Ok(value)
    }
}
fn validate(e: &ShellLogEntry) -> Result<()> {
    valid_id(&e.id)?;
    valid_id(&e.session_id)?;
    // shell_id is a bounded integration nonce, not necessarily a UUID.
    if e.shell_id.is_empty()
        || e.shell_id.len() > 128
        || !e
            .shell_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err(E::InvalidRecord);
    }
    number(&e.sequence, false)?;
    number(&e.generation, true)?;
    number(&e.shell_sequence, false)?;
    if e.command.is_empty()
        || e.command.len() > MAX_COMMAND_BYTES
        || e.cwd.len() > MAX_CWD_BYTES
        || e.host.len() > 1024
        || e.username.len() > 1024
        || e.port == 0
        || e.started_at_ms < 0
        || e.finished_at_ms.is_some_and(|n| n < 0)
        || e.remote_started_at.as_ref().is_some_and(|s| s.len() > 128)
        || e.command.contains('\0')
        || e.cwd.contains('\0')
    {
        return Err(E::InvalidRecord);
    }
    if e.status == CommandStatus::Running && (e.finished_at_ms.is_some() || e.exit_code.is_some()) {
        return Err(E::InvalidRecord);
    }
    Ok(())
}
fn redact_entry(e: &mut ShellLogEntry, redact: &mut impl FnMut(&str) -> String) {
    for field in [&mut e.command, &mut e.cwd, &mut e.host, &mut e.username] {
        let filtered = redact(field);
        e.redacted |= *field != filtered;
        *field = filtered;
    }
    if let Some(value) = &mut e.remote_started_at {
        let filtered = redact(value);
        e.redacted |= *value != filtered;
        *value = filtered;
    }
}
fn encode(e: &ShellLogEntry) -> Result<String> {
    let encoded = serde_json::to_string(e).map_err(|_| E::InvalidRecord)?;
    if encoded.len() > MAX_PAYLOAD {
        return Err(E::LimitReached);
    }
    Ok(encoded)
}
fn decode(row: &rusqlite::Row<'_>) -> Result<ShellLogEntry> {
    let raw: String = row.get(5)?;
    if raw.len() > MAX_PAYLOAD {
        return Err(E::Corrupt);
    }
    let e: ShellLogEntry = serde_json::from_str(&raw).map_err(|_| E::Corrupt)?;
    validate(&e).map_err(|_| E::Corrupt)?;
    if number(&e.sequence, false)? != row.get::<_, i64>(0)?
        || e.id != row.get::<_, String>(1)?
        || e.session_id != row.get::<_, String>(2)?
        || e.started_at_ms != row.get::<_, i64>(3)?
        || e.command != row.get::<_, String>(4)?
    {
        return Err(E::Corrupt);
    }
    Ok(e)
}
fn validate_query(q: &ShellLogQuery) -> Result<()> {
    if let Some(id) = &q.session_id {
        valid_id(id)?;
    }
    if let Some(n) = &q.before_seq {
        number(n, false)?;
    }
    if let Some(n) = &q.snapshot_max_seq {
        number(n, true)?;
    }
    if q.limit.is_some_and(|v| v == 0 || v > 200)
        || q.contains.as_ref().is_some_and(|v| v.len() > 256)
        || q.from_ms.is_some_and(|v| v < 0)
        || q.to_ms.is_some_and(|v| v < 0)
        || matches!((q.from_ms,q.to_ms),(Some(a),Some(b)) if a>b)
    {
        return Err(E::InvalidRecord);
    }
    Ok(())
}
fn snapshot(c: &Connection, q: &ShellLogQuery) -> Result<i64> {
    match &q.snapshot_max_seq {
        Some(s) => number(s, true),
        None => Ok(c.query_row(
            "SELECT coalesce(max(sequence),0) FROM shell_logs",
            [],
            |r| r.get(0),
        )?),
    }
}
fn collect_page(
    c: &Connection,
    q: &ShellLogQuery,
    max: i64,
    limit: usize,
) -> Result<Vec<ShellLogEntry>> {
    let before = q
        .before_seq
        .as_deref()
        .map(|v| number(v, false))
        .transpose()?;
    let mut stmt = c.prepare("SELECT sequence,id,session_id,started_ms,command,payload FROM shell_logs WHERE sequence<=?1 AND (?2 IS NULL OR sequence<?2) AND (?3 IS NULL OR session_id=?3) AND (?4 IS NULL OR started_ms>=?4) AND (?5 IS NULL OR started_ms<=?5) AND (?6 IS NULL OR instr(command,?6)>0) ORDER BY sequence DESC LIMIT ?7")?;
    let mut rows = stmt.query(params![
        max,
        before,
        q.session_id,
        q.from_ms,
        q.to_ms,
        q.contains,
        limit as i64
    ])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        result.push(decode(row)?);
    }
    Ok(result)
}
struct BoundedOutput(Vec<u8>);
impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_EXPORT_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("export limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn write_csv(output: &mut BoundedOutput, e: &ShellLogEntry) -> Result<()> {
    let fields = vec![
        e.id.clone(),
        e.sequence.clone(),
        e.session_id.clone(),
        e.generation.clone(),
        e.shell_id.clone(),
        e.shell_sequence.clone(),
        e.host.clone(),
        e.port.to_string(),
        e.username.clone(),
        e.started_at_ms.to_string(),
        e.finished_at_ms.map(|n| n.to_string()).unwrap_or_default(),
        e.remote_started_at.clone().unwrap_or_default(),
        e.command.clone(),
        e.cwd.clone(),
        match e.status {
            CommandStatus::Running => "running",
            CommandStatus::Completed => "completed",
            CommandStatus::Unknown => "unknown",
        }
        .into(),
        e.exit_code.map(|n| n.to_string()).unwrap_or_default(),
        e.redacted.to_string(),
    ];
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            output.write_all(b",").map_err(|_| E::LimitReached)?;
        }
        output.write_all(b"\"").map_err(|_| E::LimitReached)?;
        let trimmed = field.trim_start_matches(|c: char| c.is_whitespace() || c.is_control());
        if trimmed.starts_with(['=', '+', '-', '@']) || field.starts_with(['\t', '\r', '\n']) {
            output.write_all(b"'").map_err(|_| E::LimitReached)?;
        }
        output
            .write_all(field.replace('"', "\"\"").as_bytes())
            .map_err(|_| E::LimitReached)?;
        output.write_all(b"\"").map_err(|_| E::LimitReached)?;
    }
    output.write_all(b"\r\n").map_err(|_| E::LimitReached)
}
#[cfg(test)]
#[path = "shell_log_store_tests.rs"]
mod tests;
