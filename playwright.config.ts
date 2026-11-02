import path from 'node:path';

import { defineConfig, devices } from '@playwright/test';

const realDsh = process.env.E2E_REAL_DSH === '1';
const realCodex = process.env.E2E_REAL_CODEX === '1';
if (realCodex && !process.env.E2E_CODEX_HOME) throw new Error('Real Codex E2E requires an isolated E2E_CODEX_HOME');
if (realDsh && realCodex) throw new Error('Select one real harness for this run');
if (realDsh && !process.env.E2E_DSH_HOME) throw new Error('Real DSH E2E requires an isolated E2E_DSH_HOME');

const apiPort = Number(process.env.E2E_API_PORT ?? 8787);
const webPort = Number(process.env.E2E_WEB_PORT ?? 5173);
const apiBaseUrl = `http://127.0.0.1:${apiPort}`;
const webBaseUrl = `http://localhost:${webPort}`;
const e2eDatabaseUrl = path.resolve(process.env.E2E_DATABASE_URL ?? `.local/e2e-${apiPort}.sqlite`);
const e2eWorkspaceRoot = path.resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright');

export default defineConfig({
  testDir: './e2e',
  timeout: 120_000,
  expect: { timeout: 20_000 },
  fullyParallel: false,
  reporter: 'list',
  use: {
    baseURL: webBaseUrl,
    trace: 'retain-on-failure',
  },
  webServer: [
    {
      command: './target/debug/pockymoe supervisor',
      // Harness subprocesses inherit the live Supervisor's environment. Clear its
      // settings and set both aliases: the prefixed database path takes priority.
      env: {
        ...Object.fromEntries(
          Object.keys(process.env)
            .filter((key) => /^(POCKYMOE|REMOTE_CODEX)_/.test(key))
            .map((key) => [key, '']),
        ),
        POCKYMOE_MODE: 'local',
        POCKYMOE_E2E_FAKE_RUNTIME: realDsh || realCodex ? '' : '1',
        ...(realCodex ? { CODEX_HOME: process.env.E2E_CODEX_HOME!, POCKYMOE_ENABLED_AGENT_PROVIDERS: 'codex' } : {}),
        // DSH's own telemetry stays off in tests (it reports commands such as /feedback).
        ...(realDsh ? { DSH_HOME: process.env.E2E_DSH_HOME!, DSH_TELEMETRY_DISABLED: '1', POCKYMOE_ENABLED_AGENT_PROVIDERS: 'acp' } : {}),
        HOST: '127.0.0.1',
        PORT: String(apiPort),
        POCKYMOE_DATABASE_PATH: e2eDatabaseUrl,
        DATABASE_URL: e2eDatabaseUrl,
        POCKYMOE_WORKSPACE_ROOT: e2eWorkspaceRoot,
        WORKSPACE_ROOT: e2eWorkspaceRoot,
      },
      url: `${apiBaseUrl}/healthz`,
      reuseExistingServer: true,
      timeout: 180_000,
    },
    {
      command: `VITE_API_PROXY_TARGET=${apiBaseUrl} VITE_WS_PROXY_TARGET=ws://127.0.0.1:${apiPort} pnpm --filter @pockymoe/supervisor-web exec vite --force --host localhost --port ${webPort} --strictPort`,
      url: webBaseUrl,
      reuseExistingServer: true,
      timeout: 120_000,
    },
  ],
  projects: [
    { name: 'desktop-webkit', use: { ...devices['Desktop Safari'] } },
    { name: 'mobile-webkit', use: { ...devices['iPhone 13'] } },
    {
      name: 'desktop-chromium',
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'mobile-chromium',
      use: { ...devices['Pixel 5'] },
    },
  ],
});
