import { defineConfig, devices } from '@playwright/test';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  testDir: '.', testMatch: '*.pw.ts', fullyParallel: false,
  timeout: 30_000, expect: { timeout: 8000 }, reporter: 'list',
  outputDir: '/tmp/rc-desktop/evidence/devices/playwright-results',
  use: { baseURL: 'http://127.0.0.1:18904', trace: 'retain-on-failure' },
  webServer: {
    command: 'pnpm --filter @remote-codex/supervisor-web exec vite --host 127.0.0.1 --port 18904 --strictPort',
    cwd: fileURLToPath(new URL('../../../../../', import.meta.url)), url: 'http://127.0.0.1:18904', reuseExistingServer: true,
  },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 1120 } } }],
});
