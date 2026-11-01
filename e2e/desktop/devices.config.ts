import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: '.', testMatch: ['devices.pw.ts', 'hourly.pw.ts', 'locale.pw.ts', 'compact.pw.ts'], fullyParallel: false,
  timeout: 45_000, expect: { timeout: 8000 }, reporter: 'list',
  outputDir: join(process.env.DESKTOP_EVIDENCE_DIR ?? join(tmpdir(), 'pockymoe-desktop-devices'), 'playwright'),
  use: { baseURL: 'http://127.0.0.1:18924', trace: 'retain-on-failure', timezoneId: 'America/Toronto' },
  webServer: {
    command: 'pnpm --filter @pockymoe/supervisor-web exec vite --force --host 127.0.0.1 --port 18924 --strictPort', url: 'http://127.0.0.1:18924', reuseExistingServer: true,
  },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 1100 } } }],
});
