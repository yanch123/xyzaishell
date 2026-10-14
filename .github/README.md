# Build and release workflows

## Branch and artifact policy

- Work is staged on `dev`. Pushes to `dev`/`main`, pull requests targeting either branch, and manual CI runs execute `ci.yml`.
- Both platforms use the same source commit, root `Cargo.lock`, `package-lock.json`, and exact Rust version from `rust-toolchain.toml`. Node is pinned to 22.23.3 in the workflow/setup action.
- Cargo registry index/cache/source, public Git dependency databases, and `target/` are cached with OS, architecture, exact-toolchain-file hash, lockfile hash, and source SHA keys. Restore fallback stays within the same OS/toolchain/lockfile. Cargo config and credential files are never cached; all locked tests and builds still run.
- CI runs frontend tests and a production build, Rust formatting/tests/Clippy, then Tauri packaging. The root Cargo workspace means packages are in `target/release/bundle`, not `src-tauri/target`.
- Windows x64 NSIS is built on `windows-2022`. Ubuntu x64 DEB is built on `ubuntu-22.04`, the oldest promised Linux baseline. No AppImage, macOS, or ARM artifact is advertised.
- Candidate installers, SHA-256 checksums, and build provenance are retained as Actions artifacts for 14 days. A successful development build does not publish a GitHub Release.

## What the checks establish

A separate Playwright/Chromium job runs the browser preview with synthetic/mock data and uploads success screenshots, traces when applicable, and the HTML report as `browser-preview-smoke`. This verifies preview rendering and the mocked interaction flows in the tests; it is not Tauri IPC, WebView2/WebKitGTK, SSH, model-service, or native desktop acceptance. Inspect the screenshots as well as the test result before making visual-quality claims.

The Linux build job explicitly runs the normally ignored real-OpenSSH integration test using `.github/scripts/openssh-test.sh --disposable-user`. It creates fresh temporary host/client keys and a temporary unprivileged account only on a disposable `CI=true` runner, binds the isolated daemon to loopback, checks real PTY/shell/resize behavior and the absence of hidden exec/subsystem requests, and cleans up. It does not use a production SSH server or credentials. The CI result must be checked before claiming this interoperability test passed; ordinary `cargo test` reports it as ignored.

The Linux build job also runs `.github/scripts/secret-service-test.sh`: it builds the locked core test executable, then runs the two explicitly ignored native Secret Service tests under a new D-Bus session, synthetic GNOME login keyring, and private temporary HOME/XDG directories. The session receives a cleared environment, never an existing desktop bus or keyring. It verifies store/load/delete first and locked-provider fail-closed behavior last, bounds execution time, stops the daemon/session, and removes the fixture. Windows runs the synthetic CurrentUser DPAPI roundtrip and binding-failure test in the ordinary Rust suite. Neither test uses a user's real credential. These are provider-runtime checks, not native application/UI acceptance.

`linux-install-smoke` downloads the actual Ubuntu 22.04-built DEB into fresh Ubuntu 22.04 and 24.04 jobs, checks its checksum, installs its declared dependencies, checks binary linkage and desktop entry, and launches it with an isolated configuration in Xvfb. It requires a visible top-level window and a live process. Evidence is retained separately for each OS.

This is **headless startup smoke**, not a real-desktop acceptance result. It does not establish rendering correctness, Chinese input/clipboard, X11/Wayland behavior, SSH/model integration, keyring behavior, installation on a clean end-user desktop, upgrade, or uninstall. A Windows Server build is not Windows 10/11 runtime testing. Complete and record the acceptance matrix in `docs/v0.1/06-testing.md` before promising v0.1 support.

## Preparing a release

1. Finish the staged implementation and verification on `dev`, then merge the verified version to `main` under the repository's normal review policy. These workflows do not change branch protection or merge automatically.
2. Confirm the workspace Cargo version, Tauri version, `package.json`, and `package-lock.json` all match, for example `0.1.0`. Commit both lockfiles.
3. Create the corresponding stable version tag, for example `v0.1.0`, on the intended `main` commit. A pushed version tag runs `release.yml`. Alternatively, run **Prepare draft release** manually with an existing tag. The workflow refuses a tag whose commit is not an ancestor of `origin/main` or whose version does not match.
4. The release workflow rebuilds and reruns the entire CI workflow for that exact tag commit, checks both platforms' source and lockfile hashes, then creates a **draft** release. It includes source ZIP and TAR.GZ archives generated from that commit, the NSIS and DEB installers, per-platform provenance JSON, and `SHA256SUMS.txt`.
5. Review actual runtime/installation/security/model/SSH results and dependency licenses, replace the draft's pending-acceptance text with precise evidence and limitations, and only then explicitly publish the draft. This workflow never publishes automatically. It refuses to overwrite an existing draft or published release; investigate a partial or failed previous run first.

No signing identity has been configured. Windows installers may show unknown-publisher or SmartScreen warnings. WebView2 and WebKitGTK behavior must be checked in the platform acceptance runs. No updater is published, and no code-signing key, PAT, repository setting, or persistent credential is created. Only the draft-release job receives `contents: write` through the ephemeral `GITHUB_TOKEN`; CI jobs get `contents: read`. Checkout does not persist that token. Third-party actions are official GitHub actions pinned to full commit hashes.

## Local workflow checks

From the repository root:

```sh
python3 -m unittest discover -s .github/scripts -p 'test_*.py' -v
python3 .github/scripts/artifacts.py check-version --tag v0.1.0
bash -n .github/scripts/linux-install-smoke.sh
bash -n .github/scripts/openssh-test.sh
bash -n .github/scripts/secret-service-test.sh
actionlint .github/workflows/*.yml
```

These static/helper checks do not substitute for a successful hosted workflow run. Update action pins and pinned toolchain versions deliberately, rerunning CI afterward.

## Upstream references

- [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/)
- [Tauri GitHub Actions guidance](https://v2.tauri.app/distribute/pipelines/github/)
- [Tauri Windows installer](https://v2.tauri.app/distribute/windows-installer/)
- [Tauri Debian packaging](https://v2.tauri.app/distribute/debian/)
- [Tauri CLI flags](https://v2.tauri.app/reference/cli/)
- [Node 22.23.3 release](https://nodejs.org/en/blog/release/v22.23.3)
