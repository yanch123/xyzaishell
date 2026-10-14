use super::*;
use std::fs;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("xyz-shell-store-test-{}", uuid::Uuid::new_v4())))
    }
    fn open(&self) -> ShellLogStore {
        ShellLogStore::open(&self.0).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn entry() -> ShellLogEntry {
    ShellLogEntry {
        id: id(),
        sequence: String::new(),
        session_id: id(),
        generation: "1".into(),
        shell_id: id(),
        shell_sequence: "1".into(),
        host: "synthetic.invalid".into(),
        port: 22,
        username: "fixture".into(),
        started_at_ms: 1,
        finished_at_ms: None,
        remote_started_at: Some("123.4".into()),
        command: "echo synthetic".into(),
        cwd: "/tmp".into(),
        status: CommandStatus::Running,
        exit_code: None,
        redacted: false,
    }
}
fn unchanged(s: &str) -> String {
    s.to_owned()
}
#[test]
fn append_reopen_finish_and_immutable_identity() {
    let f = Fixture::new();
    let mut s = f.open();
    let e = s.append(&entry(), unchanged).unwrap();
    assert_eq!(e.sequence, "1");
    assert_eq!(s.append(&e, unchanged).err(), Some(E::Conflict));
    s.finish(&e.id, &CommandStatus::Completed, Some(0), 2)
        .unwrap();
    assert_eq!(
        s.finish(&e.id, &CommandStatus::Completed, Some(1), 3).err(),
        Some(E::Conflict)
    );
    drop(s);
    let s = f.open();
    let p = s.query(&ShellLogQuery::default(), unchanged).unwrap();
    assert_eq!(p.entries.len(), 1);
    assert_eq!(p.entries[0].command, e.command);
    assert_eq!(p.entries[0].status, CommandStatus::Completed);
    assert_eq!(p.entries[0].exit_code, Some(0));
}
#[test]
fn pagination_snapshot_and_literal_filter() {
    let f = Fixture::new();
    let mut s = f.open();
    for _ in 0..4 {
        s.append(&entry(), unchanged).unwrap();
    }
    let mut q = ShellLogQuery {
        limit: Some(2),
        ..Default::default()
    };
    let p = s.query(&q, unchanged).unwrap();
    assert_eq!(
        p.entries
            .iter()
            .map(|e| e.sequence.as_str())
            .collect::<Vec<_>>(),
        vec!["4", "3"]
    );
    s.append(&entry(), unchanged).unwrap();
    q.before_seq = p.next_before_seq;
    q.snapshot_max_seq = Some(p.snapshot_max_seq);
    let p = s.query(&q, unchanged).unwrap();
    assert_eq!(
        p.entries
            .iter()
            .map(|e| e.sequence.as_str())
            .collect::<Vec<_>>(),
        vec!["2", "1"]
    );
    assert!(p.next_before_seq.is_none());
    let q = ShellLogQuery {
        contains: Some("%".into()),
        ..Default::default()
    };
    assert!(s.query(&q, unchanged).unwrap().entries.is_empty());
}
#[test]
fn redaction_before_disk_and_on_read_and_export() {
    let f = Fixture::new();
    let mut s = f.open();
    let mut e = entry();
    e.command = "echo fixture-secret later-secret".into();
    e.cwd = "/fixture-secret".into();
    e.username = "fixture-secret".into();
    e.host = "fixture-secret.invalid".into();
    s.append(&e, |s| s.replace("fixture-secret", "[redacted]"))
        .unwrap();
    let bytes = fs::read(f.0.join(DATABASE_NAME)).unwrap();
    assert!(!bytes
        .windows(b"fixture-secret".len())
        .any(|w| w == b"fixture-secret"));
    let p = s
        .query(&ShellLogQuery::default(), |s| {
            s.replace("later-secret", "[redacted]")
        })
        .unwrap();
    assert!(!p.entries[0].command.contains("later-secret"));
    assert!(p.entries[0].redacted);
    let export = s
        .export(&ShellLogQuery::default(), ExportFormat::Json, |s| {
            s.replace("later-secret", "[redacted]")
        })
        .unwrap();
    let text = fs::read_to_string(export.path).unwrap();
    assert!(!text.contains("later-secret"));
    assert_eq!(export.records, 1);
}
#[test]
fn export_csv_quotes_formula_newlines_and_unique_files() {
    let f = Fixture::new();
    let mut s = f.open();
    let mut e = entry();
    e.command = " \t=HYPERLINK(\"x\")\nnext".into();
    e.cwd = "@folder".into();
    s.append(&e, unchanged).unwrap();
    let a = s
        .export(&ShellLogQuery::default(), ExportFormat::Csv, unchanged)
        .unwrap();
    let b = s
        .export(&ShellLogQuery::default(), ExportFormat::Csv, unchanged)
        .unwrap();
    assert_ne!(a.path, b.path);
    let text = fs::read_to_string(&a.path).unwrap();
    assert!(text.contains("\"' \t=HYPERLINK(\"\"x\"\")\nnext\""));
    assert!(text.contains("\"'@folder\""));
    assert_eq!(text.len(), a.bytes);
    let j = s
        .export(&ShellLogQuery::default(), ExportFormat::Json, unchanged)
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&fs::read(j.path).unwrap()).unwrap();
    assert_eq!(v["entries"][0]["command"], e.command);
    assert_eq!(v["snapshot_max_seq"], "1");
    assert!(v["exported_at_ms"].as_i64().unwrap() > 0);
}
#[test]
fn invalid_and_oversized_records_rejected_without_truncation() {
    let f = Fixture::new();
    let mut s = f.open();
    let mut e = entry();
    e.command = "x".repeat(MAX_COMMAND_BYTES + 1);
    assert_eq!(s.append(&e, unchanged).err(), Some(E::InvalidRecord));
    e = entry();
    e.cwd = "x".repeat(MAX_CWD_BYTES + 1);
    assert_eq!(s.append(&e, unchanged).err(), Some(E::InvalidRecord));
    e = entry();
    e.session_id = "not-uuid".into();
    assert_eq!(s.append(&e, unchanged).err(), Some(E::InvalidRecord));
    assert_eq!(s.count().unwrap(), 0);
    for limit in [0, 201] {
        assert_eq!(
            s.query(
                &ShellLogQuery {
                    limit: Some(limit),
                    ..Default::default()
                },
                unchanged
            )
            .err(),
            Some(E::InvalidRecord)
        );
    }
}
#[test]
fn foreign_schema_is_preserved_and_mutation_rejected() {
    let f = Fixture::new();
    let s = f.open();
    let c = Connection::open(f.0.join(DATABASE_NAME)).unwrap();
    c.execute_batch("CREATE TABLE foreign_data(value TEXT)")
        .unwrap();
    assert_eq!(s.count().err(), Some(E::UnsupportedSchema));
    drop(c);
    drop(s);
    assert_eq!(ShellLogStore::open(&f.0).err(), Some(E::UnsupportedSchema));
    let c = Connection::open(f.0.join(DATABASE_NAME)).unwrap();
    assert!(c.prepare("SELECT * FROM foreign_data").is_ok());
}
#[test]
fn unknown_database_and_wal_are_not_reset() {
    let f = Fixture::new();
    let s = f.open();
    drop(s);
    let c = Connection::open(f.0.join(DATABASE_NAME)).unwrap();
    c.pragma_update(None, "user_version", 99).unwrap();
    drop(c);
    assert_eq!(ShellLogStore::open(&f.0).err(), Some(E::UnsupportedSchema));
    let f = Fixture::new();
    let s = f.open();
    drop(s);
    let d = PrivateDirectory::open_or_create(&f.0).unwrap();
    d.update("shell-log.sqlite3-wal", 1024, |_| {
        Ok(((), Some(Zeroizing::new(Vec::new()))))
    })
    .unwrap();
    assert_eq!(ShellLogStore::open(&f.0).err(), Some(E::UnsafeFile));
}
#[test]
fn export_respects_session_filter_but_not_page_cursor() {
    let f = Fixture::new();
    let mut s = f.open();
    let e = entry();
    s.append(&e, unchanged).unwrap();
    s.append(&entry(), unchanged).unwrap();
    let q = ShellLogQuery {
        session_id: Some(e.session_id),
        before_seq: Some("1".into()),
        limit: Some(1),
        ..Default::default()
    };
    let x = s.export(&q, ExportFormat::Json, unchanged).unwrap();
    assert_eq!(x.records, 1);
}
#[test]
fn bounded_output_refuses_overrun() {
    let mut o = BoundedOutput(vec![0; MAX_EXPORT_BYTES - 1]);
    assert!(o.write_all(&[1, 2]).is_err());
    assert_eq!(o.0.len(), MAX_EXPORT_BYTES - 1);
}
#[cfg(target_os = "linux")]
#[test]
fn symlink_database_and_export_directory_are_refused() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let s = f.open();
    drop(s);
    fs::rename(f.0.join(DATABASE_NAME), f.0.join("original")).unwrap();
    symlink("original", f.0.join(DATABASE_NAME)).unwrap();
    assert!(ShellLogStore::open(&f.0).is_err());
    let f = Fixture::new();
    let s = f.open();
    let target = Fixture::new();
    PrivateDirectory::open_or_create(&target.0).unwrap();
    symlink(&target.0, f.0.join("shell-log-exports")).unwrap();
    assert!(s
        .export(&ShellLogQuery::default(), ExportFormat::Json, unchanged)
        .is_err());
    assert_eq!(fs::read_dir(&target.0).unwrap().count(), 0);
}
#[cfg(target_os = "linux")]
#[test]
fn exported_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let s = f.open();
    let e = s
        .export(&ShellLogQuery::default(), ExportFormat::Json, unchanged)
        .unwrap();
    assert_eq!(
        fs::metadata(e.path).unwrap().permissions().mode() & 0o077,
        0
    );
}

#[test]
fn cooperating_instances_allocate_unique_sequences() {
    let f = Fixture::new();
    let mut first = f.open();
    let mut second = f.open();
    assert_eq!(first.append(&entry(), unchanged).unwrap().sequence, "1");
    assert_eq!(second.append(&entry(), unchanged).unwrap().sequence, "2");
    assert_eq!(first.count().unwrap(), 2);
}

#[test]
fn row_capacity_stops_append_without_removing_records() {
    let f = Fixture::new();
    let mut store = f.open();
    let mut e = entry();
    {
        let tx = store.connection.transaction().unwrap();
        let mut stmt = tx.prepare("INSERT INTO shell_logs(sequence,id,session_id,started_ms,command,payload) VALUES(?1,?2,?3,?4,?5,?6)").unwrap();
        for sequence in 1..=MAX_LOG_ROWS {
            e.id = id();
            e.sequence = sequence.to_string();
            stmt.execute(params![
                sequence as i64,
                e.id,
                e.session_id,
                e.started_at_ms,
                e.command,
                encode(&e).unwrap()
            ])
            .unwrap();
        }
        drop(stmt);
        tx.commit().unwrap();
    }
    assert_eq!(
        store.append(&entry(), unchanged).err(),
        Some(E::LimitReached)
    );
    assert_eq!(store.count().unwrap(), MAX_LOG_ROWS);
}

#[test]
fn exit_updates_never_replace_command_and_missing_id_is_not_found() {
    let f = Fixture::new();
    let mut store = f.open();
    assert_eq!(
        store.finish(&id(), &CommandStatus::Unknown, None, 5).err(),
        Some(E::NotFound)
    );
    let saved = store.append(&entry(), unchanged).unwrap();
    assert_eq!(
        store
            .finish(&saved.id, &CommandStatus::Running, None, 5)
            .err(),
        Some(E::InvalidRecord)
    );
    store
        .finish(&saved.id, &CommandStatus::Unknown, None, 5)
        .unwrap();
    let page = store.query(&ShellLogQuery::default(), unchanged).unwrap();
    assert_eq!(page.entries[0].command, saved.command);
    assert_eq!(page.entries[0].status, CommandStatus::Unknown);
}

#[test]
fn corrupted_payload_is_not_silently_skipped() {
    let f = Fixture::new();
    let mut store = f.open();
    let saved = store.append(&entry(), unchanged).unwrap();
    store
        .connection
        .execute(
            "UPDATE shell_logs SET payload='{}' WHERE id=?1",
            [&saved.id],
        )
        .unwrap();
    assert_eq!(
        store.query(&ShellLogQuery::default(), unchanged).err(),
        Some(E::Corrupt)
    );
    assert_eq!(
        store
            .export(&ShellLogQuery::default(), ExportFormat::Json, unchanged)
            .err(),
        Some(E::Corrupt)
    );
    drop(store);
    assert_eq!(ShellLogStore::open(&f.0).err(), Some(E::Corrupt));
    assert!(!f.0.join("shell-log-exports").exists());
}

#[test]
fn append_redactor_expansion_is_bounded() {
    let f = Fixture::new();
    let mut store = f.open();
    assert_eq!(
        store
            .append(&entry(), |_| "x".repeat(MAX_COMMAND_BYTES + 1))
            .err(),
        Some(E::InvalidRecord)
    );
    assert_eq!(store.count().unwrap(), 0);
}

#[test]
fn database_whitelist_remains_exact() {
    let f = Fixture::new();
    let d = PrivateDirectory::open_or_create(&f.0).unwrap();
    for name in [
        "other.sqlite3",
        "shell-log.sqlite3-extra",
        "shell-log.sqlite3-wal-extra",
        "../shell-log.sqlite3",
    ] {
        assert_eq!(
            d.prepare_database_file(name, 1024).err(),
            Some(StorageError::InvalidPath)
        );
        assert_eq!(
            d.validate_database_file(name, 1024).err(),
            Some(StorageError::InvalidPath)
        );
    }
    assert_eq!(
        d.prepare_database_file("shell-log.sqlite3-wal", 1024).err(),
        Some(StorageError::InvalidPath)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn shell_database_hardlinks_and_sidecar_symlinks_are_refused() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let store = f.open();
    fs::hard_link(f.0.join(DATABASE_NAME), f.0.join("other-link")).unwrap();
    assert_eq!(store.count().err(), Some(E::UnsafeFile));
    drop(store);
    let f = Fixture::new();
    let store = f.open();
    symlink(DATABASE_NAME, f.0.join("shell-log.sqlite3-journal")).unwrap();
    assert_eq!(store.count().err(), Some(E::UnsafeFile));
}

#[test]
fn finish_clamps_backwards_local_clock() {
    let f = Fixture::new();
    let mut store = f.open();
    let saved = store.append(&entry(), unchanged).unwrap();
    store
        .finish(&saved.id, &CommandStatus::Completed, Some(0), 0)
        .unwrap();
    let e = store
        .query(&ShellLogQuery::default(), unchanged)
        .unwrap()
        .entries
        .remove(0);
    assert_eq!(e.finished_at_ms, Some(e.started_at_ms));
}
