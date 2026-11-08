import { defineConfig, devices } from '@playwright/test';
import { fileURLToPath } from 'node:url';
const repo = fileURLToPath(new URL('../../../', import.meta.url));
const port = Number(process.env.DOKPLOY_UI_WEB_PORT ?? 16463);
// API/WS are intercepted in the spec. No Supervisor or database is started.
export default defineConfig({
  testDir: '.', testMatch: 'dokploy.spec.ts', workers: 1, retries: 0, timeout: 45000,
  expect: { timeout: 10000 }, reporter: 'list', outputDir: '../../../.scratch/dokploy-ui/playwright',
  use: { baseURL: `http://localhost:${port}`, actionTimeout: 10000, trace: 'retain-on-failure' },
  webServer: {
    cwd: repo,
    command: `corepack pnpm --filter @pockymoe/supervisor-web exec vite --host localhost --port ${port} --strictPort`,
    env: { VITE_API_PROXY_TARGET: 'http://127.0.0.1:1', VITE_WS_PROXY_TARGET: 'ws://127.0.0.1:1' },
    url: `http://localhost:${port}`, reuseExistingServer: false, timeout: 60000,
  },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'] } }, { name: 'mobile-chromium', use: { ...devices['Pixel 5'] } }],
});
