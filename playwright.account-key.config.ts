import { defineConfig } from '@playwright/test';
import config from './playwright.config';

// Automatic ARIA prompts can include recovery codes and binding commands.
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';

// These specs start a real relay and disposable relay-connected Supervisors.
// The normal config's local Supervisor/Vite servers would occupy their ports.
export default defineConfig({
  ...config,
  testMatch: ['account-key-trust.spec.ts', 'signed-sharing-and-peers.spec.ts', 'one-click-protection.spec.ts', 'relay-admin.spec.ts'],
  webServer: [],
  workers: 1,
  retries: 0,
});
