import { defineConfig, devices } from '@playwright/test';
// The spec starts its own API-only Relay, isolated Supervisor and static Web.
export default defineConfig({
  testDir: './e2e', testMatch: 'chat-image-pipeline.spec.ts',
  timeout: 240_000, expect: { timeout: 15_000 }, workers: 1, retries: 0,
  reporter: 'list', use: { trace: 'off', screenshot: 'off', video: 'off', launchOptions: { ignoreDefaultArgs: ['--disable-background-timer-throttling', '--disable-backgrounding-occluded-windows', '--disable-renderer-backgrounding'] } },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], locale: 'en-US' } }],
});
