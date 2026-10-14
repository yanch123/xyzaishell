# Connection profiles: implementation and acceptance

Implemented for the v0.1.2 development candidate. This is scoped implementation
and test evidence, not a claim of Windows/Linux installed-product acceptance.

## Behavior

- One connection editor covers temporary Connect, Save connection + Connect,
  Remember password/key passphrase, and existing-profile Save/Cancel. An explicit
  secondary “仅保存连接” supports offline metadata creation without authentication.
- Existing profile edits and the saved list are nonmodal. Passwords are never
  read back. Rename preserves the profile ID and credential reference. An
  authentication-target change requires new credentials or explicit clearing.
- Missing passwords are requested by the connect workflow. Private keys are
  validated before saved-profile network activity; unencrypted keys need no
  fabricated passphrase and encrypted keys produce a dedicated challenge.
- New credentials default to one authentication attempt. The native host binds
  input to the exact saved profile ID/revision, validates again after slow local
  credential/key operations, and does not keep an in-process reusable secret.
- Remembering first obtains a confirmed save result, then separately connects.
  Save failures never implicitly connect or downgrade protection. Forms clear
  submitted secrets, reject duplicate submissions, and require rereading after
  conflicting or uncertain saves. Cancellation/discard is explicit.
- Policy is per profile: `ask_on_connect` or `system_secure`. DPAPI CurrentUser
  and Secret Service remain the only persistent providers. Locked, unavailable,
  access-denied and cross-platform states retain original references; metadata
  can still be edited and temporary authentication can bypass the provider.
- Deletion/clear is scoped to the selected profile and expected revision. Old
  keyring entries and protected migration backups are deliberately retained;
  there is no unsafe cross-storage garbage collection. UI discloses this. Key
  files, trust, chats and operation logs are not deleted and sessions stay live.

## Schema and migration

`connections.toml` now writes schema 2 with a required per-profile policy. The
remaining `storage.secret_protection` field records legacy backend provenance,
not a global policy. The host always offers its native provider; old
`session_only` data can recover one profile at a time.

Startup validates schema 1, acquires the stable file lock, writes and rereads a
restricted exact-byte `connections.v1.backup.toml`, preserves IDs/labels/targets
and protected references, advances the revision once, safely replaces the file,
and verifies the result. A different existing backup, unknown schema, invalid
file, permissions failure or conflicting write stops migration. The only legacy
copy is never overwritten first. Migration itself does not connect, access
credential values, send model requests or replay commands. Foreign-platform
protected references are retained and cannot be decrypted by the wrong provider.

The old all-profile downgrade operation now returns a fixed retired-operation
error and cannot mutate data. Low-level legacy session APIs remain for existing
unit coverage, but the current frontend exposes neither a global mode switch nor
a persistent process-wide credential cache.

## Evidence

Local checks on the development tree (2026-10-14):

- `npm run typecheck`: passed.
- ConnectionDialog, ProfileDialog and storageBridge focused tests: 21 passed.
  Covers temporary default, save-before-connect ordering, explicit persistence,
  unavailable provider, existing edit/keep, changed target, key passphrase
  challenge, save-only metadata, cancellation, duplicate submit, new-process
  routing, delete confirmation and IPC ID/revision binding.
- `cargo test -p xyz-core --lib storage:: --offline`: 32 passed, 3 explicitly
  platform-isolated tests ignored. Includes key preflight, no secret persistence,
  revision conflicts, protected migration/backup failure, concurrent-process
  migration, independent policies, foreign references and scoped deletion.
- Existing native DPAPI and isolated Secret Service roundtrip tests now also
  verify upgrading legacy session-only profiles and recovering a single profile
  to native protection while another remains ask-on-connect.

Remaining acceptance is real Windows DPAPI/ACL runtime, disposable Linux Secret
Service success/locked/access-denied, installed v0.1.1 → v0.1.2 upgrades, abrupt
failure/uncertain commits, and installed multi-window interaction. Passing mocks,
a browser preview, compilation or same-version reinstall does not replace it.
All tests use synthetic secrets, isolated files and explicit disposable providers;
no real user credentials are used.
