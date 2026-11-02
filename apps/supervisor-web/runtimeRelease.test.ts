// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import configure from './vite.config';
import { resolveConfig } from 'vite';

afterEach(() => vi.unstubAllEnvs());
it('portable release overrides an inherited deployment origin while regular builds retain configuration', async () => {
  vi.stubEnv('VITE_POCKYMOE_RELAY_ORIGIN', 'https://relay.example.com');
  vi.stubEnv('POCKYMOE_RUNTIME_RELEASE', '1');
  const config = await resolveConfig({ configFile: false, ...await (configure as Function)({ mode: 'production' }) }, 'build', 'production');
  expect(config.define['import.meta.env.VITE_POCKYMOE_RELAY_ORIGIN']).toBe('""');
  vi.stubEnv('POCKYMOE_RUNTIME_RELEASE', '');
  const configured = await resolveConfig({ configFile: false, ...await (configure as Function)({ mode: 'production' }) }, 'build', 'production');
  expect(configured.env.VITE_POCKYMOE_RELAY_ORIGIN).toBe('https://relay.example.com');
  expect(configured.define['import.meta.env.VITE_POCKYMOE_RELAY_ORIGIN']).toBeUndefined();
});
