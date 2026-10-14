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
        self.output.clear();
        self.write(
            &session,
            "stty -echo; printf '%s%s\\n' XYZ_LOG_READY_ ACK\n",
        )
        .await;
        self.until(|t| {
            t.output
                .windows(b"XYZ_LOG_READY_ACK\r\n".len())
                .any(|s| s == b"XYZ_LOG_READY_ACK\r\n")
        })
        .await;
        let bootstrap = self.service.start(self.manager.snapshot(), true).unwrap();
        self.write(&session, &bootstrap).await;
        self.until(|t| t.service.status().state == CaptureState::Recording)
            .await;
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
    async fn disconnect(&mut self, session: &SessionRef) {
        self.manager.disconnect(session).unwrap();
        self.until(|t| t.manager.snapshot().state == SessionState::Disconnected)
            .await;
        self.service.flush().unwrap();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires real local OpenSSH fixture; run .github/scripts/openssh-test.sh"]
async fn shell_log_real_openssh_records_commands_without_model() {
    timeout(Duration::from_secs(120), async {
        let root = env::temp_dir().join(format!("xyz-shell-log-openssh-{}", Uuid::new_v4()));
        let mut t = Terminal::new(root.clone());
        let first = t.connect().await;
        let initial = t.completed(&first, "pwd").await;
        let cd = t.completed(&first, "cd /tmp; pwd").await;
        assert_eq!(cd.cwd, initial.cwd);
        for command in [
            "printf a | cat",
            "(cd /; pwd)",
            "printf '%s\\n' 'one\ntwo'",
            "cat <<'XYZ_END'\nhello\nXYZ_END",
        ] {
            let row = t.completed(&first, command).await;
            assert_eq!(row.cwd, "/tmp");
            assert_eq!(row.exit_code, Some(0));
            assert!(row.started_at_ms > 0 && row.finished_at_ms.unwrap() >= row.started_at_ms);
            assert!(row.remote_started_at.is_some());
        }
        assert_eq!(t.completed(&first, "false").await.exit_code, Some(1));
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
        t.write(&second, "echo MUST_NOT_REUSE_PREVIOUS_COMMAND\n")
            .await;
        t.until(|t| t.service.status().state == CaptureState::Gap)
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
