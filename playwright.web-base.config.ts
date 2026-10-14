import { defineConfig } from '@playwright/test';
import { resolve } from 'node:path';
import config from './playwright.config';

// This spec consumes the built UI served by the same isolated fake Supervisor.
// Avoid starting Vite: its dev asset handling cannot prove installed bundles.
const supervisor = Array.isArray(config.webServer) ? config.webServer[0]! : config.webServer!;
export default defineConfig({
  ...config,
  webServer: [{
    ...supervisor,
    env: { ...supervisor.env, POCKYMOE_WEB_DIST_DIR: resolve('apps/supervisor-web/dist') },
    reuseExistingServer: false,
  }],
});
