import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { defineConfig, devices } from '@playwright/test';
const sharedRevision = readFileSync(resolve('thread-ui.sha'), 'utf8').trim().slice(0, 12);
const sharedBuild = createHash('sha256').update(readFileSync(resolve('apps/supervisor-web/node_modules/@pockymoe/thread-ui/dist/index.js'))).digest('hex').slice(0, 12);
const port = Number(process.env.DEVICE_MANAGEMENT_PORT ?? 18934);
export default defineConfig({
  testDir: '.', testMatch: ['device-management.pw.ts', 'route-frames.pw.ts', 'retained-scope.pw.ts', 'device-management-actions.pw.ts'], fullyParallel: false,
  outputDir: '../../.scratch/device-management/playwright',
  timeout: 180000, expect: { timeout: 10000 }, reporter: 'list',
  use: { baseURL: `http://127.0.0.1:${port}`, trace: 'retain-on-failure' },
  // Specs use deterministic API/WebSocket fixtures; reserved port 0 makes any
  // unmatched proxy traffic fail without contacting a development Supervisor.
  webServer: { command: `DESKTOP_E2E_CACHE_DIR=node_modules/.vite-device-management-${sharedRevision}-${sharedBuild} VITE_API_PROXY_TARGET=http://127.0.0.1:0 VITE_WS_PROXY_TARGET=ws://127.0.0.1:0 pnpm --filter @pockymoe/supervisor-web exec vite --config src/desktop/testing/vite.config.ts --host 127.0.0.1 --port ${port} --strictPort`, url: `http://127.0.0.1:${port}`, reuseExistingServer: false, cwd: '../..' },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 1000 } } }],
});
