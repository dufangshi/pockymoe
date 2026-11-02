import { defineConfig, devices } from '@playwright/test';
const port = Number(process.env.DESKTOP_E2E_WEB_PORT ?? 18926);
export default defineConfig({
  testDir: '.', testMatch: ['chrome.pw.ts', 'forms.pw.ts'], fullyParallel: false, timeout: 30000, expect: { timeout: 5000 }, reporter: 'list',
  outputDir: process.env.DESKTOP_EVIDENCE_DIR ? `${process.env.DESKTOP_EVIDENCE_DIR}/playwright` : '../../test-results/desktop/webnav',
  use: { baseURL: `http://127.0.0.1:${port}`, trace: 'retain-on-failure' },
  webServer: { command: `pnpm --filter @pockymoe/supervisor-web exec vite --config src/desktop/testing/vite.config.ts --force --host 127.0.0.1 --port ${port} --strictPort`, url: `http://127.0.0.1:${port}`, reuseExistingServer: true },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } }],
});
