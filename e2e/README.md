# Browser-only UI smoke tests

These tests exercise the production frontend bundle in Chromium at 1400×900 and 960×640. They are not native Tauri, SSH, or real model-provider tests.

- `browser preview` verifies the split layout, disabled native capabilities, local drafts, settings, and keyboard focus.
- `synthetic IPC mock` injects an in-browser mock of the native IPC boundary. It verifies explicit recipient consent and safe rendering of synthetic streamed content. It never reads a model configuration, opens SSH, or calls a model endpoint.
- Successful tests save PNG screenshots under `test-results/` with `browser-preview-*` or `synthetic-ipc-*` names. CI uploads these and the HTML report for human inspection. A test pass is not a claim that a person reviewed the screenshots.

Run `npm ci`, `npx playwright install --with-deps chromium`, and `npm run test:browser`. Playwright builds and serves the frontend automatically. `npm run test:browser -- --list` validates discovery only; it does not launch a browser. The TypeScript aggregate also checks the tests and configuration.

Unit tests stay under `src/`; Vitest deliberately excludes these browser tests. No screenshots, traces, or browser reports are committed.
