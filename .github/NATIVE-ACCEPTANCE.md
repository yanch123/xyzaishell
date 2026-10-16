# Installed native application smoke

`native-acceptance.yml` is a reusable workflow intended to run **after** the
caller's installer builds. It consumes those same-run artifacts for the exact
source SHA. It does not build a special test version or install an application
debug plugin. `native-ui.py` uses the standard W3C WebDriver protocol via pinned
official `tauri-driver` 2.1.0, platform WebKitWebDriver, or an Authenticode-verified
Microsoft driver matching the installed WebView2 runtime.
The Windows fixture selects the matching installed runtime with supported
process-local WebView2 environment overrides and uses a private disposable
WebView2 user-data folder. It does not alter machine browser/registry settings.

## Covered, only after a successful hosted run

- Actual Ubuntu-built DEB installation, runtime linkage and desktop entry on
  Ubuntu 22.04 and 24.04; actual NSIS installation on Windows Server 2022.
- Installed third-party notices, runtime inventory, Rust library notices and
  exact MPL-covered `option-ext` source archive are present and nonempty.
- Native WebKitGTK/WebView2 renders the real packaged frontend. Real DOM
  clicks/keystrokes call the ordinary Tauri IPC and Rust backend.
- Configure and save a synthetic model key through both reviewed versions' settings UI;
  verify a separate explicit minimal-test consent and successful real loopback
  streaming test, with no chat history sent or key echoed. A private fixture
  file enables loopback HTTP and non-default advanced options before startup;
  their preservation is checked. This is not missing-file setup coverage.
- Save and reopen a synthetic nonsecret SSH profile through the shared editor
  using the explicit save-only action; retain the same profile ID after restart. Do not connect to a host or
  retrieve any real credential.
- Explicitly enable plaintext history; recipient consent blocks a model request
  until checked. The model is an in-process loopback HTTP fixture that accepts
  exactly the synthetic request and returns SSE text including Chinese and an
  inert script literal. Nothing is forwarded to a third party.
- Close the actual app process and restart. History requires fresh opt-in; read
  the saved profile and restore only text, retain task/chat identity, and observe
  zero new model requests and no SSH connection.
- On the candidate, open and close the real Shell-log view while disconnected; consent/start remain disabled, explicitly select all offline records, and JSON/CSV export returns the real empty-store error without creating a DB, lock or export file. No synthetic shell events are injected.
- Same-version installer reinstallation, then uninstall/reinstall. Configuration
  hashes remain unchanged during installer operations; the binary disappears on
  uninstall; the restored native UI still reads the synthetic saved state.

A separate fresh private fixture installs the public v0.1.4 prerelease pinned
by installer and manifest SHA256, exact source/run provenance and independently
extracted installed-binary SHA256. Pins fail closed if an upstream asset changes.
The reviewed source is `0e29b1e5912f35e9eabd73ac073524af143cd622`, public
[release run 38937958608](https://github.com/yanch123/xyzaishell/actions/runs/38937958608),
[release 415214180](https://github.com/yanch123/xyzaishell/releases/tag/v0.1.4).
The [reviewed fixture](scripts/fixtures/upgrade-v0.1.4.json) records exact manifest
bytes and both application identities extracted from the public packages.
No earlier candidate or v0.1.3 Windows binary hash is reused.

The baseline is seeded through its real UI, restarted and directly upgraded to
v0.2.0. Before candidate launch, installer operations must preserve data-file
hashes. On launch, history migrates from schema v2 to v3: original scalar chat
identity fields stay exact; `legacy_backups` retains original payload bytes; all
original message fields and ordered identities are copied to `entries`; the chat
metadata no longer contains a bounded transcript copy. Profile/model files retain
exact hashes and private ownership/permissions. The candidate restores text with
fresh opt-in and no model requests. This establishes only the reviewed upgrade
path after a successful hosted run, never arbitrary upgrade/downgrade support.

Candidate-only native checks send six completed synthetic conversation turns
through the real composer. The engine protects the two newest complete turns;
long older replies therefore make positive reduction possible. At turn three,
local quick compaction must succeed (not merely report no effect), without any
model call. After additional turns, an LLM compaction calls the loopback provider
with real serialized source manifests. The fixture holds that response until
the UI cancels and the backend reports cancellation; only then is it released.
A retry must issue another real request and commit a validated, manifest-bound
strict JSON summary with positive reduction. No IPC, ledger, or command state
is injected. The popover also verifies default 256000 capacity/source/80%-95%
explanation. After both compactions, the archive must still contain every
original message. Archive reads and page refresh never call a model or SSH.

Browser synthetic IPC tests additionally cover draft isolation and 130-message
cursor pagination. These do not establish native multi-page archive behavior.
Real-provider behavior and native multi-page history remain explicit gaps.
All native coverage above requires a successful hosted run; helper unit tests
and source changes alone do not establish application acceptance.

Fresh-install and upgrade scenarios do not share HOME/XDG/AppData or WebView
profiles. Only the native driver download is reused; Windows signature/version
checks still run on every use. Upgrade evidence is inside `version-upgrade/`.

Screenshots, native-driver logs, installer transcript, runtime/driver versions,
installed resource hashes and JSON results are retained as Actions artifacts.
Only synthetic data appears in those artifacts. App configuration uses a fresh
private fixture HOME/XDG/AppData, which is removed after the test. No ordinary
user profile, keyring, credential or SSH server is involved.

## Not established by this suite

Same-version reinstallation is **not a version-upgrade test**; the separate pinned
0.1.2→0.1.3 fixture is required to establish that specific path. Windows Server
2022 is **not Windows 10 or Windows 11**.
Xvfb is **not an end-user X11/Wayland desktop session**. These tests do not cover
IME/clipboard, real-provider compatibility, SSH through the installed UI or
nonempty Shell-log export through installed UI,
or v1-to-v2 migration in this current matrix, or an installed-version upgrade of a previously encrypted credential (the native
upgrade fixture intentionally stores only nonsecret connection metadata),
SmartScreen/unsigned-publisher UX, or long-running interactive behavior. Keep
the remaining acceptance matrix and release limits explicit.

The Linux Secret Service and Windows DPAPI provider tests are separate Rust
runtime checks. A synthetic HTTP model fixture is different from a mocked Tauri
IPC bridge: the native suite exercises real IPC and Rust networking/persistence.

## Local helper validation

`python3 -m unittest discover -s .github/scripts -p 'test_*.py' -v` checks helper
integrity and the loopback fixture, not the native application. Shell syntax and
actionlint checks likewise do not substitute for running the hosted native jobs.
The Linux/Windows lifecycle scripts deliberately require a disposable CI runner.

## Primary references

- [Tauri native WebDriver setup](https://v2.tauri.app/develop/tests/webdriver/manual-setup/)
- [Tauri WebDriver on GitHub Actions](https://v2.tauri.app/develop/tests/webdriver/ci/)
- [Official tauri-driver release history](https://tauri.app/release/tauri-driver/all-versions/)
- [Microsoft WebView2 WebDriver](https://learn.microsoft.com/en-us/microsoft-edge/webview2/how-to/webdriver)
- [Tauri installer configuration](https://v2.tauri.app/distribute/windows-installer/)
