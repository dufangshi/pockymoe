import { defineConfig, devices } from '@playwright/test';

// Owns its Relay, Supervisor, static server and optional packaged Electron profile.
// Traces/video stay off because local investigations may use private input PNGs.
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
export default defineConfig({
  testDir: '.',
  testMatch: 'chat-image-pipeline.spec.ts',
  timeout: 180_000,
  expect: { timeout: 20_000 },
  workers: 1,
  retries: 0,
  reporter: 'list',
  outputDir: '../.scratch/chat-image-2/playwright',
  use: { actionTimeout: 15_000, navigationTimeout: 20_000, trace: 'off', screenshot: 'off', video: 'off' },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'] } }],
});
