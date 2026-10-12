import { defineConfig, devices } from '@playwright/test';

// Isolated smoke fixture owns its relay and Vite processes. Ports are assigned by
// the coordinator and supplied explicitly; the regular Supervisor is not started.
export default defineConfig({
  testDir: './e2e',
  testMatch: 'relay-static-origin.spec.ts',
  timeout: 90_000,
  expect: { timeout: 15_000 },
  workers: 1,
  reporter: 'list',
  use: { trace: 'off', screenshot: 'off', video: 'off' },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], locale: 'en-US' } }],
});
