import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: '.', testMatch: 'fixture.pw.ts', timeout: 90_000, expect: { timeout: 30_000 }, reporter: 'list',
  outputDir: '../../test-results/desktop/playwright',
  use: { trace: 'off', screenshot: 'off', video: 'off' },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 1000 } } }],
});
