import { defineConfig, devices } from '@playwright/test';
// This spec owns its real Relay/Supervisor and an independent Electron profile.
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
export default defineConfig({ testDir: '.', testMatch: 'account-shell.spec.ts', timeout: 180000, expect: { timeout: 30000 }, workers: 1, retries: 0, reporter: 'list', outputDir: '../.scratch/account-shell/playwright', use: { actionTimeout: 15000, navigationTimeout: 15000, trace: 'off', screenshot: 'off', video: 'off' }, projects: [{ name: 'desktop-chromium', use: { ...devices['Desktop Chrome'] } }] });
