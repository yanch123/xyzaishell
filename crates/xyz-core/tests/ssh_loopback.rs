//! Independent integration checks against an in-process SSH protocol server.
//! These validate core SSH transport behavior, not an operating-system shell,
//! desktop rendering, OpenSSH interoperability, packaging, or credential stores.

mod support;

use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine};
use support::{Behavior, LoopbackServer, BANNER, PASSWORD, USER, WAIT};
use tokio::{sync::mpsc, time::timeout};
use xyz_core::ssh::{
    Authentication, ConnectRequest, HostKeyChallenge, HostKeyDecision, Secret, SessionManager,
    SessionRef, SessionSnapshot, SessionState, SshEvent, TerminalSize, TrustDecision,
    MAX_INPUT_BYTES,
};

struct Client {
    manager: SessionManager,
    incoming: mpsc::UnboundedReceiver<SshEvent>,
    events: Vec<SshEvent>,
    output: Vec<u8>,
    output_seq: u64,
    acknowledge_output: bool,
}

impl Client {
    fn new() -> Self {
        let (sender, incoming) = mpsc::unbounded_channel();
        Self {
            manager: SessionManager::new(Arc::new(move |event| sender.send(event).is_ok())),
            incoming,
            events: Vec::new(),
            output: Vec::new(),
            output_seq: 0,
            acknowledge_output: true,
        }
    }

    fn persistent(root: Result<std::path::PathBuf, xyz_core::ssh::ApiError>) -> Self {
        let (sender, incoming) = mpsc::unbounded_channel();
        Self {
            manager: SessionManager::with_persistent_trust(
                Arc::new(move |event| sender.send(event).is_ok()),
                root,
            ),
            incoming,
            events: Vec::new(),
            output: Vec::new(),
            output_seq: 0,
            acknowledge_output: true,
        }
    }

    fn request(server: &LoopbackServer, password: &str) -> ConnectRequest {
        ConnectRequest {
            host: server.addr.ip().to_string(),
            port: server.addr.port(),
            username: USER.into(),
            auth: Authentication::Password {
                password: Secret::new(password.into()),
            },
            session_only: true,
            size: TerminalSize { cols: 80, rows: 24 },
        }
    }

    fn connect(&self, server: &LoopbackServer) -> SessionRef {
        let snapshot = self
            .manager
            .connect(Self::request(server, PASSWORD))
            .unwrap();
        assert_eq!(snapshot.state, SessionState::Connecting);
        snapshot.session.unwrap()
    }

    async fn event(&mut self) -> SshEvent {
        let event = timeout(WAIT, self.incoming.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "client event timeout; snapshot: {:?}; prior events: {:?}",
                    self.manager.snapshot(),
                    self.events
                )
            })
            .expect("event sink unexpectedly closed");
        if let SshEvent::Output {
            session,
            seq,
            data_base64,
            stream,
        } = &event
        {
            assert_eq!(stream, "stdout");
            let seq_number: u64 = seq.parse().unwrap();
            assert_eq!(
                seq_number,
                self.output_seq + 1,
                "output cursor must be contiguous within a session"
            );
            self.output_seq = seq_number;
            self.output.extend(
                STANDARD
                    .decode(data_base64)
                    .expect("terminal output is valid base64"),
            );
            // The actual UI acknowledges rendered bytes. Tests acknowledge only
            // after decoding and collecting the exact published bytes.
            if self.acknowledge_output {
                self.manager.ack_output(session, seq).unwrap();
            }
        }
        self.events.push(event.clone());
        event
    }

    async fn challenge(&mut self) -> HostKeyChallenge {
        loop {
            match self.event().await {
                SshEvent::HostKey { challenge } => return challenge,
                SshEvent::State { snapshot } if snapshot.state == SessionState::Error => {
                    panic!("connection failed before host-key review: {snapshot:?}")
                }
                _ => {}
            }
        }
    }

    fn decide(&self, challenge: &HostKeyChallenge, decision: TrustDecision) {
        self.manager
            .confirm_host_key(HostKeyDecision {
                challenge_id: challenge.challenge_id.clone(),
                expected_fingerprint: challenge.fingerprint.clone(),
                decision,
            })
            .unwrap();
    }

    async fn state(&mut self, desired: SessionState) -> SessionSnapshot {
        loop {
            if let SshEvent::State { snapshot } = self.event().await {
                if snapshot.state == desired {
                    return snapshot;
                }
                assert_ne!(
                    snapshot.state,
                    SessionState::Error,
                    "unexpected SSH error: {snapshot:?}"
                );
            }
        }
    }

    async fn trusted_connection(&mut self, server: &LoopbackServer) -> SessionRef {
        let prior_authentications = server.trace().authentications.len();
        let session = self.connect(server);
        let challenge = self.challenge().await;
        assert_eq!(challenge.session, session);
        assert_eq!(challenge.host, "127.0.0.1");
        assert_eq!(challenge.port, server.addr.port());
        assert_eq!(challenge.algorithm, "ssh-ed25519");
        assert_eq!(challenge.fingerprint, server.fingerprint);
        assert!(!challenge.changed);
        assert!(challenge.previous_fingerprint.is_none());
        assert_eq!(self.manager.snapshot().state, SessionState::AwaitingHostKey);
        assert_eq!(
            server.trace().authentications.len(),
            prior_authentications,
            "credentials must not be sent before host-key approval"
        );
        self.decide(&challenge, TrustDecision::Accept);
        let connected = self.state(SessionState::Connected).await;
        assert_eq!(connected.session.as_ref(), Some(&session));
        session
    }

    async fn output_len(&mut self, size: usize) {
        while self.output.len() < size {
            self.event().await;
        }
    }

    async fn disconnect(&mut self, session: &SessionRef) {
        self.manager.disconnect(session).unwrap();
        self.state(SessionState::Disconnected).await;
    }

    fn assert_never_connected(&self) {
        assert!(!self.events.iter().any(|e| matches!(e, SshEvent::State { snapshot } if snapshot.state == SessionState::Connected)));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_host_trust_then_same_pty_binary_io_resize_and_disconnect() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    let session = client.trusted_connection(&server).await;
    client.output_len(BANNER.len()).await;
    assert_eq!(client.output, BANNER);

    let payload = b"echo \xe4\xb8\xad\xe6\x96\x87\r\n\x00\xff\x1b[31m\x03";
    let receipt = timeout(
        WAIT,
        client.manager.write(&session, &STANDARD.encode(payload)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(receipt.transport_seq, "1");
    assert_eq!(receipt.delivery, "write_accepted");
    assert_eq!(
        receipt.remote_outcome, "unknown",
        "transport acceptance is not command success"
    );
    client.output_len(BANNER.len() + payload.len()).await;
    assert_eq!(&client.output[BANNER.len()..], payload);
    client
        .manager
        .resize(
            &session,
            TerminalSize {
                cols: 132,
                rows: 47,
            },
        )
        .unwrap();
    let trace = server
        .wait_for("server never received resize", |t| !t.resizes.is_empty())
        .await;
    assert_eq!(trace.authentications, [true]);
    assert_eq!(trace.channels.len(), 1);
    let channel = trace.channels[0];
    assert_eq!(trace.ptys, [(channel, "xterm-256color".into(), 80, 24)]);
    assert_eq!(trace.shells, [channel]);
    assert_eq!(trace.input, [(channel, payload.to_vec())]);
    assert_eq!(trace.resizes, [(channel, 132, 47)]);
    assert_eq!(
        trace.exec_requests, 0,
        "all terminal traffic must use the visible shell PTY"
    );
    client.disconnect(&session).await;
    assert_eq!(
        client
            .manager
            .write(&session, &STANDARD.encode(b"late"))
            .await
            .unwrap_err()
            .code,
        "SESSION_NOT_CONNECTED"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_host_key_never_sends_authentication_or_opens_pty() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    client.connect(&server);
    let challenge = client.challenge().await;
    client.decide(&challenge, TrustDecision::Reject);
    let failed = client.state(SessionState::Error).await;
    assert_eq!(failed.error.unwrap().code, "SSH_HANDSHAKE_FAILED");
    client.assert_never_connected();
    assert!(server.trace().authentications.is_empty());
    assert!(server.trace().ptys.is_empty());
    assert!(client.output.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_key_decision_is_bound_to_challenge_and_fingerprint_and_single_use() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    let session = client.connect(&server);
    let challenge = client.challenge().await;
    for (id, fingerprint) in [
        (
            "not-the-current-challenge".into(),
            challenge.fingerprint.clone(),
        ),
        (
            challenge.challenge_id.clone(),
            "SHA256:not-the-server-key".into(),
        ),
    ] {
        let error = client
            .manager
            .confirm_host_key(HostKeyDecision {
                challenge_id: id,
                expected_fingerprint: fingerprint,
                decision: TrustDecision::Accept,
            })
            .unwrap_err();
        assert_eq!(error.code, "HOST_KEY_STALE");
        assert_eq!(
            client.manager.snapshot().state,
            SessionState::AwaitingHostKey
        );
        assert!(server.trace().authentications.is_empty());
    }
    client.decide(&challenge, TrustDecision::Accept);
    assert_eq!(
        client
            .manager
            .confirm_host_key(HostKeyDecision {
                challenge_id: challenge.challenge_id,
                expected_fingerprint: challenge.fingerprint,
                decision: TrustDecision::Accept
            })
            .unwrap_err()
            .code,
        "HOST_KEY_STALE"
    );
    client.state(SessionState::Connected).await;
    client.output_len(BANNER.len()).await;
    client.disconnect(&session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_password_fails_without_pty_or_secret_in_events() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    let wrong = "synthetic-wrong-password-do-not-echo";
    client
        .manager
        .connect(Client::request(&server, wrong))
        .unwrap();
    let challenge = client.challenge().await;
    client.decide(&challenge, TrustDecision::Accept);
    let failed = client.state(SessionState::Error).await;
    assert_eq!(failed.error.unwrap().code, "SSH_AUTH_FAILED");
    client.assert_never_connected();
    let trace = server.trace();
    assert_eq!(trace.authentications, [false]);
    assert!(trace.channels.is_empty());
    assert!(trace.ptys.is_empty());
    let encoded_events = serde_json::to_string(&client.events).unwrap();
    assert!(!encoded_events.contains(wrong));
    assert!(!encoded_events.contains(PASSWORD));
    assert!(client.output.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_pty_rejection_is_not_reported_as_connected() {
    refused_startup(
        Behavior {
            accept_pty: false,
            accept_shell: true,
        },
        "SSH_PTY_FAILED",
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_shell_rejection_is_not_reported_as_connected() {
    refused_startup(
        Behavior {
            accept_pty: true,
            accept_shell: false,
        },
        "SSH_SHELL_FAILED",
        true,
    )
    .await;
}

async fn refused_startup(behavior: Behavior, code: &str, shell_requested: bool) {
    let server = LoopbackServer::start(behavior).await;
    let mut client = Client::new();
    client.connect(&server);
    let challenge = client.challenge().await;
    client.decide(&challenge, TrustDecision::Accept);
    let failed = client.state(SessionState::Error).await;
    assert_eq!(failed.error.unwrap().code, code);
    client.assert_never_connected();
    assert_eq!(server.trace().ptys.len(), 1);
    assert_eq!(!server.trace().shells.is_empty(), shell_requested);
    assert!(client.output.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_live_session_reconnects_with_new_identity_and_rejects_stale_requests() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    let first = client.connect(&server);
    assert_eq!(
        client
            .manager
            .connect(Client::request(&server, PASSWORD))
            .unwrap_err()
            .code,
        "SESSION_BUSY"
    );
    let challenge = client.challenge().await;
    client.decide(&challenge, TrustDecision::Accept);
    client.state(SessionState::Connected).await;
    client.output_len(BANNER.len()).await;
    assert_eq!(
        client
            .manager
            .connect(Client::request(&server, PASSWORD))
            .unwrap_err()
            .code,
        "SESSION_BUSY"
    );
    client.disconnect(&first).await;

    client.output_seq = 0;
    client.output.clear();
    let second = client.connect(&server);
    assert_ne!(first.session_id, second.session_id);
    assert_eq!(
        second.generation.parse::<u64>().unwrap(),
        first.generation.parse::<u64>().unwrap() + 1
    );
    // The same in-memory host key remains trusted for this manager lifetime.
    client.state(SessionState::Connected).await;
    client.output_len(BANNER.len()).await;
    assert_eq!(
        client
            .manager
            .write(&first, &STANDARD.encode(b"stale input"))
            .await
            .unwrap_err()
            .code,
        "SESSION_STALE"
    );
    assert_eq!(
        client
            .manager
            .resize(
                &first,
                TerminalSize {
                    cols: 100,
                    rows: 30
                }
            )
            .unwrap_err()
            .code,
        "SESSION_STALE"
    );
    assert_eq!(
        client.manager.disconnect(&first).unwrap_err().code,
        "SESSION_STALE"
    );
    let current_before_ack = serde_json::to_value(client.manager.snapshot()).unwrap();
    client.manager.ack_output(&first, "1").unwrap();
    assert_eq!(
        serde_json::to_value(client.manager.snapshot()).unwrap(),
        current_before_ack
    );
    assert!(server.trace().input.is_empty());
    assert!(server.trace().resizes.is_empty());
    client
        .manager
        .write(&second, &STANDARD.encode(b"current input"))
        .await
        .unwrap();
    client
        .output_len(BANNER.len() + b"current input".len())
        .await;
    assert_eq!(&client.output[BANNER.len()..], b"current input");
    let trace = server.trace();
    assert_eq!(trace.connections, 2);
    assert_eq!(trace.shells.len(), 2);
    assert_eq!(trace.input.len(), 1);
    client.disconnect(&second).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_host_key_review_invalidates_approval_and_leaks_no_credentials() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    let session = client.connect(&server);
    let challenge = client.challenge().await;
    client.disconnect(&session).await;
    assert_eq!(
        client
            .manager
            .confirm_host_key(HostKeyDecision {
                challenge_id: challenge.challenge_id,
                expected_fingerprint: challenge.fingerprint,
                decision: TrustDecision::Accept
            })
            .unwrap_err()
            .code,
        "HOST_KEY_STALE"
    );
    assert!(server.trace().authentications.is_empty());
    assert!(server.trace().shells.is_empty());
    client.assert_never_connected();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_disconnect_ends_session_and_does_not_replay_written_input() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    let session = client.trusted_connection(&server).await;
    client.output_len(BANNER.len()).await;
    let receipt = client
        .manager
        .write(&session, &STANDARD.encode(b"one write"))
        .await
        .unwrap();
    assert_eq!(receipt.remote_outcome, "unknown");
    client.output_len(BANNER.len() + b"one write".len()).await;
    server.drop_connections().await;
    client.state(SessionState::Disconnected).await;
    assert_eq!(
        client
            .manager
            .write(&session, &STANDARD.encode(b"late write"))
            .await
            .unwrap_err()
            .code,
        "SESSION_NOT_CONNECTED"
    );
    assert_eq!(server.trace().input.len(), 1);
    assert_eq!(server.trace().connections, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_limits_and_bad_resize_fail_before_transmission() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    let session = client.trusted_connection(&server).await;
    client.output_len(BANNER.len()).await;
    assert_eq!(
        client
            .manager
            .write(&session, "!!not-base64!!")
            .await
            .unwrap_err()
            .code,
        "INPUT_INVALID"
    );
    assert_eq!(
        client.manager.write(&session, "").await.unwrap_err().code,
        "INPUT_TOO_LARGE"
    );
    assert_eq!(
        client
            .manager
            .write(&session, &STANDARD.encode(vec![b'x'; MAX_INPUT_BYTES + 1]))
            .await
            .unwrap_err()
            .code,
        "INPUT_TOO_LARGE"
    );
    for size in [
        TerminalSize { cols: 0, rows: 24 },
        TerminalSize {
            cols: 80,
            rows: 1001,
        },
    ] {
        assert_eq!(
            client.manager.resize(&session, size).unwrap_err().code,
            "INPUT_INVALID"
        );
    }
    let bytes: Vec<u8> = (0..MAX_INPUT_BYTES).map(|i| (i % 256) as u8).collect();
    client
        .manager
        .write(&session, &STANDARD.encode(&bytes))
        .await
        .unwrap();
    client.output_len(BANNER.len() + bytes.len()).await;
    assert_eq!(&client.output[BANNER.len()..], bytes);
    let trace = server.trace();
    assert_eq!(
        trace.input.iter().map(|(_, b)| b.len()).sum::<usize>(),
        MAX_INPUT_BYTES
    );
    assert!(trace.resizes.is_empty());
    client.disconnect(&session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_host_key_requires_new_explicit_approval_and_rejection_preserves_old_trust() {
    let mut original = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    let session = client.trusted_connection(&original).await;
    client.output_len(BANNER.len()).await;
    client.disconnect(&session).await;
    original
        .wait_for("first socket did not close", |t| t.connections_closed == 1)
        .await;
    original.stop().await;

    let replacement = LoopbackServer::start_at(Behavior::default(), original.addr).await;
    assert_ne!(replacement.fingerprint, original.fingerprint);
    client.connect(&replacement);
    let changed = client.challenge().await;
    assert!(changed.changed);
    assert_eq!(
        changed.previous_fingerprint.as_deref(),
        Some(original.fingerprint.as_str())
    );
    assert_eq!(changed.fingerprint, replacement.fingerprint);
    assert!(replacement.trace().authentications.is_empty());
    client.decide(&changed, TrustDecision::Reject);
    assert_eq!(
        client.state(SessionState::Error).await.error.unwrap().code,
        "SSH_HANDSHAKE_FAILED"
    );
    assert!(replacement.trace().authentications.is_empty());

    client.output_seq = 0;
    client.output.clear();
    let replacement_session = client.connect(&replacement);
    let reviewed_again = client.challenge().await;
    assert!(reviewed_again.changed);
    assert_eq!(
        reviewed_again.previous_fingerprint,
        changed.previous_fingerprint
    );
    assert_ne!(reviewed_again.challenge_id, changed.challenge_id);
    client.decide(&reviewed_again, TrustDecision::Accept);
    client.state(SessionState::Connected).await;
    client.output_len(BANNER.len()).await;
    client.disconnect(&replacement_session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unread_output_is_bounded_and_disconnect_remains_responsive() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    let session = client.trusted_connection(&server).await;
    client.output_len(BANNER.len()).await;
    client.acknowledge_output = false;
    server.send_output(vec![b'x'; 512 * 1024]).await;
    client.output_len(BANNER.len() + 256 * 1024).await;
    assert_eq!(client.output.len(), BANNER.len() + 256 * 1024);
    // No acknowledgements release the 256-KiB output window. Cancellation
    // must interrupt a blocked publisher rather than wait for UI consumption.
    client.disconnect(&session).await;
    assert_eq!(
        client.output.len(),
        BANNER.len() + 256 * 1024,
        "output window exceeded its bound without acknowledgements"
    );
    server
        .wait_for("cancelled output stream left a socket alive", |t| {
            t.connections_closed == 1
        })
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trust_is_scoped_to_one_manager_lifetime() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut first = Client::new();
    let session = first.trusted_connection(&server).await;
    first.output_len(BANNER.len()).await;
    first.disconnect(&session).await;
    let mut second = Client::new();
    let session = second.trusted_connection(&server).await;
    second.output_len(BANNER.len()).await;
    second.disconnect(&session).await;
    assert_eq!(server.trace().connections, 2);
}

/// Synthetic per-test key material exists only in a restricted temporary file
/// because the public M1 API intentionally accepts a user-selected path. It is
/// never a repository fixture and is removed even when an assertion unwinds.
#[cfg(unix)]
struct TemporaryKey {
    directory: std::path::PathBuf,
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl TemporaryKey {
    fn create(key: &russh::keys::PrivateKey) -> Self {
        use std::io::Write;
        let directory =
            std::env::temp_dir().join(format!("xyz-loopback-key-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory).unwrap();
        let path = directory.join("ephemeral_key");
        let key_file = Self { directory, path };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let encoded = key
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        options
            .open(&key_file.path)
            .unwrap()
            .write_all(encoded.as_bytes())
            .unwrap();
        key_file
    }
}

#[cfg(unix)]
impl Drop for TemporaryKey {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ephemeral_private_key_authenticates_and_opens_the_same_shell_pty() {
    private_key_authentication(false).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn encrypted_ephemeral_private_key_authenticates_with_session_only_passphrase() {
    private_key_authentication(true).await;
}

#[cfg(unix)]
async fn private_key_authentication(encrypt: bool) {
    use russh::keys::{Algorithm, PrivateKey};
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let server = LoopbackServer::start_for_public_key(key.public_key().clone()).await;
    let passphrase = "synthetic-key-passphrase-never-a-user-secret";
    let stored_key = if encrypt {
        key.encrypt(&mut rand::rng(), passphrase).unwrap()
    } else {
        key
    };
    let temporary_key = TemporaryKey::create(&stored_key);
    let mut client = Client::new();
    let mut request = Client::request(&server, PASSWORD);
    request.auth = Authentication::PrivateKey {
        private_key_path: temporary_key.path.to_str().unwrap().into(),
        passphrase: encrypt.then(|| Secret::new(passphrase.into())),
    };
    let session = client.manager.connect(request).unwrap().session.unwrap();
    let challenge = client.challenge().await;
    assert_eq!(challenge.fingerprint, server.fingerprint);
    assert!(server.trace().authentications.is_empty());
    client.decide(&challenge, TrustDecision::Accept);
    client.state(SessionState::Connected).await;
    client.output_len(BANNER.len()).await;
    client
        .manager
        .write(&session, &STANDARD.encode(b"key-auth input"))
        .await
        .unwrap();
    client
        .output_len(BANNER.len() + b"key-auth input".len())
        .await;
    assert_eq!(&client.output[BANNER.len()..], b"key-auth input");
    let trace = server.trace();
    assert_eq!(trace.authentications, [true]);
    assert_eq!(trace.public_key_authentications, 1);
    assert_eq!(trace.channels.len(), 1);
    assert_eq!(trace.ptys.len(), 1);
    assert_eq!(trace.shells.len(), 1);
    assert_eq!(trace.exec_requests, 0);
    let events = serde_json::to_string(&client.events).unwrap();
    assert!(!events.contains(passphrase));
    assert!(!events.contains("OPENSSH PRIVATE KEY"));
    assert!(!events.contains(temporary_key.path.to_str().unwrap()));
    client.disconnect(&session).await;
    let path = temporary_key.path.clone();
    let directory = temporary_key.directory.clone();
    drop(temporary_key);
    assert!(!path.exists(), "temporary private key was not removed");
    assert!(
        !directory.exists(),
        "temporary key directory was not removed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn final_output_and_exit_status_survive_close_and_delayed_render_ack() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    let session = client.trusted_connection(&server).await;
    client.output_len(BANNER.len()).await;
    client.acknowledge_output = false;
    let final_bytes = b"last bytes before shell exit\r\n";
    server.finish_shell(final_bytes.to_vec(), 17).await;
    client.state(SessionState::Disconnected).await;
    assert_eq!(&client.output[BANNER.len()..], final_bytes);
    let exits: Vec<_> = client
        .events
        .iter()
        .filter_map(|event| match event {
            SshEvent::Exit {
                session,
                exit_status,
            } => Some((session, *exit_status)),
            _ => None,
        })
        .collect();
    assert_eq!(exits, [(&session, 17)]);
    // Rendering is asynchronous. Acknowledging an already delivered final
    // frame after its session closes must be harmless to the current session.
    client
        .manager
        .ack_output(&session, &client.output_seq.to_string())
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_input_frames_preserve_accepted_transport_order_without_interleaving() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut client = Client::new();
    let session = client.trusted_connection(&server).await;
    client.output_len(BANNER.len()).await;
    let mut tasks = Vec::new();
    for index in 0u8..32 {
        let manager = client.manager.clone();
        let reference = session.clone();
        tasks.push(tokio::spawn(async move {
            let bytes = vec![index; 1024];
            let result = manager.write(&reference, &STANDARD.encode(&bytes)).await;
            (bytes, result)
        }));
    }
    let mut accepted = Vec::new();
    let mut rejected = 0;
    for task in tasks {
        let (bytes, result) = timeout(WAIT, task)
            .await
            .expect("concurrent write stalled")
            .expect("write task panicked");
        match result {
            Ok(receipt) => {
                assert_eq!(receipt.remote_outcome, "unknown");
                accepted.push((receipt.transport_seq.parse::<u64>().unwrap(), bytes));
            }
            Err(error) => {
                assert_eq!(
                    error.code, "BACKPRESSURE",
                    "an unaccepted frame must fail explicitly"
                );
                rejected += 1;
            }
        }
    }
    assert!(!accepted.is_empty());
    assert_eq!(accepted.len() + rejected, 32);
    accepted.sort_by_key(|(seq, _)| *seq);
    for (index, (seq, _)) in accepted.iter().enumerate() {
        assert_eq!(*seq, index as u64 + 1);
    }
    let expected: Vec<u8> = accepted.into_iter().flat_map(|(_, bytes)| bytes).collect();
    client.output_len(BANNER.len() + expected.len()).await;
    assert_eq!(&client.output[BANNER.len()..], expected);
    let trace = server.trace();
    let observed: Vec<u8> = trace
        .input
        .into_iter()
        .flat_map(|(_, bytes)| bytes)
        .collect();
    assert_eq!(observed, expected, "rejected frames must not reach the SSH server, and accepted frames must keep transport order");
    assert_eq!(trace.channels.len(), 1);
    client.disconnect(&session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistent_trust_survives_manager_restart_without_reapproving_exact_target() {
    let root = std::env::temp_dir().join(format!("xyz-trust-integration-{}", uuid::Uuid::new_v4()));
    let server = LoopbackServer::start(Behavior::default()).await;
    let mut first = Client::persistent(Ok(root.clone()));
    let session = first.trusted_connection(&server).await;
    first.disconnect(&session).await;
    drop(first);
    let mut second = Client::persistent(Ok(root.clone()));
    let session = second.connect(&server);
    second.state(SessionState::Connected).await;
    assert!(!second
        .events
        .iter()
        .any(|event| matches!(event, SshEvent::HostKey { .. })));
    second.disconnect(&session).await;
    drop(second);
    let text = std::fs::read_to_string(root.join("known_hosts")).unwrap();
    assert!(text.contains(&server.fingerprint));
    assert!(!text.contains(PASSWORD));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_persistent_trust_never_falls_back_or_authenticates() {
    let server = LoopbackServer::start(Behavior::default()).await;
    let client = Client::persistent(Err(xyz_core::ssh::ApiError::new(
        "STORAGE_UNAVAILABLE",
        "Secure trust storage unavailable.",
    )));
    assert_eq!(
        client
            .manager
            .connect(Client::request(&server, PASSWORD))
            .unwrap_err()
            .code,
        "STORAGE_UNAVAILABLE"
    );
    assert!(server.trace().authentications.is_empty());
    assert!(!client
        .events
        .iter()
        .any(|event| matches!(event, SshEvent::HostKey { .. })));
}
