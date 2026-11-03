import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig, devices } from '@playwright/test';

const webPort = Number(process.env.E2E_WEB_PORT ?? 18924);
const webOrigin = `http://127.0.0.1:${webPort}`;

export default defineConfig({
  testDir: '.', testMatch: ['model-pricing.pw.ts', 'devices.pw.ts', 'hourly.pw.ts', 'usage-stats.pw.ts', 'locale.pw.ts', 'compact.pw.ts', 'webDevices.pw.ts'], fullyParallel: false,
  timeout: 45_000, expect: { timeout: 8000 }, reporter: 'list',
  outputDir: join(process.env.DESKTOP_EVIDENCE_DIR ?? join(tmpdir(), 'pockymoe-desktop-devices'), 'playwright'),
  use: { baseURL: webOrigin, trace: 'retain-on-failure', timezoneId: 'America/Toronto' },
  webServer: {
    command: `pnpm --filter @pockymoe/supervisor-web exec vite --force --host 127.0.0.1 --port ${webPort} --strictPort`, url: webOrigin, reuseExistingServer: true,
  },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 1100 } } }],
});
