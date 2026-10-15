use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
fn snapshot(generation: &str) -> SessionSnapshot {
    SessionSnapshot {
        profile_id: None,
        revision: "1".into(),
        session: Some(SessionRef {
            session_id: format!(
                "00000000-0000-4000-8000-{:012}",
                generation.parse::<u64>().unwrap()
            ),
            generation: generation.into(),
        }),
        state: SessionState::Connected,
        host: Some("host.invalid".into()),
        port: Some(22),
        username: Some("user".into()),
        error: None,
        host_key: None,
    }
}
struct Fixture {
    service: ShellIntegrationService,
    snapshot: SessionSnapshot,
    nonce: String,
}
impl Fixture {
    fn new() -> Self {
        let service = ShellIntegrationService::new();
        let snapshot = snapshot("1");
        service.confirm_install(snapshot.clone(), true).unwrap();
        let nonce = service.status().shell_id.unwrap();
        Self {
            service,
            snapshot,
            nonce,
        }
    }
    fn session(&self) -> &SessionRef {
        self.snapshot.session.as_ref().unwrap()
    }
    fn event(&self, body: &str) {
        assert!(self
            .service
            .ingest(
                self.session(),
                format!("\x1b]777;xyzlog;2;{};{body}\x07", self.nonce).as_bytes()
            )
            .is_empty());
    }
    fn ready(&self) {
        self.event("H;5.2.37(1)-release");
        self.event("R;0;123;");
    }
    fn begin(&self, sequence: u64, command: &str) {
        self.event(&format!(
            "B;{sequence};123;L3RtcA==;{}",
            STANDARD.encode(command)
        ));
    }
}
#[test]
fn connected_is_not_ready_and_install_requires_one_explicit_confirmation() {
    let s = ShellIntegrationService::new();
    assert_eq!(s.status().phase, IntegrationPhase::Unintegrated);
    assert!(s.confirm_install(snapshot("1"), false).is_err());
    assert!(s.confirm_install(snapshot("1"), true).unwrap().is_some());
    assert!(s.confirm_install(snapshot("1"), true).is_err());
    let f = Fixture::new();
    f.event("H;5.2.37(1)-release");
    assert_eq!(f.service.status().phase, IntegrationPhase::AwaitingReady);
    assert!(f.service.ready_snapshot(f.session()).is_err());
    f.event("R;0;123;");
    assert!(f.service.ready_snapshot(f.session()).is_ok());
    assert!(f
        .service
        .confirm_install(f.snapshot.clone(), false)
        .unwrap()
        .is_none());
}

#[test]
fn unsupported_reasons_have_specific_safe_diagnostics_and_never_restore_readiness() {
    for (reason, code, hint) in [
        (
            "unsupported_bash",
            "SHELL_UNSUPPORTED_BASH",
            "Bash 5.1 or 5.2",
        ),
        (
            "namespace_conflict",
            "SHELL_UNSUPPORTED_NAMESPACE",
            "__xyz_",
        ),
        (
            "unsupported_options",
            "SHELL_UNSUPPORTED_OPTIONS",
            "HISTIGNORE",
        ),
        ("unsupported_trap", "SHELL_UNSUPPORTED_TRAP", "EXIT"),
        ("dynamic_prompt", "SHELL_UNSUPPORTED_DYNAMIC_PROMPT", "PS1"),
        (
            "unsupported_hook_attributes",
            "SHELL_UNSUPPORTED_HOOK_ATTRIBUTES",
            "PROMPT_COMMAND",
        ),
        ("missing_tools", "SHELL_UNSUPPORTED_MISSING_TOOLS", "base64"),
        (
            "private_unrecognized_reason",
            "SHELL_UNSUPPORTED",
            "unrecognized",
        ),
    ] {
        let f = Fixture::new();
        f.event(&format!("U;{reason}"));
        let status = f.service.status();
        assert_eq!(status.phase, IntegrationPhase::Unsupported, "{reason}");
        let diagnostic = status.error.unwrap();
        assert_eq!(diagnostic.code, code);
        assert!(
            diagnostic.safe_message.contains(hint),
            "{}",
            diagnostic.safe_message
        );
        assert!(diagnostic.safe_message.contains("manual terminal"));
        assert!(!diagnostic
            .safe_message
            .contains("private_unrecognized_reason"));
        assert!(f.service.ready_snapshot(f.session()).is_err());
        assert!(f.service.confirm_install(f.snapshot.clone(), true).is_err());
        // Later remote text cannot turn an unsupported session into readiness.
        f.ready();
        assert_eq!(f.service.status().phase, IntegrationPhase::Unsupported);
    }
}
#[test]
fn matched_end_is_not_ready_and_never_approves_another_action() {
    let f = Fixture::new();
    f.ready();
    let ready = f.service.ready_snapshot(f.session()).unwrap();
    let binding = f
        .service
        .prepare_dispatch(f.session(), "action-1", "pwd", &ready)
        .unwrap();
    assert!(f.service.validate_dispatch(f.session(), &binding).is_ok());
    f.begin(1, &binding.transport_command);
    f.event("E;1;124;7");
    let status = f.service.status();
    assert_eq!(status.completed.unwrap().exit_code, 7);
    assert_eq!(status.phase, IntegrationPhase::AwaitingReady);
    assert!(f.service.ready_snapshot(f.session()).is_err());
    assert!(f.service.validate_dispatch(f.session(), &binding).is_err());
    f.event("R;1;125;");
    assert_eq!(f.service.status().phase, IntegrationPhase::Ready);
    assert!(f
        .service
        .prepare_dispatch(f.session(), "action-2", "pwd", &ready)
        .is_err());
}
#[test]
fn manual_commands_invalidate_preview_and_conflicting_input_stops_automatic_work() {
    let f = Fixture::new();
    f.ready();
    let old = f.service.ready_snapshot(f.session()).unwrap();
    f.begin(1, "ls");
    f.event("E;1;124;0");
    f.event("R;1;125;bHM=");
    assert!(f.service.status().completed.is_none());
    assert!(f
        .service
        .prepare_dispatch(f.session(), "old", "pwd", &old)
        .is_err());
    let next = f.service.ready_snapshot(f.session()).unwrap();
    assert_eq!(next.shadowed_commands, vec!["ls"]);
    f.service
        .prepare_dispatch(f.session(), "new", "pwd", &next)
        .unwrap();
    f.begin(2, "whoami");
    assert_eq!(f.service.status().phase, IntegrationPhase::Unknown);
    f.event("E;2;124;0");
    f.event("R;2;125;");
    assert_eq!(f.service.status().phase, IntegrationPhase::Unknown);
    assert!(f.service.status().completed.is_none());
}
#[test]
fn missing_duplicate_out_of_order_or_foreign_frames_cannot_advance() {
    for bad in [
        "E;1;123;0",
        "R;1;123;",
        "H;5.2.37",
        "G;0;hooks_changed",
        "B;2;123;Lw==;cHdk",
    ] {
        let f = Fixture::new();
        f.ready();
        f.event(bad);
        assert_eq!(f.service.status().phase, IntegrationPhase::Unknown, "{bad}");
    }
    let f = Fixture::new();
    f.ready();
    let output = f.service.ingest(
        f.session(),
        b"text\x1b]777;xyzlog;2;00000000000000000000000000000000;R;0;123;\x07more",
    );
    assert_eq!(output, b"textmore");
    assert_eq!(f.service.status().phase, IntegrationPhase::Ready);
}
#[test]
fn disconnect_and_uncertain_delivery_invalidate_old_bindings_without_replay() {
    let f = Fixture::new();
    f.ready();
    let ready = f.service.ready_snapshot(f.session()).unwrap();
    let binding = f
        .service
        .prepare_dispatch(f.session(), "a", "pwd", &ready)
        .unwrap();
    f.service
        .dispatch_failed(f.session(), "a", "delivery unknown");
    assert_eq!(f.service.status().phase, IntegrationPhase::Unknown);
    assert!(f.service.validate_dispatch(f.session(), &binding).is_err());
    f.service.disconnect(f.session());
    assert!(f.service.status().completed.is_none());
    let next = snapshot("2");
    f.service.confirm_install(next.clone(), true).unwrap();
    f.event("R;0;123;");
    assert_eq!(f.service.status().phase, IntegrationPhase::Installing);
}
#[test]
fn disabled_or_failed_logs_cannot_disable_execution_tracking() {
    let f = Fixture::new();
    let log = ShellLogService::new(
        Err(error("NO_DISK", "Disk unavailable")),
        crate::observation::ObservationBuffer::new(),
        Arc::new(|_| {}),
    );
    f.service.attach_log(log.clone());
    f.ready();
    assert!(!log.status().enabled);
    let ready = f.service.ready_snapshot(f.session()).unwrap();
    let _ = log.start_shared(f.snapshot.clone(), true, ready.shell_id.clone(), true);
    assert!(!log.status().enabled);
    let binding = f
        .service
        .prepare_dispatch(f.session(), "a", "pwd", &ready)
        .unwrap();
    f.begin(1, &binding.transport_command);
    f.event("E;1;124;0");
    f.event("R;1;125;");
    assert_eq!(f.service.status().phase, IntegrationPhase::Ready);
    assert_eq!(f.service.status().completed.unwrap().action_id, "a");
}

#[test]
fn shared_logger_strips_only_exact_owned_comment_and_pause_is_independent() {
    let f = Fixture::new();
    f.ready();
    let path = std::env::temp_dir().join(format!("xyz-integration-log-{}", Uuid::new_v4()));
    let log = ShellLogService::new(
        Ok(path),
        crate::observation::ObservationBuffer::new(),
        Arc::new(|_| {}),
    );
    f.service.attach_log(log.clone());
    let ready = f.service.ready_snapshot(f.session()).unwrap();
    log.start_shared(f.snapshot.clone(), true, ready.shell_id.clone(), true)
        .unwrap();
    let first = f
        .service
        .prepare_dispatch(f.session(), "one", "pwd", &ready)
        .unwrap();
    f.begin(1, &first.transport_command);
    f.event("E;1;124;0");
    f.event("R;1;125;");
    log.flush().unwrap();
    let rows = log
        .query(crate::shell_log_types::ShellLogQuery::default())
        .unwrap()
        .entries;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].command, "pwd");
    log.pause().unwrap();
    let ready = f.service.ready_snapshot(f.session()).unwrap();
    let second = f
        .service
        .prepare_dispatch(f.session(), "two", "pwd", &ready)
        .unwrap();
    assert_ne!(first.transport_command, second.transport_command);
    f.begin(2, &second.transport_command);
    f.event("E;2;124;0");
    f.event("R;2;125;");
    assert_eq!(f.service.status().phase, IntegrationPhase::Ready);
    assert_eq!(
        log.query(crate::shell_log_types::ShellLogQuery::default())
            .unwrap()
            .entries
            .len(),
        1
    );
    log.resume(f.session()).unwrap();
    // A manual look-alike suffix is retained: it was not locally bound.
    f.begin(3, "printf '# quoted' # xyz-action:unowned");
    f.event("E;3;124;0");
    f.event("R;3;125;");
    let rows = log
        .query(crate::shell_log_types::ShellLogQuery::default())
        .unwrap()
        .entries;
    assert!(rows
        .iter()
        .any(|r| r.command == "printf '# quoted' # xyz-action:unowned"));
    log.disconnect(f.session());
}
#[test]
fn proven_unsent_cancellation_restores_new_ready_revision_but_never_started_input() {
    let f = Fixture::new();
    f.ready();
    let ready = f.service.ready_snapshot(f.session()).unwrap();
    let binding = f
        .service
        .prepare_dispatch(f.session(), "cancel", "pwd", &ready)
        .unwrap();
    f.service.dispatch_failed(f.session(), "cancel", "not_sent");
    let now = f.service.ready_snapshot(f.session()).unwrap();
    assert!(now.revision > ready.revision);
    assert!(f.service.validate_dispatch(f.session(), &binding).is_err());
    let binding = f
        .service
        .prepare_dispatch(f.session(), "started", "pwd", &now)
        .unwrap();
    f.begin(1, &binding.transport_command);
    f.service
        .dispatch_failed(f.session(), "started", "not_sent");
    assert_eq!(f.service.status().phase, IntegrationPhase::Unknown);
}
#[test]
fn action_comment_keeps_quoted_hash_and_rejects_control_or_marker_injection() {
    assert_eq!(
        correlated_command("a-b", "printf '# literal'").unwrap(),
        "printf '# literal' # xyz-action:a-b"
    );
    for id in ["bad; exit", "bad\n", "$(x)", ""] {
        assert!(correlated_command(id, "pwd").is_err());
    }
    for cmd in [" pwd", "pwd\nls", "pwd\r", ""] {
        assert!(correlated_command("id", cmd).is_err());
    }
}

#[test]
fn known_correlation_echo_is_filtered_across_every_split_without_hiding_user_text() {
    let input = b"pwd # xyz-action:local-id\r\noutput # xyz-action:unowned\r\n";
    for split in 0..=input.len() {
        let mut filter = EchoMetadataFilter::default();
        filter.register("local-id");
        let mut output = filter.feed(&input[..split]);
        output.extend(filter.feed(&input[split..]));
        assert_eq!(output, b"pwd\r\noutput # xyz-action:unowned\r\n");
        assert!(filter.prefix.len() < 160);
    }
}

#[test]
fn late_install_handshake_cannot_erase_timeout_without_a_status_poll() {
    let f = Fixture::new();
    f.service.inner.lock().unwrap().deadline = Some(Instant::now() - Duration::from_secs(1));
    f.ready();
    assert_eq!(f.service.status().phase, IntegrationPhase::Unknown);
    assert!(f.service.ready_snapshot(f.session()).is_err());
}

#[test]
fn shared_prompt_boundary_gap_reaches_logger_even_after_foreground_end() {
    let f = Fixture::new();
    f.ready();
    let path = std::env::temp_dir().join(format!("xyz-integration-gap-{}", Uuid::new_v4()));
    let log = ShellLogService::new(
        Ok(path),
        crate::observation::ObservationBuffer::new(),
        Arc::new(|_| {}),
    );
    f.service.attach_log(log.clone());
    let ready = f.service.ready_snapshot(f.session()).unwrap();
    log.start_shared(f.snapshot.clone(), true, ready.shell_id, true)
        .unwrap();
    // Matches the real OpenSSH wire sequence from `set +o history` exactly.
    f.begin(1, "set +o history");
    f.event("E;1;124;0");
    f.event("G;1;hooks_changed");
    assert_eq!(f.service.status().phase, IntegrationPhase::Unknown);
    assert_eq!(
        log.status().state,
        crate::shell_log_types::CaptureState::Gap
    );
    let rows = log
        .query(crate::shell_log_types::ShellLogQuery::default())
        .unwrap()
        .entries;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].status,
        crate::shell_log_types::CommandStatus::Completed
    );
    assert_eq!(rows[0].exit_code, Some(0));
    assert!(f.service.ready_snapshot(f.session()).is_err());
    log.disconnect(f.session());
}

#[test]
fn paused_shared_logger_retains_prompt_gap_and_cannot_resume() {
    let f = Fixture::new();
    f.ready();
    let path = std::env::temp_dir().join(format!("xyz-integration-paused-gap-{}", Uuid::new_v4()));
    let log = ShellLogService::new(
        Ok(path),
        crate::observation::ObservationBuffer::new(),
        Arc::new(|_| {}),
    );
    f.service.attach_log(log.clone());
    let ready = f.service.ready_snapshot(f.session()).unwrap();
    log.start_shared(f.snapshot.clone(), true, ready.shell_id, true)
        .unwrap();
    log.pause().unwrap();
    f.begin(1, "set +o history");
    f.event("E;1;124;0");
    f.event("G;1;hooks_changed");
    assert_eq!(f.service.status().phase, IntegrationPhase::Unknown);
    assert_eq!(
        log.status().state,
        crate::shell_log_types::CaptureState::Paused
    );
    assert_eq!(
        log.resume(f.session()).unwrap_err().code,
        "SHELL_LOG_INTEGRATION_LOST"
    );
    assert!(log
        .query(crate::shell_log_types::ShellLogQuery::default())
        .unwrap()
        .entries
        .is_empty());
    log.disconnect(f.session());
}
