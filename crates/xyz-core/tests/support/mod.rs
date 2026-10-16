//! A real SSH protocol fixture. It binds only loopback, creates a fresh host key
//! in memory for every test, and never invokes a local shell or reads user keys.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use russh::{
    keys::{Algorithm, HashAlg, PrivateKey, PublicKey},
    server::{self, Msg, Server as _, Session},
    Channel, ChannelId, Pty,
};
use tokio::{
    net::TcpListener,
    sync::{oneshot, watch},
    task::JoinHandle,
    time::timeout,
};

pub const WAIT: Duration = Duration::from_secs(10);
pub const USER: &str = "fixture-user";
pub const PASSWORD: &str = "synthetic-test-password-never-a-user-secret";
pub const BANNER: &[u8] = b"fixture PTY ready\r\n";

#[derive(Clone, Debug, Default)]
pub struct Trace {
    pub connections: usize,
    pub connections_closed: usize,
    pub authentications: Vec<bool>,
    pub public_key_authentications: usize,
    pub channels: Vec<ChannelId>,
    pub ptys: Vec<(ChannelId, String, u32, u32)>,
    pub shells: Vec<ChannelId>,
    pub input: Vec<(ChannelId, Vec<u8>)>,
    pub resizes: Vec<(ChannelId, u32, u32)>,
    pub closes: Vec<ChannelId>,
    pub exec_requests: usize,
    pub exec_commands: Vec<(ChannelId, Vec<u8>)>,
}

#[derive(Clone, Copy, Debug)]
pub struct Behavior {
    pub accept_pty: bool,
    pub accept_exec: bool,
    pub accept_shell: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            accept_pty: true,
            accept_exec: true,
            accept_shell: true,
        }
    }
}

#[derive(Clone)]
struct FixtureHandler {
    trace: watch::Sender<Trace>,
    handles: Arc<Mutex<Vec<server::Handle>>>,
    behavior: Behavior,
    is_connection: bool,
    allowed_public_key: Option<PublicKey>,
}

impl server::Server for FixtureHandler {
    type Handler = Self;
    fn new_client(&mut self, peer: Option<SocketAddr>) -> Self::Handler {
        assert!(peer.expect("accepted TCP peer").ip().is_loopback());
        self.trace.send_modify(|t| t.connections += 1);
        let mut connection = self.clone();
        connection.is_connection = true;
        connection
    }
}

impl Drop for FixtureHandler {
    fn drop(&mut self) {
        if self.is_connection {
            self.trace.send_modify(|t| t.connections_closed += 1);
        }
    }
}

impl server::Handler for FixtureHandler {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: &PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        let valid = user == USER && self.allowed_public_key.as_ref() == Some(key);
        self.trace.send_modify(|t| {
            t.authentications.push(valid);
            t.public_key_authentications += 1;
        });
        Ok(if valid {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }

    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        let valid = user == USER && password == PASSWORD;
        self.trace.send_modify(|t| t.authentications.push(valid));
        Ok(if valid {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.handles.lock().unwrap().push(session.handle());
        self.trace.send_modify(|t| t.channels.push(channel.id()));
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.trace
            .send_modify(|t| t.ptys.push((channel, term.to_owned(), cols, rows)));
        if self.behavior.accept_pty {
            session.channel_success(channel)?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.trace.send_modify(|t| t.shells.push(channel));
        if self.behavior.accept_shell {
            session.channel_success(channel)?;
            session.data(channel, BANNER.to_vec())?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.trace.send_modify(|t| {
            t.exec_requests += 1;
            t.exec_commands.push((channel, command.to_vec()));
        });
        // The production startup is one exec on the already allocated visible
        // PTY. This fixture records it without interpreting or executing it.
        if self.behavior.accept_exec {
            session.channel_success(channel)?;
            session.data(channel, BANNER.to_vec())?;
        } else {
            // An explicit failure means no remote program has started, so a
            // plain manual-shell request on this same channel is safe.
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.trace
            .send_modify(|t| t.input.push((channel, data.to_vec())));
        session.data(channel, data.to_vec())?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.trace
            .send_modify(|t| t.resizes.push((channel, cols, rows)));
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.trace.send_modify(|t| t.closes.push(channel));
        Ok(())
    }
}

pub struct LoopbackServer {
    pub addr: SocketAddr,
    pub fingerprint: String,
    trace: watch::Receiver<Trace>,
    handles: Arc<Mutex<Vec<server::Handle>>>,
    shutdown: server::RunningServerHandle,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl LoopbackServer {
    pub async fn start(behavior: Behavior) -> Self {
        Self::start_at(
            behavior,
            SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
        )
        .await
    }

    pub async fn start_at(behavior: Behavior, address: SocketAddr) -> Self {
        Self::start_at_with_public_key(behavior, address, None).await
    }

    #[cfg(unix)]
    pub async fn start_for_public_key(key: PublicKey) -> Self {
        Self::start_at_with_public_key(
            Behavior::default(),
            SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
            Some(key),
        )
        .await
    }

    async fn start_at_with_public_key(
        behavior: Behavior,
        address: SocketAddr,
        allowed_public_key: Option<PublicKey>,
    ) -> Self {
        assert!(address.ip().is_loopback());
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            inactivity_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        });
        let listener = TcpListener::bind(address).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (sender, trace) = watch::channel(Trace::default());
        let handles = Arc::new(Mutex::new(Vec::new()));
        let mut handler = FixtureHandler {
            trace: sender,
            handles: handles.clone(),
            behavior,
            is_connection: false,
            allowed_public_key,
        };
        let (ready_tx, ready_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let running = handler.run_on_socket(config, &listener);
            ready_tx
                .send(running.handle())
                .unwrap_or_else(|_| panic!("fixture startup receiver dropped"));
            running.await
        });
        let shutdown = timeout(WAIT, ready_rx)
            .await
            .expect("fixture startup timed out")
            .unwrap();
        Self {
            addr,
            fingerprint,
            trace,
            handles,
            shutdown,
            task: Some(task),
        }
    }

    pub fn trace(&self) -> Trace {
        self.trace.borrow().clone()
    }

    pub async fn wait_for(&self, message: &str, predicate: impl Fn(&Trace) -> bool) -> Trace {
        let mut receiver = self.trace.clone();
        timeout(WAIT, async {
            loop {
                let observed = receiver.borrow_and_update().clone();
                if predicate(&observed) {
                    return observed;
                }
                receiver
                    .changed()
                    .await
                    .expect("fixture trace unexpectedly closed");
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{message}; last server trace: {:?}", self.trace()))
    }

    pub async fn stop(&mut self) {
        self.shutdown.shutdown("test restart".into());
        if let Some(task) = self.task.take() {
            timeout(WAIT, task)
                .await
                .expect("fixture stop timed out")
                .expect("fixture task panicked")
                .expect("fixture server failed");
        }
    }

    pub async fn send_output(&self, bytes: Vec<u8>) {
        let handle = self
            .handles
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("no SSH channel");
        let channel = *self.trace.borrow().channels.last().expect("no SSH channel");
        timeout(WAIT, handle.data(channel, bytes))
            .await
            .expect("fixture output blocked")
            .expect("fixture output failed");
    }

    pub async fn finish_shell(&self, output: Vec<u8>, status: u32) {
        let handle = self
            .handles
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("no SSH channel");
        let channel = *self.trace.borrow().channels.last().expect("no SSH channel");
        timeout(WAIT, async {
            handle
                .data(channel, output)
                .await
                .expect("final output failed");
            handle
                .exit_status_request(channel, status)
                .await
                .expect("exit status failed");
            handle.eof(channel).await.expect("EOF failed");
            handle.close(channel).await.expect("channel close failed");
        })
        .await
        .expect("clean server close timed out");
    }

    pub async fn drop_connections(&self) {
        let handles = self.handles.lock().unwrap().clone();
        assert!(
            !handles.is_empty(),
            "server cannot disconnect before a channel opens"
        );
        for handle in handles {
            timeout(
                WAIT,
                handle.disconnect(
                    russh::Disconnect::ByApplication,
                    "fixture disconnect".into(),
                    "".into(),
                ),
            )
            .await
            .expect("server disconnect timed out")
            .expect("server disconnect failed");
        }
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.shutdown.shutdown("test complete".into());
        // Normal shutdown informs accepted sessions too. An abort guarantees the
        // listener cannot remain alive after a panic or failed client assertion.
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
