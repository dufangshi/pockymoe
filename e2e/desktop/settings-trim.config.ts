import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: '.', testMatch: 'settings-trim.pw.ts', fullyParallel: false,
  timeout: 45_000, expect: { timeout: 8000 }, reporter: 'list',
  outputDir: '../../.scratch/settings-cut/playwright',
  use: { actionTimeout: 8000, baseURL: 'http://127.0.0.1:18937', trace: 'retain-on-failure' },
  webServer: { command: 'pnpm --filter @pockymoe/supervisor-web exec vite --force --host 127.0.0.1 --port 18937 --strictPort', url: 'http://127.0.0.1:18937', reuseExistingServer: false },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 1000 } } }],
});
