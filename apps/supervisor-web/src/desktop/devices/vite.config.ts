import { defineConfig, mergeConfig } from 'vite';
import { fileURLToPath } from 'node:url';
import base from '../../../vite.config';

export default defineConfig(async environment => mergeConfig(
  await (typeof base === 'function' ? base(environment) : base),
  { cacheDir: fileURLToPath(new URL('../../../../../.scratch/vite-devices-18924', import.meta.url)) },
));
