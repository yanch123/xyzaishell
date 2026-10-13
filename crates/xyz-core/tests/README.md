# SSH loopback integration tests

Run with `cargo test -p xyz-core --test ssh_loopback` or include them in
`cargo test -p xyz-core`. No running SSH daemon, user account, SSH agent, user
key, network service, GTK, or desktop session is required. TCP loopback access
must be available. A missing prerequisite is a test failure, never a silent skip.

`support/mod.rs` starts a real russh server bound to `127.0.0.1:0`, generates a
fresh Ed25519 host key in memory, and records server-side protocol evidence.
The server accepts only synthetic credentials and refuses SSH `exec` requests.
Each test uses a two-worker Tokio runtime so lifecycle work can truly overlap.
Assertions wait for protocol events with diagnostic timeouts; they do not use
sleep-based success assumptions. Test failures do not trigger automatic retries.

The checks cover:

- Host-key approval before authentication; rejection; challenge and fingerprint
  binding; single-use decisions; changed keys on the same endpoint; cancellation;
  and manager-lifetime-only trust
- Correct and incorrect password authentication, plus unencrypted and encrypted
  private-key authentication on Unix; no credentials in serialized events
- PTY and shell acknowledgements and refusals; one session channel; exact binary
  input/output; terminal size changes; no hidden exec channel
- One live connection, new identity and generation on reconnect, stale request
  rejection, and harmless delayed output acknowledgements
- Input validation, concurrent frame ordering, explicit queue backpressure, no
  replay, the 256-KiB unacknowledged-output bound, and disconnect during output
  backpressure
- Client and server disconnects, socket closure, final output and remote exit
  status arriving immediately before a clean channel close

The private-key tests create only generated synthetic key material, in a `0700`
temporary directory and `0600` file on Unix, because the public connection API
accepts a selected path. They remove it on success or unwind and verify cleanup
on success. No private key fixture is stored in the repository. Those success
tests are Unix-only: M1 intentionally rejects Windows private-key loading until
its ACL checks are implemented. Password transport tests remain cross-platform.

These tests exercise real SSH negotiation and PTY protocol requests against a
controlled server. The fixture echoes bytes and acknowledges PTY requests; it
does not allocate an operating-system PTY or run a shell. Passing here is not
OpenSSH interoperability, real shell behavior, a Tauri/GUI test, platform
credential-store validation, Windows validation, or installation/package
acceptance. Those remain separate acceptance steps.
