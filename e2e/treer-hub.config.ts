import { defineConfig, devices } from '@playwright/test';

// This spec owns an isolated hosted Supervisor and simulated Treer tunnel.
// Build target/debug/pockymoe and the Web dist before running it.
export default defineConfig({
  testDir: '.',
  testMatch: 'treer-hub.spec.ts',
  timeout: 60_000,
  expect: { timeout: 15_000 },
  workers: 1,
  reporter: 'list',
  use: { trace: 'retain-on-failure' },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'] } }],
});
