import { defineConfig, devices } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const repo = fileURLToPath(new URL('../../../', import.meta.url));
const manifest = process.env.POCKY_ENV_RUN_MANIFEST;
if (!manifest) throw new Error('A dedicated task lab manifest is required');
const run = JSON.parse(readFileSync(manifest, 'utf8'));
if (!run.labPrefix.startsWith('pocky-env-af-')) throw new Error('Dedicated synthetic lab only');
const port = Number(process.env.DOKPLOY_UI_WEB_PORT ?? 16464);
export default defineConfig({
  testDir: '.', testMatch: 'environment-owner.spec.ts', workers: 1, retries: 0, timeout: 120000,
  expect: { timeout: 12000 }, reporter: 'list', outputDir: '../../../.scratch/git-environment-feasibility/agent-first/owner-browser',
  // Login uses generated private credentials. Never persist a login trace/video.
  use: { baseURL: `http://localhost:${port}`, actionTimeout: 12000, trace: 'off', video: 'off' },
  webServer: { cwd: repo, command: `corepack pnpm --filter @pockymoe/supervisor-web exec vite --host localhost --port ${port} --strictPort`,
    env: { VITE_API_PROXY_TARGET: run.apiBaseUrl, VITE_WS_PROXY_TARGET: run.apiBaseUrl.replace('http:', 'ws:') },
    url: `http://localhost:${port}`, reuseExistingServer: false, timeout: 60000 },
  projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'] } }],
});
