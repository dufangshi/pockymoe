import { defineConfig, devices } from '@playwright/test';
const port = Number(process.env.DESKTOP_E2E_WEB_PORT ?? 18931);
export default defineConfig({
  testDir: '.', testMatch: ['shortcuts.pw.ts'], fullyParallel: false, workers: 1, timeout: 60000, expect: { timeout: 8000 }, reporter: 'list',
  outputDir: process.env.DESKTOP_EVIDENCE_DIR ? `${process.env.DESKTOP_EVIDENCE_DIR}/playwright` : '../../test-results/desktop/shortcuts',
  use: { baseURL: `http://127.0.0.1:${port}`, trace: 'retain-on-failure' },
  // No API behind the page: every request the test needs is routed in the spec.
  webServer: { command: `DESKTOP_E2E_CACHE_DIR=node_modules/.vite-desktop-shortcuts VITE_API_PROXY_TARGET=http://127.0.0.1:0 VITE_WS_PROXY_TARGET=ws://127.0.0.1:0 pnpm --filter @pockymoe/supervisor-web exec vite --config src/desktop/testing/vite.config.ts --host 127.0.0.1 --port ${port} --strictPort`, url: `http://127.0.0.1:${port}`, reuseExistingServer: true, cwd: '../..', timeout: 120000 },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } }],
});
