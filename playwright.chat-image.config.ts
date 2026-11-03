import { defineConfig, devices } from '@playwright/test';
const port = Number(process.env.E2E_WEB_PORT ?? 16373);
// UI-only fixture: no Supervisor, database, relay or real harness is started.
export default defineConfig({
  testDir: './e2e', testMatch: 'chat-image.spec.ts', timeout: 30000,
  expect: { timeout: 10000 }, workers: 1, retries: 0, reporter: 'list',
  use: { baseURL: `http://localhost:${port}`, trace: 'retain-on-failure' },
  webServer: { command: `pnpm --filter @pockymoe/supervisor-web exec vite --force --host localhost --port ${port} --strictPort`, url: `http://localhost:${port}`, reuseExistingServer: false },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'] } }],
});
