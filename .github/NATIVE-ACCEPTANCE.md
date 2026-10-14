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
- Same-version installer reinstallation, then uninstall/reinstall. Configuration
  hashes remain unchanged during installer operations; the binary disappears on
  uninstall; the restored native UI still reads the synthetic saved state.

Screenshots, native-driver logs, installer transcript, runtime/driver versions,
installed resource hashes and JSON results are retained as Actions artifacts.
Only synthetic data appears in those artifacts. App configuration uses a fresh
private fixture HOME/XDG/AppData, which is removed after the test. No ordinary
user profile, keyring, credential or SSH server is involved.

## Not established by this suite

Same-version reinstallation is **not a version-upgrade test**. There is no older
released version yet. Windows Server 2022 is **not Windows 10 or Windows 11**.
Xvfb is **not an end-user X11/Wayland desktop session**. These tests do not cover
IME/clipboard, real-provider compatibility, SSH through the installed UI,
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
