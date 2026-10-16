//! Real OpenSSH + OS PTY acceptance, explicitly provisioned by
//! `bash .github/scripts/openssh-test.sh --current-user` (Linux), or
//! `bash .github/scripts/openssh-test.sh --disposable-user` (disposable CI).
//! Ordinary `cargo test` reports this test as ignored. An explicit ignored run
//! fails if its fixture is missing; it never silently turns into a passing skip.

#![cfg(unix)]

use std::{env, sync::Arc, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine};
use tokio::{sync::mpsc, time::timeout};
use uuid::Uuid;
use xyz_core::ssh::{
    Authentication, ConnectRequest, HostKeyDecision, SessionManager, SessionRef, SessionState,
    SshEvent, TerminalSize, TrustDecision,
};

const WAIT: Duration = Duration::from_secs(20);

struct Fixture {
    port: u16,
    username: String,
    private_key_path: String,
    fingerprint: String,
}

impl Fixture {
    fn from_env() -> Self {
        let required = |name| {
            env::var(name).unwrap_or_else(|_| {
                panic!("{name} missing: provision the fixture with .github/scripts/openssh-test.sh")
            })
        };
        Self {
            port: required("XYZ_OPENSSH_PORT").parse().expect("valid port"),
            username: required("XYZ_OPENSSH_USER"),
            private_key_path: required("XYZ_OPENSSH_PRIVATE_KEY"),
            fingerprint: required("XYZ_OPENSSH_HOST_FINGERPRINT"),
        }
    }

    fn request(&self) -> ConnectRequest {
        ConnectRequest {
            // Deliberately cannot target a production or externally supplied host.
            host: "127.0.0.1".into(),
            port: self.port,
            username: self.username.clone(),
            auth: Authentication::PrivateKey {
                private_key_path: self.private_key_path.clone(),
                passphrase: None,
            },
            session_only: true,
            size: TerminalSize { cols: 80, rows: 24 },
        }
    }
}

struct Terminal {
    manager: SessionManager,
    incoming: mpsc::UnboundedReceiver<SshEvent>,
    output: Vec<u8>,
    seq: u64,
    exit_status: Option<u32>,
}

impl Terminal {
    fn new() -> Self {
        let (sender, incoming) = mpsc::unbounded_channel();
        Self {
            manager: SessionManager::new(Arc::new(move |event| sender.send(event).is_ok())),
            incoming,
            output: Vec::new(),
            seq: 0,
            exit_status: None,
        }
    }

    async fn event(&mut self) -> SshEvent {
        let event = timeout(WAIT, self.incoming.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "OpenSSH event timeout; state: {:?}; PTY output: {:?}",
                    self.manager.snapshot(),
                    String::from_utf8_lossy(&self.output)
                )
            })
            .expect("event sink closed unexpectedly");
        match &event {
            SshEvent::Output {
                session,
                seq,
                data_base64,
                stream,
            } => {
                assert_eq!(stream, "stdout", "a PTY merges stdout and stderr");
                let next: u64 = seq.parse().unwrap();
                assert_eq!(next, self.seq + 1);
                self.seq = next;
                self.output.extend(STANDARD.decode(data_base64).unwrap());
                self.manager.ack_output(session, seq).unwrap();
            }
            SshEvent::Exit { exit_status, .. } => self.exit_status = Some(*exit_status),
            SshEvent::State { snapshot } => assert_ne!(
                snapshot.state,
                SessionState::Error,
                "OpenSSH session failed: {snapshot:?}"
            ),
            _ => {}
        }
        event
    }

    async fn state(&mut self, state: SessionState) {
        loop {
            if let SshEvent::State { snapshot } = self.event().await {
                if snapshot.state == state {
                    return;
                }
            }
        }
    }

    async fn shell_ready(&mut self) {
        timeout(WAIT, async {
            while self.manager.shell_integration().status().phase
                != xyz_core::shell_integration::IntegrationPhase::Ready
            {
                if let SshEvent::State { snapshot } = self.event().await {
                    assert_ne!(
                        snapshot.state,
                        SessionState::Disconnected,
                        "disconnected during startup; exit={:?}; PTY output={:?}",
                        self.exit_status,
                        String::from_utf8_lossy(&self.output)
                    );
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "no verified initial prompt: {:?}; PTY output={:?}",
                self.manager.shell_integration().status(),
                String::from_utf8_lossy(&self.output)
            )
        });
    }

    async fn output_contains(&mut self, expected: &str) {
        // Bound the whole wait, not just individual reads: continuous unrelated
        // output must not turn a broken marker assertion into an infinite test.
        timeout(WAIT, async {
            while !self
                .output
                .windows(expected.len())
                .any(|bytes| bytes == expected.as_bytes())
            {
                if let SshEvent::State { snapshot } = self.event().await {
                    assert_ne!(
                        snapshot.state,
                        SessionState::Disconnected,
                        "disconnected before expected shell output: {expected:?}; exit={:?}; PTY output={:?}",
                        self.exit_status,
                        String::from_utf8_lossy(&self.output)
                    );
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "missing shell output {expected:?}; PTY output: {:?}",
                String::from_utf8_lossy(&self.output)
            )
        });
    }

    async fn write(&self, session: &SessionRef, command: &str) {
        let receipt = timeout(WAIT, self.manager.write(session, &STANDARD.encode(command)))
            .await
            .expect("terminal write timed out")
            .expect("terminal write failed");
        assert_eq!(receipt.delivery, "write_accepted");
        assert_eq!(receipt.remote_outcome, "unknown");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires real local OpenSSH fixture; run bash .github/scripts/openssh-test.sh"]
async fn openssh_real_pty_shell_unicode_resize_and_disconnect() {
    timeout(Duration::from_secs(90), exercise_real_pty())
        .await
        .expect("OpenSSH acceptance exceeded its overall deadline");
}

async fn exercise_real_pty() {
    let fixture = Fixture::from_env();
    let mut terminal = Terminal::new();
    let first = terminal
        .manager
        .connect(fixture.request())
        .unwrap()
        .session
        .unwrap();
    loop {
        if let SshEvent::HostKey { challenge } = terminal.event().await {
            assert_eq!(challenge.session, first);
            assert_eq!(challenge.host, "127.0.0.1");
            assert_eq!(challenge.port, fixture.port);
            assert_eq!(challenge.algorithm, "ssh-ed25519");
            assert_eq!(challenge.fingerprint, fixture.fingerprint);
            assert!(!challenge.changed);
            assert_eq!(
                terminal.manager.snapshot().state,
                SessionState::AwaitingHostKey
            );
            terminal
                .manager
                .confirm_host_key(HostKeyDecision {
                    challenge_id: challenge.challenge_id,
                    expected_fingerprint: fixture.fingerprint.clone(),
                    decision: TrustDecision::Accept,
                })
                .unwrap();
            break;
        }
    }
    terminal.state(SessionState::Connected).await;
    terminal.shell_ready().await;

    let nonce = Uuid::new_v4().simple().to_string();
    // Split the marker across printf arguments so terminal input echo cannot
    // satisfy the assertion. All commands use the public visible-PTY write API.
    terminal
        .write(
            &first,
            &format!(
                "stty -echo; XYZ_PTY_STATE='{nonce}'; printf '\\n%s%s\\n' 'XYZ_READY_' '{nonce}'; \
                 test -t 0 && test -t 1 && test -t 2 && printf '%s%s\\n' 'XYZ_OS_PTY_' '{nonce}'; \
                 printf '%s%s\\n' 'XYZ_UNICODE_' '中文🙂 café'; \
                 printf '%s' 'XYZ_INITIAL_SIZE:'; stty size\n"
            ),
        )
        .await;
    terminal
        .output_contains(&format!("\nXYZ_READY_{nonce}\r\n"))
        .await;
    terminal
        .output_contains(&format!("\nXYZ_OS_PTY_{nonce}\r\n"))
        .await;
    terminal
        .output_contains("\nXYZ_UNICODE_中文🙂 café\r\n")
        .await;
    terminal.output_contains("XYZ_INITIAL_SIZE:24 80\r\n").await;

    terminal
        .manager
        .resize(
            &first,
            TerminalSize {
                cols: 132,
                rows: 47,
            },
        )
        .unwrap();
    terminal
        .write(
            &first,
            "printf '\\n%s' 'XYZ_RESIZED:'; stty size; printf '%s%s\\n' 'XYZ_SAME_SHELL_' \"$XYZ_PTY_STATE\"\n",
        )
        .await;
    terminal.output_contains("\nXYZ_RESIZED:47 132\r\n").await;
    terminal
        .output_contains(&format!("\nXYZ_SAME_SHELL_{nonce}\r\n"))
        .await;

    terminal
        .write(
            &first,
            &format!("printf '\\n%s%s\\n' 'XYZ_FINAL_' '{nonce}'; exit 7\n"),
        )
        .await;
    terminal
        .output_contains(&format!("\nXYZ_FINAL_{nonce}\r\n"))
        .await;
    terminal.state(SessionState::Disconnected).await;
    assert_eq!(
        terminal.exit_status,
        Some(7),
        "real remote shell exit status"
    );

    terminal.output.clear();
    terminal.seq = 0;
    terminal.exit_status = None;
    let second = terminal
        .manager
        .connect(fixture.request())
        .unwrap()
        .session
        .unwrap();
    assert_ne!(first.session_id, second.session_id);
    assert_eq!(
        second.generation.parse::<u64>().unwrap(),
        first.generation.parse::<u64>().unwrap() + 1
    );
    terminal.state(SessionState::Connected).await;
    terminal.shell_ready().await;
    terminal
        .write(
            &second,
            &format!("stty -echo; printf '\\n%s%s\\n' 'XYZ_RECONNECTED_' '{nonce}'\n"),
        )
        .await;
    terminal
        .output_contains(&format!("\nXYZ_RECONNECTED_{nonce}\r\n"))
        .await;
    assert_eq!(
        terminal
            .manager
            .write(&first, &STANDARD.encode(b"stale\n"))
            .await
            .unwrap_err()
            .code,
        "SESSION_STALE"
    );
    terminal.manager.disconnect(&second).unwrap();
    terminal.state(SessionState::Disconnected).await;
    assert_eq!(
        terminal
            .manager
            .write(&second, &STANDARD.encode(b"late\n"))
            .await
            .unwrap_err()
            .code,
        "SESSION_NOT_CONNECTED"
    );
}
