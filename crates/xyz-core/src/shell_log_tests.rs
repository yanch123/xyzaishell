use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
struct Fixture {
    service: ShellLogService,
    path: PathBuf,
    snapshot: SessionSnapshot,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("xyz-shell-service-{}", Uuid::new_v4()));
        let snapshot = SessionSnapshot {
            profile_id: None,
            revision: "1".into(),
            session: Some(SessionRef {
                session_id: Uuid::new_v4().to_string(),
                generation: "1".into(),
            }),
            state: SessionState::Connected,
            host: Some("fixture.invalid".into()),
            port: Some(22),
            username: Some("tester".into()),
            error: None,
            host_key: None,
        };
        let service =
            ShellLogService::new(Ok(path.clone()), ObservationBuffer::new(), Arc::new(|_| {}));
        Self {
            service,
            path,
            snapshot,
        }
    }
    fn start(&self) -> String {
        self.service.start(self.snapshot.clone(), true).unwrap()
    }
    fn session(&self) -> &SessionRef {
        self.snapshot.session.as_ref().unwrap()
    }
    fn event(&self, event: WireEvent) {
        let mut c = self.service.capture.lock().unwrap();
        self.service.handle_event(&mut c, event);
    }
    fn hello(&self) {
        self.event(WireEvent::Hello {
            shell_version: "5.2".into(),
        });
    }
    fn begin(&self, seq: u64, command: &str) {
        self.event(WireEvent::Begin {
            sequence: seq,
            remote_time_s: 123,
            cwd: "/tmp".into(),
            command: command.into(),
        });
    }
    fn end(&self, seq: u64) {
        self.event(WireEvent::End {
            sequence: seq,
            remote_time_s: 124,
            exit_code: 7,
        });
    }
    fn rows(&self) -> Vec<ShellLogEntry> {
        self.service
            .query(ShellLogQuery::default())
            .unwrap()
            .entries
    }
}
// Do not remove files while a worker may still have them open. Test fixtures use
// unique private directories and explicitly disconnect before inspecting them.
#[test]
fn disabled_until_explicit_consent_and_hello_and_no_duplicate_enable() {
    let f = Fixture::new();
    assert_eq!(f.service.status().state, CaptureState::Disabled);
    assert!(!f.path.exists());
    assert!(f.service.start(f.snapshot.clone(), false).is_err());
    assert!(!f.path.exists());
    let bootstrap = f.start();
    assert!(bootstrap.len() < 16 * 1024);
    assert_eq!(f.service.status().state, CaptureState::Starting);
    f.begin(1, "not accepted before hello");
    assert!(f.rows().is_empty());
    f.hello();
    assert_eq!(f.service.status().state, CaptureState::Recording);
    assert!(f.service.start(f.snapshot.clone(), true).is_err());
    f.begin(1, "printf 'hello'\nprintf 'world'");
    f.end(1);
    let rows = f.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, CommandStatus::Completed);
    assert_eq!(rows[0].exit_code, Some(7));
    assert!(rows[0].command.contains('\n'));
    assert_eq!(rows[0].host, "fixture.invalid");
}
#[test]
fn next_begin_unknown_and_replays_never_change_old_records() {
    let f = Fixture::new();
    f.start();
    f.hello();
    f.begin(2, "two");
    f.begin(3, "three");
    f.end(2);
    f.begin(2, "forged old replacement");
    f.end(3);
    f.end(3);
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].command, "three");
    assert_eq!(rows[0].status, CommandStatus::Completed);
    assert_eq!(rows[1].command, "two");
    assert_eq!(rows[1].status, CommandStatus::Unknown);
}
#[test]
fn pause_closes_active_and_resume_never_collects_paused_commands() {
    let f = Fixture::new();
    f.start();
    f.hello();
    f.begin(1, "recorded");
    f.service.flush().unwrap();
    f.service.pause().unwrap();
    f.begin(2, "private while paused");
    f.end(2);
    f.service.flush().unwrap();
    assert_eq!(f.rows()[0].status, CommandStatus::Unknown);
    f.service.resume(f.session()).unwrap();
    f.end(1);
    f.begin(2, "replayed paused command");
    f.begin(3, "after resume");
    f.end(3);
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].command, "after resume");
}
#[test]
fn disconnect_and_shell_exit_are_unknown_never_success() {
    let f = Fixture::new();
    f.start();
    f.hello();
    f.begin(1, "one");
    f.service.flush().unwrap();
    f.service.disconnect(f.session());
    f.service.flush().unwrap();
    assert_eq!(f.rows()[0].status, CommandStatus::Unknown);
    assert_eq!(f.service.status().state, CaptureState::Disconnected);
    let f = Fixture::new();
    f.start();
    f.hello();
    f.begin(1, "exit");
    f.event(WireEvent::Closed);
    f.service.flush().unwrap();
    assert_eq!(f.rows()[0].status, CommandStatus::Unknown);
    assert!(!f.service.status().enabled);
}
#[test]
fn gaps_malformed_frames_and_unmatched_end_do_not_fabricate_records() {
    let f = Fixture::new();
    f.start();
    f.hello();
    f.end(77);
    assert!(f.rows().is_empty());
    f.begin(1, "one");
    f.event(WireEvent::Malformed {
        reason: "invalid".into(),
    });
    f.end(1);
    f.event(WireEvent::Gap {
        sequence: 2,
        reason: "oversize".into(),
    });
    assert_eq!(f.service.status().state, CaptureState::Gap);
    let rows = f.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, CommandStatus::Unknown);
}
#[test]
fn metadata_filtered_when_disabled_paused_and_wrong_nonce() {
    let f = Fixture::new();
    let frame = b"before\x1b]777;xyzlog;1;00000000000000000000000000000000;H;5.2\x07after";
    assert_eq!(f.service.ingest(f.session(), frame), b"beforeafter");
    f.start();
    f.hello();
    f.service.pause().unwrap();
    assert_eq!(f.service.ingest(f.session(), frame), b"beforeafter");
    assert!(f.rows().is_empty());
}
#[test]
fn protocol_only_collection_and_known_secret_filtering() {
    let f = Fixture::new();
    f.service
        .redactor
        .begin(f.session().clone(), vec!["known-password".into()]);
    f.start();
    f.hello();
    // Raw output (including password prompts) is never a source of log entries.
    let output = b"Password: known-password\r\n$ rm secret\r\n";
    assert_eq!(f.service.ingest(f.session(), output), output);
    assert!(f.rows().is_empty());
    f.begin(1, "echo known-password");
    f.end(1);
    let rows = f.rows();
    assert!(!rows[0].command.contains("known-password"));
    assert!(rows[0].redacted);
    let export = f
        .service
        .export(ShellLogQuery::default(), ExportFormat::Json)
        .unwrap();
    assert!(!std::fs::read_to_string(export.path)
        .unwrap()
        .contains("known-password"));
}
#[test]
fn actual_parser_frames_drive_capture_with_local_receipt_time() {
    let f = Fixture::new();
    f.start();
    let nonce = "0123456789abcdef0123456789abcdef";
    f.service.capture.lock().unwrap().parser = ProtocolParser::new(nonce).unwrap();
    let data=format!("\x1b]777;xyzlog;1;{nonce};H;5.2\x07\x1b]777;xyzlog;1;{nonce};B;1;123;{};{}\x07\x1b]777;xyzlog;1;{nonce};E;1;124;0\x07",STANDARD.encode(b"/tmp"),STANDARD.encode(b"echo sample"));
    for byte in data.bytes() {
        assert!(f.service.ingest(f.session(), &[byte]).is_empty());
    }
    let row = &f.rows()[0];
    assert_eq!(row.command, "echo sample");
    assert!(row.started_at_ms > 123_000);
    assert_eq!(row.remote_started_at.as_deref(), Some("123"));
    assert_eq!(row.status, CommandStatus::Completed);
}
#[test]
fn handshake_timeout_and_write_failure_require_reconnection() {
    let f = Fixture::new();
    f.start();
    *f.service.shared.handshake_deadline.lock().unwrap() =
        Some(Instant::now() - Duration::from_secs(1));
    assert_eq!(
        f.service.status().error.unwrap().code,
        "SHELL_LOG_HANDSHAKE_TIMEOUT"
    );
    f.hello();
    assert!(!f.service.status().enabled);
    assert!(f.service.start(f.snapshot.clone(), true).is_err());
    let f = Fixture::new();
    f.start();
    f.service
        .fail_start(f.session(), error("delivery", "uncertain"));
    assert_eq!(
        f.service.status().error.unwrap().code,
        "SHELL_LOG_START_FAILED"
    );
    assert!(f.service.start(f.snapshot.clone(), true).is_err());
}
#[test]
fn bounded_queue_failure_is_visible_and_stops_capture_without_blocking() {
    let f = Fixture::new();
    f.start();
    f.hello();
    f.service.flush().unwrap();
    let gate = f.service.shared.commit.lock().unwrap();
    // Freeze the worker, not ingestion; overflow must return immediately.
    for seq in 1..=(QUEUE_CAPACITY as u64 + 5) {
        f.begin(seq, "queued");
    }
    assert!(!f.service.status().enabled);
    assert_eq!(
        f.service.status().error.unwrap().code,
        "SHELL_LOG_QUEUE_FULL"
    );
    drop(gate);
    // A query can initially be busy while the worker drains invalid epochs.
    loop {
        match f.service.query(ShellLogQuery::default()) {
            Ok(page) => {
                assert!(page.entries.is_empty());
                break;
            }
            Err(e) if e.code == "SHELL_LOG_BUSY" => std::thread::yield_now(),
            Err(e) => panic!("{e:?}"),
        }
    }
}
#[test]
fn pause_invalidates_preexisting_queue_epoch() {
    let f = Fixture::new();
    f.start();
    f.hello();
    f.service.flush().unwrap();
    let gate = f.service.shared.commit.lock().unwrap();
    f.begin(1, "queued before pause");
    // Same critical section used by pause, exercised deterministically while the
    // worker is held before its first write.
    f.service.shared.epoch.fetch_add(1, Ordering::SeqCst);
    f.service.shared.update(|s| s.state = CaptureState::Paused);
    f.service.capture.lock().unwrap().active = None;
    drop(gate);
    f.service.flush().unwrap();
    assert!(f.rows().is_empty());
}

#[test]
fn listing_disabled_log_does_not_create_storage_or_enable_capture() {
    let f = Fixture::new();
    assert!(f.rows().is_empty());
    assert!(!f.path.exists());
    assert!(f
        .service
        .export(ShellLogQuery::default(), ExportFormat::Json)
        .is_err());
    assert!(!f.path.exists());
    assert_eq!(f.service.status().state, CaptureState::Disabled);
}

#[test]
fn stale_session_bytes_cannot_poison_current_parser() {
    let f = Fixture::new();
    f.start();
    let stale = SessionRef {
        session_id: Uuid::new_v4().to_string(),
        generation: "0".into(),
    };
    assert!(f.service.ingest(&stale, b"\x1b]777;xyzlog;1;").is_empty());
    assert_eq!(
        f.service.ingest(f.session(), b"ordinary current output"),
        b"ordinary current output"
    );
}

#[test]
fn reconnected_disabled_shell_keeps_observations_and_rejects_old_stream() {
    let f = Fixture::new();
    f.start();
    f.hello();
    f.service.disconnect(f.session());
    let next = SessionRef {
        session_id: Uuid::new_v4().to_string(),
        generation: "2".into(),
    };
    assert_eq!(f.service.ingest(&next, b"new shell"), b"new shell");
    assert!(f
        .service
        .ingest(f.session(), b"\x1b]777;xyzlog;1;")
        .is_empty());
    assert_eq!(f.service.ingest(&next, b"still visible"), b"still visible");
    assert!(!f.service.status().enabled);
}

#[test]
fn current_sequence_prompt_gap_remains_visible_after_completed_foreground() {
    let f = Fixture::new();
    f.start();
    f.hello();
    f.begin(1, "set +o history");
    f.end(1);
    f.event(WireEvent::Gap {
        sequence: 1,
        reason: "hooks_changed".into(),
    });
    assert_eq!(f.service.status().state, CaptureState::Gap);
    let rows = f.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, CommandStatus::Completed);
    assert_eq!(rows[0].exit_code, Some(7));
    // Repeated gap notifications do not revise the completed command outcome.
    f.event(WireEvent::Gap {
        sequence: 1,
        reason: "hooks_changed".into(),
    });
    assert_eq!(f.rows()[0].status, CommandStatus::Completed);
}
#[test]
fn same_sequence_gap_before_end_is_unknown_but_older_gap_is_a_replay() {
    let f = Fixture::new();
    f.start();
    f.hello();
    f.begin(1, "one");
    f.event(WireEvent::Gap {
        sequence: 1,
        reason: "hooks_changed".into(),
    });
    f.end(1);
    assert_eq!(f.rows()[0].status, CommandStatus::Unknown);
    // Separate healthy capture: an older gap must not poison its newer record.
    let f = Fixture::new();
    f.start();
    f.hello();
    f.begin(1, "one");
    f.end(1);
    f.begin(2, "two");
    f.event(WireEvent::Gap {
        sequence: 1,
        reason: "history_missing".into(),
    });
    assert_eq!(f.service.status().state, CaptureState::Recording);
    f.end(2);
    let rows = f.rows();
    assert_eq!(rows[0].command, "two");
    assert_eq!(rows[0].status, CommandStatus::Completed);
}

#[test]
fn metadata_loss_while_paused_is_retained_and_resume_never_claims_recording() {
    for loss in [
        WireEvent::Gap {
            sequence: 1,
            reason: "hooks_changed".into(),
        },
        WireEvent::Malformed {
            reason: "invalid_frame".into(),
        },
    ] {
        let f = Fixture::new();
        f.start();
        f.hello();
        f.begin(1, "one");
        f.end(1);
        f.service.flush().unwrap();
        f.service.pause().unwrap();
        f.event(loss);
        assert_eq!(f.service.status().state, CaptureState::Paused);
        assert!(f
            .service
            .status()
            .reason
            .unwrap()
            .contains("integration was lost"));
        assert_eq!(
            f.service.resume(f.session()).unwrap_err().code,
            "SHELL_LOG_INTEGRATION_LOST"
        );
        f.begin(2, "must not silently revive capture");
        f.end(2);
        assert_eq!(f.service.status().state, CaptureState::Paused);
        assert_eq!(f.rows().len(), 1);
    }
}
#[test]
fn pause_after_gap_cannot_clear_loss_evidence_or_resume() {
    let f = Fixture::new();
    f.start();
    f.hello();
    f.begin(1, "one");
    f.end(1);
    f.event(WireEvent::Gap {
        sequence: 1,
        reason: "hooks_changed".into(),
    });
    f.service.pause().unwrap();
    assert_eq!(
        f.service.resume(f.session()).unwrap_err().code,
        "SHELL_LOG_INTEGRATION_LOST"
    );
    assert_eq!(f.service.status().state, CaptureState::Paused);
}
