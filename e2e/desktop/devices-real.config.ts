import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: '.', testMatch: 'devices-real.pw.ts', timeout: 90_000, expect: { timeout: 30_000 }, reporter: 'list',
  outputDir: join(process.env.DESKTOP_EVIDENCE_DIR ?? join(tmpdir(), 'pockymoe-desktop-devices'), 'playwright-real'), use: { trace: 'off', screenshot: 'off', video: 'off' },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 1100 } } }],
});
