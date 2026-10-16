# Browser-only UI smoke tests

These tests exercise the production frontend bundle in Chromium at 1400×900 and 960×640. They are not native Tauri, SSH, or real model-provider tests.

- `browser preview` verifies the split layout, disabled native capabilities, local drafts, nonmodal singleton settings, and keyboard dismissal.
- `synthetic IPC mock` injects an in-browser mock of the native IPC boundary. It verifies one configuration/target-scope consent, no-SSH greeting through the unified composer, no mode tabs, and safe rendering of synthetic streamed content. It never reads a model configuration, opens SSH, or calls a model endpoint.
- `synthetic settings` checks saved-version identity, separate test consent, no key echo, and cancel/discard of unsaved edits. Only the mocked IPC is invoked.
- `synthetic storage` verifies the unified profile metadata/credential editor, explicit history consent, read-only restoration, and a separately confirmed history removal. No SSH or model action is invoked.
- `synthetic terminal task` checks configuration/target-scope consent and backend readiness recovery without a manual readiness checkbox, command display before acknowledgement, single-action rejection, unknown completion, and the separate Ctrl+C button. All task/model/SSH results are explicitly synthetic, with no external execution.
- `synthetic unified conversation` covers connected greeting → displayed ordinary command → follow-up, one configuration/target-scope confirmation, exactly one copy of each transcript message, and command/target visibility before the synthetic dispatch. The mock supplies completion evidence; native command completion and model context are separately covered by Rust/native acceptance.
- Successful tests save PNG screenshots under `test-results/` with `browser-preview-*` , `synthetic-ipc-*`, `synthetic-task-*`, `workspace-*`, `settings-*`, or `storage-*` names. CI uploads these and the HTML report for human inspection. A test pass is not a claim that a person reviewed the screenshots.

Run `npm ci`, `npx playwright install --with-deps chromium`, and `npm run test:browser`. Playwright builds and serves the frontend automatically. `npm run test:browser -- --list` validates discovery only; it does not launch a browser. The TypeScript aggregate also checks the tests and configuration.

Unit tests stay under `src/`; Vitest deliberately excludes these browser tests. No screenshots, traces, or browser reports are committed.
