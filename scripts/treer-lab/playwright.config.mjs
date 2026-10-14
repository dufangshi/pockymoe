import { createRequire } from 'node:module';
import { stateDir, treerSource } from './common.mjs';
const require = createRequire(`${treerSource}/web/package.json`);
const { defineConfig } = require('@playwright/test');
export default defineConfig({
  testDir: '.', testMatch: 'tunnel.spec.mjs', timeout: 45_000,
  retries: 0, workers: 1, reporter: 'line', outputDir: `${stateDir}/browser-results`,
  projects: [{ name: 'desktop-chromium', use: { browserName: 'chromium', viewport: { width: 1280, height: 900 } } }],
  // Cookies and injected credentials must never be captured in traces/storage state.
  use: { trace: 'off', video: 'off', screenshot: 'off' },
});
