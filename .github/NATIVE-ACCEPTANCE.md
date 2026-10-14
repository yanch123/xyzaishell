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
- Save and reopen a synthetic nonsecret SSH profile. Do not connect to a host or
  retrieve any real credential.
- Explicitly enable plaintext history; recipient consent blocks a model request
  until checked. The model is an in-process loopback HTTP fixture that accepts
  exactly the synthetic request and returns SSE text including Chinese and an
  inert script literal. Nothing is forwarded to a third party.
- Close the actual app process and restart. History requires fresh opt-in; read
  the saved profile and restore only text, retain task/chat identity, and observe
  zero new model requests and no SSH connection.
- On v0.1.1, open and close the real Shell-log dialog while disconnected; consent/start remain disabled, and JSON/CSV export returns the real empty-store error without creating a DB, lock or export file. No synthetic shell events are injected.
- Same-version installer reinstallation, then uninstall/reinstall. Configuration
  hashes remain unchanged during installer operations; the binary disappears on
  uninstall; the restored native UI still reads the synthetic saved state.

A separate fresh private fixture then installs the public v0.1.0 release pinned
by installer and manifest SHA256, exact source/run provenance and installed binary
SHA256. It creates the synthetic profile/history through the old real UI, restarts,
then directly installs v0.1.1 over it. Before launching the new application it
checks the installed version and unchanged data-file hashes. The new real UI must
restore the same text/task identity with fresh history consent and zero model
requests, then actually uninstall without deleting the data. This is the reviewed
0.1.0→0.1.1 upgrade path, not arbitrary upgrades or downgrade support.

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
0.1.0→0.1.1 fixture is required to establish that specific path. Windows Server
2022 is **not Windows 10 or Windows 11**.
Xvfb is **not an end-user X11/Wayland desktop session**. These tests do not cover
IME/clipboard, real-provider compatibility, SSH through the installed UI or
nonempty Shell-log export through installed UI,
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
