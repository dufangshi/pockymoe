import { defineConfig, devices } from '@playwright/test';

// Owns disposable services; never start the ordinary E2E Supervisor or record secrets.
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
export default defineConfig({
  testDir: '.',
  testMatch: 'port-preview-benchmark.spec.ts',
  timeout: 240_000,
  expect: { timeout: 15_000 },
  workers: 1,
  retries: 0,
  reporter: 'list',
  outputDir: '../.scratch/port-bench/playwright',
  use: { trace: 'off', screenshot: 'off', video: 'off', actionTimeout: 15_000 },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], locale: 'en-US' } }],
});
