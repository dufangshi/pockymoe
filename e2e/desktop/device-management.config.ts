import { defineConfig, devices } from '@playwright/test';
const port = Number(process.env.DEVICE_MANAGEMENT_PORT ?? 18934);
export default defineConfig({
  testDir: '.', testMatch: ['device-management.pw.ts', 'route-frames.pw.ts', 'device-management-actions.pw.ts'], fullyParallel: false,
  timeout: 180000, expect: { timeout: 10000 }, reporter: 'list',
  use: { baseURL: `http://127.0.0.1:${port}`, trace: 'retain-on-failure' },
  // All API/WebSocket traffic is mocked by the specs; no Supervisor is started.
  webServer: { command: `pnpm --filter @pockymoe/supervisor-web exec vite --force --host 127.0.0.1 --port ${port} --strictPort`, url: `http://127.0.0.1:${port}`, reuseExistingServer: false, cwd: '../..' },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 1000 } } }],
});
