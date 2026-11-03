import path from 'node:path';
import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: '.', testMatch: 'mobile-map.spec.ts', timeout: 30_000, expect: { timeout: 8000 }, reporter: 'list',
  outputDir: '../test-results/mobile-map',
  use: { baseURL: 'http://127.0.0.1:18927', trace: 'retain-on-failure' },
  webServer: [
    { cwd: path.resolve(__dirname, '..'), command: 'pnpm --filter @pockymoe/supervisor-web exec vite --host 127.0.0.1 --port 18925 --strictPort', url: 'http://127.0.0.1:18925', reuseExistingServer: true },
    { cwd: path.resolve(__dirname, '..'), command: 'MAP_THEME_ORIGIN=http://127.0.0.1:18925 pnpm --dir pockymoe-thread-ui --filter @pockymoe/thread-ui-playground exec vite --host 127.0.0.1 --port 18927 --strictPort', url: 'http://127.0.0.1:18927', reuseExistingServer: true },
  ],
  projects: [{ name: 'mobile-chromium', use: { ...devices['Pixel 5'], viewport: { width: 390, height: 844 } } }],
});
