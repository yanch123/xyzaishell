//! Real OpenSSH/OS-PTY command capture without configuring any AI provider.
//! Provision with .github/scripts/openssh-test.sh; never targets a remote host.
#![cfg(unix)]
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{env, sync::Arc, time::Duration};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout, Instant},
};
use uuid::Uuid;
use xyz_core::{
    shell_integration::IntegrationPhase,
    shell_log::ShellLogService,
    shell_log_types::{CaptureState, CommandStatus, ExportFormat, ShellLogEntry, ShellLogQuery},
    ssh::{
        Authentication, ConnectRequest, HostKeyDecision, SessionManager, SessionRef, SessionState,
        SshEvent, TerminalSize, TrustDecision,
    },
};
const WAIT: Duration = Duration::from_secs(20);
struct Terminal {
    manager: SessionManager,
    service: ShellLogService,
    incoming: mpsc::UnboundedReceiver<SshEvent>,
    output: Vec<u8>,
    fingerprint: String,
}
impl Terminal {
    fn new(root: std::path::PathBuf) -> Self {
        let (tx, incoming) = mpsc::unbounded_channel();
        let manager = SessionManager::new(Arc::new(move |event| tx.send(event).is_ok()));
        let service = ShellLogService::new(Ok(root), manager.observation(), Arc::new(|_| {}));
        manager.attach_shell_log(service.clone());
        Self {
            manager,
            service,
            incoming,
            output: Vec::new(),
            fingerprint: env::var("XYZ_OPENSSH_HOST_FINGERPRINT")
                .expect("provision OpenSSH fixture"),
        }
    }
    fn drain(&mut self) {
        while let Ok(event) = self.incoming.try_recv() {
            match event {
                SshEvent::Output {
                    session,
                    seq,
                    data_base64,
                    ..
                } => {
                    self.output.extend(STANDARD.decode(data_base64).unwrap());
                    self.manager.ack_output(&session, &seq).unwrap();
                }
                SshEvent::HostKey { challenge } => {
                    assert_eq!(challenge.fingerprint, self.fingerprint);
                    self.manager
                        .confirm_host_key(HostKeyDecision {
                            challenge_id: challenge.challenge_id,
                            expected_fingerprint: self.fingerprint.clone(),
                            decision: TrustDecision::Accept,
                        })
                        .unwrap();
                }
                SshEvent::State { snapshot } => {
                    assert_ne!(snapshot.state, SessionState::Error, "{snapshot:?}")
                }
                _ => {}
            }
        }
    }
    async fn until(&mut self, condition: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            self.drain();
            if condition(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "OpenSSH log timeout: {:?}; output={:?}",
                self.service.status(),
                String::from_utf8_lossy(&self.output)
            );
            sleep(Duration::from_millis(20)).await;
        }
    }
    async fn connect(&mut self) -> SessionRef {
        let session = self
            .manager
            .connect(ConnectRequest {
                host: "127.0.0.1".into(),
                port: env::var("XYZ_OPENSSH_PORT")
                    .expect("fixture port")
                    .parse()
                    .unwrap(),
                username: env::var("XYZ_OPENSSH_USER").expect("fixture user"),
                auth: Authentication::PrivateKey {
                    private_key_path: env::var("XYZ_OPENSSH_PRIVATE_KEY").expect("fixture key"),
                    passphrase: None,
                },
                session_only: true,
                size: TerminalSize {
                    cols: 120,
                    rows: 40,
                },
            })
            .unwrap()
            .session
            .unwrap();
        self.until(|t| t.manager.snapshot().state == SessionState::Connected)
            .await;
        // Preparation runs before the first prompt, entirely in the SSH exec
        // payload. Do not repair shell options or inject a bootstrap into stdin.
        self.until(|t| t.manager.shell_integration().status().phase == IntegrationPhase::Ready)
            .await;
        let ready = self
            .manager
            .shell_integration()
            .ready_snapshot(&session)
            .unwrap();
        self.service
            .start_shared(self.manager.snapshot(), true, ready.shell_id, true)
            .unwrap();
        session
    }
    async fn write(&self, session: &SessionRef, command: &str) {
        let receipt = self
            .manager
            .write(session, &STANDARD.encode(command))
            .await
            .unwrap();
        assert_eq!(receipt.delivery, "write_accepted");
        assert_eq!(receipt.remote_outcome, "unknown");
    }
    fn rows(&self, session: &SessionRef) -> Vec<ShellLogEntry> {
        self.service
            .query(ShellLogQuery {
                session_id: Some(session.session_id.clone()),
                limit: Some(100),
                ..Default::default()
            })
            .unwrap()
            .entries
    }
    async fn completed(&mut self, session: &SessionRef, command: &str) -> ShellLogEntry {
        self.write(session, &format!("{command}\n")).await;
        self.until(|t| {
            t.rows(session).iter().any(|r| {
                r.command.trim_end_matches('\n') == command && r.status == CommandStatus::Completed
            })
        })
        .await;
        self.rows(session)
            .into_iter()
            .find(|r| r.command.trim_end_matches('\n') == command)
            .unwrap()
    }
    async fn ai_completed(&mut self, session: &SessionRef, command: &str) -> ShellLogEntry {
        self.until(|t| {
            t.manager
                .shell_integration()
                .ready_snapshot(session)
                .is_ok()
        })
        .await;
        let integration = self.manager.shell_integration();
        let ready = integration.ready_snapshot(session).unwrap();
        let action_id = Uuid::new_v4().to_string();
        let binding = integration
            .prepare_dispatch(session, &action_id, command, &ready)
            .unwrap();
        self.write(session, &format!("{}\n", binding.transport_command))
            .await;
        self.until(|t| {
            t.manager
                .shell_integration()
                .receiver_ready(session, &action_id)
        })
        .await;
        integration.consume_receiver(session, &action_id).unwrap();
        let wire =
            xyz_core::shell_integration::action_wire_frame(session, &action_id, command, &ready)
                .unwrap();
        self.write(session, std::str::from_utf8(&wire).unwrap())
            .await;
        self.until(|t| {
            t.manager.shell_integration().status().phase == IntegrationPhase::Ready
                && t.rows(session)
                    .iter()
                    .any(|row| row.command == command && row.status == CommandStatus::Completed)
        })
        .await;
        let rows = self.rows(session);
        assert_eq!(
            rows.iter().filter(|row| row.command == command).count(),
            1,
            "one AI payload must create exactly one log entry"
        );
        assert!(
            !rows.iter().any(|row| row.command.contains("__xyz_")),
            "envelope internals must never be duplicated as manual commands"
        );
        let completed = integration.status().completed.unwrap();
        assert_eq!(completed.action_id, action_id);
        assert_eq!(completed.sequence, binding.sequence);
        rows.into_iter().find(|row| row.command == command).unwrap()
    }

    async fn disconnect(&mut self, session: &SessionRef) {
        if self.manager.snapshot().state != SessionState::Disconnected {
            self.manager.disconnect(session).unwrap();
        }
        self.until(|t| t.manager.snapshot().state == SessionState::Disconnected)
            .await;
        self.service.flush().unwrap();
    }
}
// Bash4.2 promises exact framed AI audit, while manual input has an explicit
// observation gap followed by an independently verified prompt boundary.
async fn legacy_prompt_only_contract(t: &mut Terminal, first: &SessionRef) {
    for (command, code) in [
        ("xyz_fixture_function", 23),
        ("xyz_fixture_alias", 0),
        ("printf 'XYZ_RUNTIME:%s\\n' \"$BASH_VERSION\"", 0),
        ("printf '%s\\n' alpha beta | grep beta", 0),
        ("false || printf RECOVERED; true", 0),
        ("false", 1),
        ("export XYZ_AI_TEST_STATE=preserved", 0),
        ("printenv XYZ_AI_TEST_STATE", 0),
        ("cd /tmp; pwd", 0),
    ] {
        assert_eq!(t.ai_completed(first, command).await.exit_code, Some(code));
    }
    let expected = format!(
        "XYZ_RUNTIME:{}\r\n",
        env::var("XYZ_OPENSSH_BASH_VERSION").unwrap()
    );
    assert!(t
        .output
        .windows(expected.len())
        .any(|b| b == expected.as_bytes()));
    if env::var_os("XYZ_OPENSSH_FIXTURE_BASHRC").is_some() {
        assert!(t
            .output
            .windows(b"XYZ_ALIAS_PRESERVED".len())
            .any(|b| b == b"XYZ_ALIAS_PRESERVED"));
    }
    let before = t.manager.shell_integration().ready_snapshot(first).unwrap();
    t.write(first, "printf MANUAL_OBSERVATION_GAP\n").await;
    t.until(|t| {
        t.manager
            .shell_integration()
            .ready_snapshot(first)
            .is_ok_and(|ready| ready.revision > before.revision)
    })
    .await;
    assert!(t
        .rows(first)
        .iter()
        .all(|r| r.command != "printf MANUAL_OBSERVATION_GAP"));
    assert_eq!(t.service.status().state, CaptureState::Gap);
    assert_eq!(
        t.ai_completed(first, "printf AI_AFTER_MANUAL_GAP")
            .await
            .exit_code,
        Some(0)
    );
    t.output.clear();
    let before = t.manager.shell_integration().ready_snapshot(first).unwrap();
    t.write(first, "read -s -p XYZ_SECRET_READY xyz_secret\n")
        .await;
    t.until(|t| t.output.windows(16).any(|b| b == b"XYZ_SECRET_READY"))
        .await;
    let secret = "SYNTHETIC_READ_PASSWORD_NOT_A_COMMAND";
    t.write(first, &format!("{secret}\n")).await;
    t.until(|t| {
        t.manager
            .shell_integration()
            .ready_snapshot(first)
            .is_ok_and(|ready| ready.revision > before.revision)
    })
    .await;
    assert!(t.rows(first).iter().all(|r| !r.command.contains(secret)));
    t.write(first, "exit\n").await;
    t.until(|t| !t.service.status().enabled).await;
    assert!(t.rows(first).iter().all(|r| r.command != "exit"));
    t.service.flush().unwrap();
    let exported = t
        .service
        .export(ShellLogQuery::default(), ExportFormat::Json)
        .unwrap();
    let data = std::fs::read_to_string(exported.path).unwrap();
    assert!(data.contains("AI_AFTER_MANUAL_GAP"));
    assert!(!data.contains(secret));
    t.disconnect(first).await;
    let second = t.connect().await;
    t.write(&second, "set +o history\n").await;
    t.until(|t| t.manager.shell_integration().status().phase == IntegrationPhase::Unknown)
        .await;
    assert_eq!(t.service.status().state, CaptureState::Gap);
    assert!(t
        .rows(&second)
        .iter()
        .all(|r| r.command != "set +o history"));
    t.disconnect(&second).await;
    let third = t.connect().await;
    let integration = t.manager.shell_integration();
    let ready = integration.ready_snapshot(&third).unwrap();
    let action_id = Uuid::new_v4().to_string();
    let binding = integration
        .prepare_dispatch(&third, &action_id, "sleep 30", &ready)
        .unwrap();
    t.write(&third, &format!("{}\n", binding.transport_command))
        .await;
    t.until(|t| {
        t.manager
            .shell_integration()
            .receiver_ready(&third, &action_id)
    })
    .await;
    integration.consume_receiver(&third, &action_id).unwrap();
    let wire =
        xyz_core::shell_integration::action_wire_frame(&third, &action_id, "sleep 30", &ready)
            .unwrap();
    t.write(&third, std::str::from_utf8(&wire).unwrap()).await;
    t.until(|t| t.rows(&third).iter().any(|r| r.command == "sleep 30"))
        .await;
    t.disconnect(&third).await;
    let interrupted = t
        .rows(&third)
        .into_iter()
        .find(|r| r.command == "sleep 30")
        .unwrap();
    assert_eq!(interrupted.status, CommandStatus::Unknown);
    assert_eq!(interrupted.exit_code, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires real local OpenSSH fixture; run .github/scripts/openssh-test.sh"]
async fn shell_log_real_openssh_records_commands_without_model() {
    timeout(Duration::from_secs(120), async {
        let root = env::temp_dir().join(format!("xyz-shell-log-openssh-{}", Uuid::new_v4()));
        let mut t = Terminal::new(root.clone());
        let first = t.connect().await;
        if env::var("XYZ_OPENSSH_BASH_VERSION")
            .unwrap_or_default()
            .starts_with("4.2.")
        {
            legacy_prompt_only_contract(&mut t, &first).await;
            drop(t);
            std::fs::remove_dir_all(root).unwrap();
            return;
        }
        // Disposable fixture definitions come from .bashrc, before hooks are
        // installed. Verify both are preserved in the prepared shell.
        if env::var_os("XYZ_OPENSSH_FIXTURE_BASHRC").is_some() {
            assert_eq!(
                t.completed(&first, "xyz_fixture_function").await.exit_code,
                Some(23)
            );
            assert_eq!(
                t.completed(&first, "xyz_fixture_alias").await.exit_code,
                Some(0)
            );
            assert!(t
                .output
                .windows(b"XYZ_ALIAS_PRESERVED".len())
                .any(|s| s == b"XYZ_ALIAS_PRESERVED"));
        }
        let runtime = env::var("XYZ_OPENSSH_BASH_VERSION").expect("fixture Bash runtime");
        let version = t
            .completed(&first, "printf 'XYZ_RUNTIME:%s\\n' \"$BASH_VERSION\"")
            .await;
        assert_eq!(version.exit_code, Some(0));
        let expected_runtime = format!("XYZ_RUNTIME:{runtime}\r\n");
        assert!(t
            .output
            .windows(expected_runtime.len())
            .any(|bytes| bytes == expected_runtime.as_bytes()));
        assert_eq!(
            t.ai_completed(&first, "printf '%s\\n' alpha beta | grep beta")
                .await
                .exit_code,
            Some(0)
        );
        assert_eq!(
            t.ai_completed(&first, "false || printf RECOVERED; true")
                .await
                .exit_code,
            Some(0)
        );
        assert_eq!(t.ai_completed(&first, "false").await.exit_code, Some(1));
        assert_eq!(
            t.ai_completed(&first, "export XYZ_AI_TEST_STATE=preserved")
                .await
                .exit_code,
            Some(0)
        );
        assert_eq!(
            t.ai_completed(&first, "printenv XYZ_AI_TEST_STATE")
                .await
                .exit_code,
            Some(0)
        );
        assert!(t
            .output
            .windows(b"preserved\r\n".len())
            .any(|bytes| bytes == b"preserved\r\n"));
        let initial = t.completed(&first, "pwd").await;
        let cd = t.completed(&first, "cd /tmp; pwd").await;
        assert_eq!(cd.cwd, initial.cwd);
        for command in [
            "printf a | cat",
            "printf '%s\\n' 'one\ntwo'",
            "cat <<'XYZ_END'\nhello\nXYZ_END",
        ] {
            let row = t.completed(&first, command).await;
            assert_eq!(row.cwd, "/tmp");
            assert_eq!(row.exit_code, Some(0));
            assert!(row.started_at_ms > 0 && row.finished_at_ms.unwrap() >= row.started_at_ms);
            assert!(row.remote_started_at.is_some());
        }
        // The modern PS0 adapter also records complete manual subshells.
        assert_eq!(t.completed(&first, "(cd /; pwd)").await.exit_code, Some(0));
        assert_eq!(
            t.completed(&first, "false # manual").await.exit_code,
            Some(1)
        );
        t.output.clear();
        let read = "read -s -p 'XYZ_SECRET_READY' xyz_secret";
        t.write(&first, &format!("{read}\n")).await;
        t.until(|t| t.output.windows(16).any(|s| s == b"XYZ_SECRET_READY"))
            .await;
        let secret = "SYNTHETIC_READ_PASSWORD_NOT_A_COMMAND";
        t.write(&first, &format!("{secret}\n")).await;
        t.until(|t| {
            t.rows(&first)
                .iter()
                .any(|r| r.command == read && r.status == CommandStatus::Completed)
        })
        .await;
        assert!(!t.rows(&first).iter().any(|r| r.command.contains(secret)));
        t.write(&first, "exit\n").await;
        t.until(|t| !t.service.status().enabled).await;
        t.service.flush().unwrap();
        let exit = t
            .rows(&first)
            .into_iter()
            .find(|r| r.command == "exit")
            .unwrap();
        assert_eq!(exit.status, CommandStatus::Unknown);
        assert_eq!(exit.exit_code, None);
        let exported = t
            .service
            .export(ShellLogQuery::default(), ExportFormat::Json)
            .unwrap();
        let data = std::fs::read_to_string(exported.path).unwrap();
        assert!(data.contains("printf a | cat"));
        assert!(!data.contains(secret));
        t.disconnect(&first).await;
        let second = t.connect().await;
        t.completed(&second, "set +o history").await;
        // The prompt-boundary gap belongs to the same sequence as the completed
        // foreground command; it must reach both consumers before more input.
        t.until(|t| t.service.status().state == CaptureState::Gap)
            .await;
        assert_eq!(
            t.manager.shell_integration().status().phase,
            IntegrationPhase::Unknown
        );
        t.write(&second, "echo MUST_NOT_REUSE_PREVIOUS_COMMAND\n")
            .await;
        t.service.flush().unwrap();
        assert_eq!(
            t.rows(&second)
                .iter()
                .filter(|r| r.command == "set +o history")
                .count(),
            1
        );
        assert!(!t
            .rows(&second)
            .iter()
            .any(|r| r.command.contains("MUST_NOT_REUSE")));
        t.disconnect(&second).await;
        let third = t.connect().await;
        t.write(&third, "sleep 30\n").await;
        t.until(|t| t.rows(&third).iter().any(|r| r.command == "sleep 30"))
            .await;
        t.disconnect(&third).await;
        let interrupted = t
            .rows(&third)
            .into_iter()
            .find(|r| r.command == "sleep 30")
            .unwrap();
        assert_eq!(interrupted.status, CommandStatus::Unknown);
        assert_eq!(interrupted.exit_code, None);
        drop(t);
        std::fs::remove_dir_all(root).unwrap();
    })
    .await
    .expect("OpenSSH shell log test exceeded overall deadline");
}
