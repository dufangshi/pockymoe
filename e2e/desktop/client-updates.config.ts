import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: '.', testMatch: 'client-updates.pw.ts', fullyParallel: false,
  timeout: 45_000, expect: { timeout: 8000 }, reporter: 'list',
  outputDir: join(process.env.DESKTOP_EVIDENCE_DIR ?? join(tmpdir(), 'pockymoe-client-updates'), 'playwright'),
  use: { baseURL: 'http://127.0.0.1:18929', trace: 'retain-on-failure', timezoneId: 'America/Toronto' },
  webServer: { command: 'pnpm --filter @pockymoe/supervisor-web exec vite --force --host 127.0.0.1 --port 18929 --strictPort', url: 'http://127.0.0.1:18929', reuseExistingServer: false },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 1000 } } }],
});
