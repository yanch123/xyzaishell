# Secure storage integration boundary

These modules are host-only. Dedicated settings/authentication IPC wraps them in
`host_storage`; they are never model tools. The host wires persistent trust and
task secret snapshots into the same SSH session and execution gate. Filesystem
and native keyring methods are blocking; settings IPC uses blocking workers.
Final task-reference validation runs inside the serialized dispatch boundary.

## Registration and dependencies

`pub mod storage` is registered in the core. Windows enables
`Win32_Security_Cryptography`; Linux dependencies are pinned/configured as follows:

- `secret-service = { version = "=5.2.0", default-features = false, features = ["rt-async-io-crypto-rust"] }`
- `zbus = { version = "5", default-features = false, features = ["async-io"] }`

Use the selected absolute platform configuration root, never the working directory
or a model-provided path. Directory/file adapters validate ownership, private
permissions, local filesystem, links and component traversal at use time.

## Intended host flow

1. `ProfileStore::open(root)` and `initialize(native_mode)` when absent, then
   `migrate()`. Schema 1 is validated under the stable profile lock, copied to a
   restricted `connections.v1.backup.toml`, validated, and replaced as schema 2.
   The backup must match if already present; mismatch stops migration. IDs,
   labels, targets, ciphertext and references are preserved. The revision advances
   once. No network connection, credential lookup, or command replay occurs.
2. Schema 2 stores `credential_policy = "ask_on_connect" | "system_secure"`
   per connection. The retained file-level storage field is legacy backend
   provenance, never a global policy switch. The host uses its current native
   DPAPI/Secret Service adapter regardless of a legacy `session_only` setting.
   Mixed policies coexist. Cross-platform protected descriptors remain intact;
   only an explicit replacement/clear changes the selected profile.
3. `save` accepts `CredentialChange::{Keep,Clear,Set}`. Metadata edits preserve
   stored credentials. Keep cannot silently reuse a secret after host, port,
   username, authentication method or private-key path changes. Set writes a new
   protected item before committing its reference; Clear removes only that
   profile's current reference. Orphan cleanup is deferred because protected
   backups and uncertain commits may still reference an item. Delete likewise
   removes only metadata/current references, never key files, trust or history.
   Failed/uncertain writes must be reread, never automatically retried.
   Authentication accepts an optional one-attempt value tied to exact profile ID
   and revision; it bypasses an unavailable provider without changing any stored
   reference. The host neither caches it nor returns it in status. New encrypted
   private-key attempts preflight the private file and prompt only when required.
   Old global-downgrade IPC is retired and cannot mutate any profile.
4. `KnownHostsStore::check(host,port,fingerprint)` returns Trusted or a single-use
   exact-target challenge, with prior fingerprint for changed keys. Only the
   dedicated user trust-confirmation path calls `accept_from_trust_ui`. Neither
   reconnect nor model output can grant trust. The `known_hosts` file is the
   application's private TOML database, not OpenSSH known_hosts syntax.
5. The host supplies its stable generated task UUID to
   `TaskCredentialStore::ensure_template`. Only requested field names are added;
   filled values are preserved. Existing files require the previously returned
   snapshot ID. The local edit path belongs only in settings UI; models receive
   `fields` metadata. User-edited task files are private plaintext, not a vault.
6. Explicit `load_snapshot` produces immutable values and new opaque IDs. It does
   not revoke existing snapshot objects. The host atomically changes the active
   snapshot at a safe execution boundary and invalidates old prepared actions
   and approvals. At final serialized dispatch call
   `validate_current_reference(owner_task_id, reference)` (or
   `validate_current_snapshot`) in addition to run/owner/session-generation,
   expiry, one-time approval, destination and supported-adapter checks. This
   guard compares the current loaded ID and bounded private-file bytes. Any
   mismatch requires reload and fresh preparation/approval. Then resolve only
   from the approved immutable snapshot. There is no shell substitution API.

External editors do not obey locks. Precommit byte comparison detects many editor
races but cannot establish full isolation; an edit after the final check remains
possible. A prior immutable secret snapshot cannot silently acquire new values.
The same-user compromise/administrator/device-driver threat remains outside the
private file boundary. Atomic visibility is not a guarantee against failed media
or sudden power loss.

## Tests and remaining runtime evidence

The storage tests cover strict/unknown schema fields, platform mismatch,
profile/purpose and task ownership binding, bounded input, no-secret listings,
single-attempt credential lifetime, versioned migration/backup failure and concurrent migration, mixed policies, identity-change rejection, task snapshots, final-dispatch guards, native backend absence,
permissions/links, host trust and separate-process revision conflicts. Windows
private-file adapter tests additionally cover ACLs, rename failures/uncertain
commit, pinned directory handles and stable locks.

Ordinary Linux tests run a native unavailable-provider check in a child process
with a deliberately nonexistent D-Bus socket. They never write a real keyring.
Windows ordinary tests run the synthetic DPAPI CurrentUser roundtrip and binding
failure. Cross-compilation is not Windows runtime evidence.

Linux native success/locked tests are ignored unless explicitly run inside a
**disposable** D-Bus/Secret Service session with private fresh HOME and XDG dirs,
`XYZ_TEST_ISOLATED_KEYRING=1`, and a synthetic collection password. Run in order:

1. `storage::tests::native_secret_service_isolated_roundtrip`
2. `storage::tests::native_secret_service_isolated_locked_is_fail_closed`

Use `cargo test -p xyz-core --locked --lib <name> -- --exact --ignored --nocapture`.
The second test intentionally leaves the disposable collection locked. Terminate
its daemon/session and remove only fixture directories afterward. Never use an
ordinary logged-in user's keyring for these tests. Providers that deny access or
cancel prompts still need dedicated isolated fault-provider runtime coverage.

SQLite reviewed-chat persistence is integrated separately in `history`/`persistence`.
The host selects APPDATA or XDG_CONFIG_HOME/HOME platform configuration roots;
history uses LOCALAPPDATA or XDG_STATE_HOME/HOME state roots. Credential migration preserves the protected legacy backup and supports
per-profile recovery from old session-only metadata. There is no global downgrade
or plaintext fallback. A user-confirmed single-line task-secret input
adapter is integrated via `task_secrets` and the existing one-time execution gate;
unknown contexts remain manual, and the user must verify the live target prompt.
Normal-exit history flush is best effort; abrupt termination can lose uncommitted
text and never restores a remote task or its approvals.

## SQLite private-file bridge

`PrivateDirectory::prepare_database_file("history.sqlite3", max_bytes)` returns
true only when it created a new private empty file. Existing contents are never
replaced/truncated, and an existing empty file returns false for the history
layer's explicit corruption handling. `validate_database_file` performs only
metadata checks on the fixed main/journal/wal/shm filenames; it permits a live
SQLite writer. Windows SQLite sidecars may inherit a private effective ACL from
the protected parent; the main file is precreated with an explicit protected
ACL. This exception does not relax the TOML read/write ACL contract.

Keep the directory guard alive while SQLite uses its selected path. The history
layer still owns SQLite NOFOLLOW/open flags, journal mode, safe serialization,
schema/transaction checks, sidecar validation before operations, retention and
consent. These metadata guards cannot stop a compromised same-user process from
racing a SQLite path after validation and do not make raw secret persistence safe.
