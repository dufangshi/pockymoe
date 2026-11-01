import { defineConfig, devices } from '@playwright/test';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  testDir: '.', testMatch: '*.pw.ts', fullyParallel: false,
  timeout: 45_000, expect: { timeout: 8000 }, reporter: 'list',
  outputDir: '/tmp/rc-desktop/evidence/devices/v2/playwright-results',
  use: { baseURL: 'http://127.0.0.1:18924', trace: 'retain-on-failure', timezoneId: 'America/Toronto' },
  webServer: {
    command: 'pnpm --filter @pockymoe/supervisor-web exec vite --config src/desktop/devices/vite.config.ts --force --host 127.0.0.1 --port 18924 --strictPort',
    cwd: fileURLToPath(new URL('../../../../../', import.meta.url)), url: 'http://127.0.0.1:18924', reuseExistingServer: true,
  },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 1100 } } }],
});
