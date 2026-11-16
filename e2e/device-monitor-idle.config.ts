import { defineConfig, devices } from '@playwright/test';

const port = Number(process.env.E2E_WEB_PORT ?? 18935);
export default defineConfig({
  testDir: '.', testMatch: 'device-monitor-idle.spec.ts', workers: 1,
  timeout: 45_000, expect: { timeout: 8000 },
  use: { baseURL: `http://127.0.0.1:${port}`, trace: 'retain-on-failure' },
  webServer: {
    command: `pnpm --filter @pockymoe/supervisor-web exec vite --host 127.0.0.1 --port ${port} --strictPort`,
    url: `http://127.0.0.1:${port}`, reuseExistingServer: false,
  },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'] } }],
});
