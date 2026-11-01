import { defineConfig, mergeConfig } from 'vite';
import base from '../../../vite.config';
export default defineConfig(async env => mergeConfig(await (typeof base === 'function' ? base(env) : base), { cacheDir: process.env.DESKTOP_E2E_CACHE_DIR ?? 'node_modules/.vite-desktop-webnav' }));
