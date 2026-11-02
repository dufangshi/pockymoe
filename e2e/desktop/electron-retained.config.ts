import { defineConfig } from '@playwright/test';
import config from '../../playwright.account-key.config';
export default defineConfig({
  ...config, testDir: '.', testMatch: 'electron-retained.pw.ts',
  outputDir: '../../.scratch/device-management/review-electron/playwright',
  webServer: [], workers: 1, projects: [{ name: 'electron' }],
});
