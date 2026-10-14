use super::*;
use std::{fs, path::PathBuf};

struct Fixture {
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        // Only synthetic fixtures in newly created private directories.
        let path = std::env::temp_dir().join(format!("xyz-history-test-{}", uuid::Uuid::new_v4()));
        PrivateDirectory::open_or_create(&path).unwrap();
        Self { path }
    }
    fn open(&self) -> HistoryStore {
        HistoryStore::open(&self.path, HistoryPreference::SaveRedactedText).unwrap()
    }
    fn database(&self) -> PathBuf {
        self.path.join(DATABASE_NAME)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn snapshot() -> ChatSnapshot {
    ChatSnapshot {
        chat_id: id(),
        task_id: id(),
        title: "Synthetic task".into(),
        messages: vec![
            ChatMessage {
                id: id(),
                role: MessageRole::User,
                text: "Synthetic question".into(),
                run_id: id(),
                state: MessageState::Complete,
                error: None,
                destination: Some("https://synthetic.example.invalid".into()),
                model: Some("synthetic-model".into()),
            },
            ChatMessage {
                id: id(),
                role: MessageRole::Assistant,
                text: "Partial synthetic response".into(),
                run_id: id(),
                state: MessageState::Streaming,
                error: Some(ApiError::new(
                    "SYNTHETIC_RAW_ERROR",
                    "synthetic-credential-in-error",
                )),
                destination: Some("https://synthetic.example.invalid".into()),
                model: Some("synthetic-model".into()),
            },
        ],
        active_run_id: Some(id()),
        revision: "57".into(),
    }
}
fn prepared(snapshot: &ChatSnapshot) -> RedactedChat {
    RedactedChat::from_snapshot(snapshot, |text| {
        text.replace("synthetic-known-secret", "[redacted]")
    })
    .unwrap()
}
fn error<T>(value: Result<T>) -> HistoryError {
    value.err().expect("expected error")
}

#[test]
fn memory_only_is_default_and_touches_no_files() {
    assert_eq!(HistoryPreference::default(), HistoryPreference::MemoryOnly);
    let path = std::env::temp_dir().join(format!("xyz-disabled-{}", id()));
    assert_eq!(
        error(HistoryStore::open(&path, HistoryPreference::default())),
        HistoryError::Disabled
    );
    assert!(!path.exists());
}
#[test]
fn stable_ids_restore_only_inert_history_and_interrupted_messages() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let source = snapshot();
    store.create(&prepared(&source)).unwrap();
    drop(store);
    let store = fixture.open();
    let saved = store.read(&source.chat_id).unwrap();
    assert_eq!(saved.chat.chat_id, source.chat_id);
    assert_eq!(saved.chat.task_id, source.task_id);
    assert_eq!(saved.chat.messages[0].id, source.messages[0].id);
    assert_eq!(
        saved.chat.messages[1].state,
        ArchivedMessageState::Interrupted
    );
    let restored = saved.to_snapshot();
    assert!(restored.active_run_id.is_none());
    assert_eq!(restored.messages[1].state, MessageState::Cancelled);
    assert_eq!(
        restored.messages[1].error.as_ref().unwrap().code,
        "HISTORY_INTERRUPTED"
    );
    assert!(restored
        .messages
        .iter()
        .all(|m| m.run_id.is_empty() && m.destination.is_none() && m.model.is_none()));
    let data = fs::read(fixture.database()).unwrap();
    for forbidden in [
        &source.messages[0].run_id,
        &source.messages[1].run_id,
        source.active_run_id.as_ref().unwrap(),
    ] {
        assert!(!data
            .windows(forbidden.len())
            .any(|part| part == forbidden.as_bytes()));
    }
}
#[test]
fn mandatory_redactor_runs_before_any_text_hits_sqlite() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let mut source = snapshot();
    source.title = "Title synthetic-known-secret".into();
    source.messages[0].text = "command --token synthetic-known-secret".into();
    source.messages[1].text = "echo synthetic-known-secret".into();
    let mut fields = 0;
    let redacted = RedactedChat::from_snapshot(&source, |text| {
        fields += 1;
        text.replace("synthetic-known-secret", "[redacted]")
    })
    .unwrap();
    assert_eq!(fields, 3);
    store.create(&redacted).unwrap();
    drop(store);
    let bytes = fs::read(fixture.database()).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for forbidden in [
        "synthetic-known-secret",
        "synthetic-credential-in-error",
        "synthetic.example.invalid",
        "synthetic-model",
        "active_run_id",
        "run_id",
        "approval_token",
        "keyring_ref",
    ] {
        // run_id exists as the audit schema's column name, but never in payload.
        if forbidden != "run_id" {
            assert!(!text.contains(forbidden), "leaked {forbidden}");
        }
    }
    assert!(text.contains("[redacted]"));
    assert!(text.contains("command --token [redacted]"));
    let raw = fixture
        .open()
        .connection
        .query_row("SELECT payload FROM chats", [], |r| r.get::<_, String>(0))
        .unwrap();
    assert!(!raw.contains("run_id"));
}
#[test]
fn revoked_consent_blocks_reads_writes_and_audit_without_deleting_history() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let source = snapshot();
    let saved = store.create(&prepared(&source)).unwrap();
    store.set_preference(HistoryPreference::MemoryOnly);
    assert_eq!(error(store.read(&source.chat_id)), HistoryError::Disabled);
    assert_eq!(
        error(store.create(&prepared(&snapshot()))),
        HistoryError::Disabled
    );
    assert_eq!(
        error(store.forget(&source.chat_id, 1)),
        HistoryError::Disabled
    );
    assert_eq!(
        error(store.append_audit(&source.chat_id, 1, None, AuditKind::RunStarted)),
        HistoryError::Disabled
    );
    assert_eq!(
        fixture.open().read(&source.chat_id).unwrap().revision,
        saved.revision
    );
}
#[test]
fn two_instances_cas_conflicts_never_overwrite_newer_text_or_stable_task_id() {
    let fixture = Fixture::new();
    let mut first = fixture.open();
    let mut second = fixture.open();
    let mut source = snapshot();
    first.create(&prepared(&source)).unwrap();
    source.title = "Newer title".into();
    let newer = second.replace(&prepared(&source), 1).unwrap();
    assert_eq!(newer.revision, 2);
    source.title = "Stale title".into();
    assert_eq!(
        error(first.replace(&prepared(&source), 1)),
        HistoryError::Conflict
    );
    assert_eq!(
        error(first.forget(&source.chat_id, 1)),
        HistoryError::Conflict
    );
    assert_eq!(
        error(first.set_listed(&source.chat_id, 1, false)),
        HistoryError::Conflict
    );
    source.task_id = id();
    assert_eq!(
        error(first.replace(&prepared(&source), 2)),
        HistoryError::Conflict
    );
    assert_eq!(
        first.read(&source.chat_id).unwrap().chat.title,
        "Newer title"
    );
}
#[test]
fn delist_is_reversible_and_forget_removes_related_audit_atomically() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let source = snapshot();
    store.create(&prepared(&source)).unwrap();
    let run = id();
    store
        .append_audit(&source.chat_id, 1, Some(&run), AuditKind::ApprovalGranted)
        .unwrap();
    let revision = store.set_listed(&source.chat_id, 1, false).unwrap();
    assert!(store.list(false).unwrap().is_empty());
    assert_eq!(store.list(true).unwrap().len(), 1);
    assert!(!store.read(&source.chat_id).unwrap().listed);
    let revision = store.set_listed(&source.chat_id, revision, true).unwrap();
    assert_eq!(store.list(false).unwrap().len(), 1);
    assert_eq!(
        store.read_audit(&source.chat_id).unwrap()[0].kind,
        AuditKind::ApprovalGranted
    );
    store.forget(&source.chat_id, revision).unwrap();
    assert_eq!(error(store.read(&source.chat_id)), HistoryError::NotFound);
    assert!(store.read_audit(&source.chat_id).unwrap().is_empty());
    assert!(store
        .connection
        .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, bool>(0))
        .unwrap());
}
#[test]
fn audit_has_no_free_text_and_cannot_recreate_authority() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let source = snapshot();
    store.create(&prepared(&source)).unwrap();
    let event = store
        .append_audit(&source.chat_id, 1, Some(&id()), AuditKind::ApprovalGranted)
        .unwrap();
    assert!(require_id(&event.event_id).is_ok());
    assert_eq!(
        error(store.append_audit(
            &source.chat_id,
            1,
            Some("synthetic-secret-command"),
            AuditKind::RunStarted
        )),
        HistoryError::InvalidRecord
    );
    assert_eq!(
        error(store.append_audit(&source.chat_id, 2, None, AuditKind::RunStarted)),
        HistoryError::Conflict
    );
    let restored = store.read(&source.chat_id).unwrap().to_snapshot();
    assert!(restored.active_run_id.is_none());
    assert!(restored.messages.iter().all(|m| m.run_id.is_empty()));
}
#[test]
fn duplicate_chat_task_message_and_invalid_ids_are_rejected() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let mut source = snapshot();
    store.create(&prepared(&source)).unwrap();
    assert_eq!(
        error(store.create(&prepared(&source))),
        HistoryError::Conflict
    );
    source.chat_id = id();
    assert_eq!(
        error(store.create(&prepared(&source))),
        HistoryError::Conflict
    );
    source.messages[1].id = source.messages[0].id.clone();
    assert_eq!(
        error(RedactedChat::from_snapshot(&source, str::to_owned)),
        HistoryError::InvalidRecord
    );
    assert_eq!(
        error(store.read("../../config")),
        HistoryError::InvalidRecord
    );
    assert_eq!(
        error(store.read(&uuid::Uuid::nil().to_string())),
        HistoryError::InvalidRecord
    );
}
#[test]
fn text_message_count_and_utf8_byte_bounds_are_enforced_after_redaction() {
    let mut source = snapshot();
    source.messages[0].text = "x".repeat(MAX_MESSAGE_BYTES + 1);
    assert_eq!(
        error(RedactedChat::from_snapshot(&source, str::to_owned)),
        HistoryError::InvalidRecord
    );
    source = snapshot();
    assert_eq!(
        error(RedactedChat::from_snapshot(&source, |_| "x".repeat(MAX_ASSISTANT_BYTES + 1))),
        HistoryError::InvalidRecord
    );
    source.title = "é".repeat(129);
    assert_eq!(
        error(RedactedChat::from_snapshot(&source, str::to_owned)),
        HistoryError::InvalidRecord
    );
    source = snapshot();
    source.messages = vec![source.messages[0].clone(); MAX_MESSAGES + 1];
    assert_eq!(
        error(RedactedChat::from_snapshot(&source, str::to_owned)),
        HistoryError::InvalidRecord
    );
    source = snapshot();
    source.messages[0].text = "nul\0text".into();
    assert_eq!(
        error(RedactedChat::from_snapshot(&source, str::to_owned)),
        HistoryError::InvalidRecord
    );
}
#[test]
fn chat_cap_includes_delisted_records_and_never_evicts_history() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let first = store.create(&prepared(&snapshot())).unwrap();
    store.set_listed(&first.chat.chat_id, 1, false).unwrap();
    for _ in 1..MAX_CHATS {
        store.create(&prepared(&snapshot())).unwrap();
    }
    assert_eq!(
        error(store.create(&prepared(&snapshot()))),
        HistoryError::LimitReached
    );
    assert_eq!(store.list(true).unwrap().len(), MAX_CHATS);
    assert_eq!(store.list(false).unwrap().len(), MAX_CHATS - 1);
}
#[test]
fn audit_cap_fails_visibly_instead_of_eviction() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let source = snapshot();
    store.create(&prepared(&source)).unwrap();
    {
        let tx = store.connection.transaction().unwrap();
        for _ in 0..MAX_AUDIT_EVENTS {
            tx.execute(
                "INSERT INTO audit(event_id,chat_id,run_id,kind,created_ms) VALUES(?1,?2,NULL,0,0)",
                params![id(), source.chat_id],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    assert_eq!(
        error(store.append_audit(&source.chat_id, 1, None, AuditKind::RunStarted)),
        HistoryError::LimitReached
    );
    assert_eq!(
        store.read_audit(&source.chat_id).unwrap().len(),
        MAX_AUDIT_EVENTS
    );
}
#[test]
fn unknown_schema_and_corruption_fail_without_resetting_bytes() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    store.create(&prepared(&snapshot())).unwrap();
    store
        .connection
        .pragma_update(None, "user_version", 99)
        .unwrap();
    drop(store);
    let before = fs::read(fixture.database()).unwrap();
    assert_eq!(
        error(HistoryStore::open(
            &fixture.path,
            HistoryPreference::SaveRedactedText
        )),
        HistoryError::UnsupportedSchema
    );
    assert_eq!(fs::read(fixture.database()).unwrap(), before);
    fs::write(
        fixture.database(),
        b"synthetic-corrupt-history-do-not-erase",
    )
    .unwrap();
    let before = fs::read(fixture.database()).unwrap();
    assert_eq!(
        error(HistoryStore::open(
            &fixture.path,
            HistoryPreference::SaveRedactedText
        )),
        HistoryError::Corrupt
    );
    assert_eq!(fs::read(fixture.database()).unwrap(), before);
}
#[test]
fn zero_length_existing_file_is_not_assumed_to_be_new() {
    let fixture = Fixture::new();
    let directory = PrivateDirectory::open_or_create(&fixture.path).unwrap();
    assert!(directory
        .prepare_database_file(DATABASE_NAME, MAX_DATABASE_BYTES)
        .unwrap());
    assert_eq!(
        error(HistoryStore::open(
            &fixture.path,
            HistoryPreference::SaveRedactedText
        )),
        HistoryError::Corrupt
    );
    assert!(fs::read(fixture.database()).unwrap().is_empty());
}
#[test]
fn malformed_payload_unknown_fields_and_schema_injection_are_not_silently_skipped() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    store.create(&prepared(&snapshot())).unwrap();
    store
        .connection
        .execute("UPDATE chats SET payload='{}'", [])
        .unwrap();
    assert_eq!(error(store.list(true)), HistoryError::Corrupt);
    drop(store);
    assert_eq!(
        error(HistoryStore::open(
            &fixture.path,
            HistoryPreference::SaveRedactedText
        )),
        HistoryError::Corrupt
    );
    let fixture = Fixture::new();
    let mut store = fixture.open();
    store.create(&prepared(&snapshot())).unwrap();
    store
        .connection
        .execute_batch("CREATE TRIGGER malicious AFTER DELETE ON chats BEGIN SELECT 1; END")
        .unwrap();
    assert_eq!(error(store.list(true)), HistoryError::UnsupportedSchema);
    drop(store);
    assert_eq!(
        error(HistoryStore::open(
            &fixture.path,
            HistoryPreference::SaveRedactedText
        )),
        HistoryError::UnsupportedSchema
    );
}
#[test]
fn payload_cannot_inject_restored_runtime_fields() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let source = snapshot();
    store.create(&prepared(&source)).unwrap();
    let mut data = serde_json::to_value(&prepared(&source).0).unwrap();
    data["active_run_id"] = serde_json::json!(id());
    store
        .connection
        .execute("UPDATE chats SET payload=?1", [data.to_string()])
        .unwrap();
    assert_eq!(error(store.read(&source.chat_id)), HistoryError::Corrupt);
}
#[test]
fn rollback_preserves_previous_transcript_and_busy_is_bounded() {
    let fixture = Fixture::new();
    let mut first = fixture.open();
    let mut second = fixture.open();
    let source = snapshot();
    first.create(&prepared(&source)).unwrap();
    second
        .connection
        .busy_timeout(Duration::from_millis(30))
        .unwrap();
    let tx = first
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    tx.execute("UPDATE chats SET payload='{}'", []).unwrap();
    let start = std::time::Instant::now();
    assert_eq!(
        error(second.replace(&prepared(&source), 1)),
        HistoryError::Busy
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    drop(tx);
    assert_eq!(
        second.read(&source.chat_id).unwrap().chat.title,
        source.title
    );
}
#[test]
fn sqlite_limits_private_files_and_delete_journal_are_configured() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let pragma = |name: &str| {
        store
            .connection
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(
        pragma("max_page_count"),
        (MAX_DATABASE_BYTES / PAGE_SIZE) as i64
    );
    assert_eq!(pragma("secure_delete"), 1);
    assert_eq!(pragma("temp_store"), 2);
    assert_eq!(pragma("synchronous"), 3);
    assert_eq!(pragma("trusted_schema"), 0);
    assert_eq!(
        store
            .connection
            .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
    assert_eq!(
        store
            .connection
            .limit(Limit::SQLITE_LIMIT_ATTACHED)
            .unwrap(),
        0
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&fixture.path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(fixture.database())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn wal_and_shm_are_refused_not_deleted() {
    for suffix in ["wal", "shm"] {
        let fixture = Fixture::new();
        drop(fixture.open());
        let directory = PrivateDirectory::open_or_create(&fixture.path).unwrap();
        let name = format!("{DATABASE_NAME}-{suffix}");
        directory
            .update(&name, MAX_JOURNAL_BYTES, |_| {
                Ok(((), Some(zeroize::Zeroizing::new(Vec::new()))))
            })
            .unwrap();
        assert_eq!(
            error(HistoryStore::open(
                &fixture.path,
                HistoryPreference::SaveRedactedText
            )),
            HistoryError::UnsafeFile
        );
        assert!(fixture.path.join(name).exists());
    }
}
#[cfg(unix)]
#[test]
fn symlink_hardlink_permissive_files_and_oversized_database_fail_closed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let target = outside.path.join("synthetic-do-not-touch");
    fs::write(&target, b"synthetic-protected-content").unwrap();
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let linked = fixture.path.join(format!("{DATABASE_NAME}{suffix}"));
        symlink(&target, &linked).unwrap();
        assert!(HistoryStore::open(&fixture.path, HistoryPreference::SaveRedactedText).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"synthetic-protected-content");
        fs::remove_file(linked).unwrap();
    }
    drop(fixture.open());
    let alias = outside.path.join("hardlinked-database");
    fs::hard_link(fixture.database(), &alias).unwrap();
    assert_eq!(
        error(HistoryStore::open(
            &fixture.path,
            HistoryPreference::SaveRedactedText
        )),
        HistoryError::UnsafeFile
    );
    fs::remove_file(alias).unwrap();
    fs::set_permissions(fixture.database(), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        error(HistoryStore::open(
            &fixture.path,
            HistoryPreference::SaveRedactedText
        )),
        HistoryError::UnsafeFile
    );
    fs::set_permissions(fixture.database(), fs::Permissions::from_mode(0o600)).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(fixture.database())
        .unwrap()
        .set_len(MAX_DATABASE_BYTES as u64 + 1)
        .unwrap();
    assert_eq!(
        error(HistoryStore::open(
            &fixture.path,
            HistoryPreference::SaveRedactedText
        )),
        HistoryError::LimitReached
    );
}
#[cfg(unix)]
#[test]
fn permissive_directory_and_symlinked_ancestor_are_refused() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        error(HistoryStore::open(
            &fixture.path,
            HistoryPreference::SaveRedactedText
        )),
        HistoryError::UnsafeFile
    );
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o700)).unwrap();
    let alias = std::env::temp_dir().join(format!("xyz-history-alias-{}", id()));
    symlink(&fixture.path, &alias).unwrap();
    assert!(HistoryStore::open(&alias.join("child"), HistoryPreference::SaveRedactedText).is_err());
    fs::remove_file(alias).unwrap();
}

#[test]
fn concurrent_instances_allow_one_revision_winner() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let source = snapshot();
    store.create(&prepared(&source)).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = ["Concurrent one", "Concurrent two"]
            .into_iter()
            .map(|title| {
                let path = fixture.path.clone();
                let barrier = barrier.clone();
                let mut source = source.clone();
                scope.spawn(move || {
                    let mut store =
                        HistoryStore::open(&path, HistoryPreference::SaveRedactedText).unwrap();
                    source.title = title.into();
                    barrier.wait();
                    store.replace(&prepared(&source), 1).map(|row| row.revision)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|value| value == &&Ok(2)).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|value| value == &&Err(HistoryError::Conflict))
            .count(),
        1
    );
    assert_eq!(store.read(&source.chat_id).unwrap().revision, 2);
}

// Runs only as the synthetic child of crash_rollback_journal_recovers_locally.
#[test]
fn synthetic_crash_writer_process() {
    let Some(path) = std::env::var_os("XYZ_SYNTHETIC_HISTORY_CRASH_DIR") else {
        return;
    };
    let path = PathBuf::from(path);
    assert!(path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("xyz-history-test-"));
    let store = HistoryStore::open(&path, HistoryPreference::SaveRedactedText).unwrap();
    store
        .connection
        .execute_batch("PRAGMA cache_size=1; BEGIN IMMEDIATE; UPDATE chats SET payload='{}';")
        .unwrap();
    // Simulate process termination: no transaction/connection destructors run.
    std::process::exit(0);
}

#[test]
fn crash_rollback_journal_recovers_locally_without_replaying_runs() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let mut originals = Vec::new();
    for _ in 0..12 {
        let mut source = snapshot();
        source.messages[0].text = "s".repeat(MAX_MESSAGE_BYTES);
        store.create(&prepared(&source)).unwrap();
        originals.push(source);
    }
    drop(store);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "persistence::tests::synthetic_crash_writer_process",
            "--nocapture",
        ])
        .env("XYZ_SYNTHETIC_HISTORY_CRASH_DIR", &fixture.path)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(fixture.path.join("history.sqlite3-journal").exists());
    let store = fixture.open();
    assert_eq!(store.list(true).unwrap().len(), originals.len());
    for source in originals {
        let restored = store.read(&source.chat_id).unwrap().to_snapshot();
        assert_eq!(restored.messages[0].text, source.messages[0].text);
        assert!(restored.active_run_id.is_none());
    }
    assert!(!fixture.path.join("history.sqlite3-journal").exists());
}

#[test]
fn forgetting_and_recreating_ids_never_reuses_a_revision() {
    let fixture = Fixture::new();
    let mut old = fixture.open();
    let mut other = fixture.open();
    let source = snapshot();
    let original = old.create(&prepared(&source)).unwrap();
    other.forget(&source.chat_id, original.revision).unwrap();
    let mut newer = source.clone();
    newer.title = "New generation of the same stable IDs".into();
    let replacement = other.create(&prepared(&newer)).unwrap();
    assert!(replacement.revision > original.revision);
    assert_eq!(
        error(old.replace(&prepared(&source), original.revision)),
        HistoryError::Conflict
    );
    assert_eq!(
        error(old.set_listed(&source.chat_id, original.revision, false)),
        HistoryError::Conflict
    );
    assert_eq!(
        error(old.forget(&source.chat_id, original.revision)),
        HistoryError::Conflict
    );
    assert_eq!(
        error(old.append_audit(
            &source.chat_id,
            original.revision,
            None,
            AuditKind::RunStarted
        )),
        HistoryError::Conflict
    );
    assert_eq!(other.read(&source.chat_id).unwrap().chat.title, newer.title);
    assert!(other.read_audit(&source.chat_id).unwrap().is_empty());
}

#[test]
fn schema_is_rechecked_after_waiting_for_another_writer() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WAITING: AtomicBool = AtomicBool::new(false);
    fn busy(count: i32) -> bool {
        WAITING.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1));
        count < 2000
    }
    WAITING.store(false, Ordering::SeqCst);
    let fixture = Fixture::new();
    let mut first = fixture.open();
    let mut second = fixture.open();
    let source = snapshot();
    first.create(&prepared(&source)).unwrap();
    second.connection.busy_handler(Some(busy)).unwrap();
    first.connection.execute_batch("BEGIN IMMEDIATE; PRAGMA user_version=2; CREATE TRIGGER future_schema AFTER UPDATE ON chats BEGIN SELECT 1; END;").unwrap();
    let source_for_thread = source.clone();
    let handle = std::thread::spawn(move || second.replace(&prepared(&source_for_thread), 1));
    let start = std::time::Instant::now();
    while !WAITING.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(1) {
        std::thread::yield_now();
    }
    assert!(WAITING.load(Ordering::SeqCst));
    first.connection.execute_batch("COMMIT").unwrap();
    assert_eq!(
        error(handle.join().unwrap()),
        HistoryError::UnsupportedSchema
    );
    let revision: i64 = first
        .connection
        .query_row("SELECT revision FROM chats", [], |r| r.get(0))
        .unwrap();
    assert_eq!(revision, 1);
}

#[test]
fn corruption_after_open_cannot_be_overwritten_delisted_audited_or_forgotten() {
    let fixture = Fixture::new();
    let mut first = fixture.open();
    let second = fixture.open();
    let source = snapshot();
    first.create(&prepared(&source)).unwrap();
    second
        .connection
        .execute("UPDATE chats SET payload='{}'", [])
        .unwrap();
    assert_eq!(
        error(first.replace(&prepared(&source), 1)),
        HistoryError::Corrupt
    );
    assert_eq!(
        error(first.set_listed(&source.chat_id, 1, false)),
        HistoryError::Corrupt
    );
    assert_eq!(
        error(first.append_audit(&source.chat_id, 1, None, AuditKind::RunStarted)),
        HistoryError::Corrupt
    );
    assert_eq!(
        error(first.forget(&source.chat_id, 1)),
        HistoryError::Corrupt
    );
    let raw: (String, i64, i64) = second
        .connection
        .query_row("SELECT payload,revision,listed FROM chats", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(raw, ("{}".into(), 1, 1));
    assert_eq!(
        second
            .connection
            .query_row("SELECT count(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn forgetting_a_chat_preserves_corrupt_audit_for_explicit_recovery() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let source = snapshot();
    store.create(&prepared(&source)).unwrap();
    store
        .append_audit(&source.chat_id, 1, None, AuditKind::RemoteOutcomeUnknown)
        .unwrap();
    store
        .connection
        .execute("UPDATE audit SET run_id='invalid-synthetic-run'", [])
        .unwrap();
    assert_eq!(
        error(store.forget(&source.chat_id, 1)),
        HistoryError::Corrupt
    );
    assert_eq!(store.read(&source.chat_id).unwrap().revision, 1);
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn page_limit_exhaustion_rolls_back_replacement_without_evicting_old_text() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let mut source = snapshot();
    store.create(&prepared(&source)).unwrap();
    let pages: i64 = store
        .connection
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    store
        .connection
        .pragma_update(None, "max_page_count", pages)
        .unwrap();
    source.messages[1].text = "large synthetic replacement ".repeat(2200);
    assert!(source.messages[1].text.len() < MAX_ASSISTANT_BYTES);
    assert_eq!(
        error(store.replace(&prepared(&source), 1)),
        HistoryError::LimitReached
    );
    let saved = store.read(&source.chat_id).unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(saved.chat.messages[1].text, "Partial synthetic response");
    assert_eq!(
        store
            .connection
            .query_row("SELECT revision FROM metadata", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn sqlite_errors_and_uncertain_commit_never_expose_raw_diagnostics() {
    let io_error = || {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
            Some("synthetic-secret /private/synthetic/path".into()),
        )
    };
    assert_eq!(HistoryError::from(io_error()), HistoryError::Unavailable);
    assert_eq!(commit_error(io_error()), HistoryError::SaveUncertain);
    assert!(!commit_error(io_error()).to_string().contains("synthetic"));
    assert_eq!(
        commit_error(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            None
        )),
        HistoryError::Busy
    );
}

#[test]
fn simultaneous_first_open_initializes_once_without_false_corruption() {
    let fixture = Fixture::new();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let path = fixture.path.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    let store =
                        HistoryStore::open(&path, HistoryPreference::SaveRedactedText).unwrap();
                    assert!(store.list(true).unwrap().is_empty());
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    });
    let mut store = fixture.open();
    assert_eq!(store.create(&prepared(&snapshot())).unwrap().revision, 1);
}

#[test]
fn late_counter_corruption_cannot_erase_revision_history() {
    let fixture = Fixture::new();
    let mut first = fixture.open();
    let second = fixture.open();
    let source = snapshot();
    first.create(&prepared(&source)).unwrap();
    second
        .connection
        .execute("UPDATE metadata SET revision=0", [])
        .unwrap();
    assert_eq!(
        error(first.forget(&source.chat_id, 1)),
        HistoryError::Corrupt
    );
    assert_eq!(
        error(first.append_audit(&source.chat_id, 1, None, AuditKind::RunStarted)),
        HistoryError::Corrupt
    );
    assert_eq!(
        error(first.create(&prepared(&snapshot()))),
        HistoryError::Corrupt
    );
    let count: i64 = second
        .connection
        .query_row("SELECT count(*) FROM chats", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}
