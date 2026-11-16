import { defineConfig, devices } from '@playwright/test';
const port = Number(process.env.E2E_WEB_PORT ?? 19631);
export default defineConfig({
  testDir: '.', testMatch: 'built-status.spec.ts', timeout: 30_000,
  use: { baseURL: `http://127.0.0.1:${port}`, trace: 'retain-on-failure' },
  webServer: {
    command: `pnpm --filter @pockymoe/supervisor-web exec vite preview --host 127.0.0.1 --port ${port} --strictPort`,
    url: `http://127.0.0.1:${port}/e2e/fixtures/built-status.html`,
    reuseExistingServer: !process.env.CI, timeout: 30_000,
  },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'] } }],
});
