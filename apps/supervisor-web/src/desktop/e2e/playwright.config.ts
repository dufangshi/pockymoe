import { fileURLToPath } from 'node:url';
import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: '.', testMatch: process.env.DESKTOP_REAL_BACKEND === '1' ? 'fixture.pw.ts' : 'desktop.pw.ts', fullyParallel: false,
  timeout: 30_000, expect: { timeout: 8_000 },
  reporter: 'list', outputDir: '/tmp/rc-desktop/evidence/webnav/playwright-results',
  use: { baseURL: 'http://127.0.0.1:18900', trace: 'retain-on-failure', viewport: { width: 1280, height: 850 } },
  webServer: process.env.DESKTOP_REAL_BACKEND === '1' ? [] : { command: 'pnpm --filter @remote-codex/supervisor-web exec vite --host 127.0.0.1 --port 18900 --strictPort', cwd: fileURLToPath(new URL('../../../../../', import.meta.url)), url: 'http://127.0.0.1:18900', reuseExistingServer: true },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1280, height: 850 } } }],
});
