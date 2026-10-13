import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL: 'http://127.0.0.1:1420',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    { name: 'chromium-wide', use: { browserName: 'chromium', viewport: { width: 1400, height: 900 } } },
    { name: 'chromium-compact', use: { browserName: 'chromium', viewport: { width: 960, height: 640 } } },
  ],
  webServer: {
    command: 'npm run build && npm run preview -- --port 1420 --strictPort',
    url: 'http://127.0.0.1:1420',
    reuseExistingServer: !process.env.CI,
    timeout: 120000,
  },
});
