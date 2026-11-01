import { defineConfig, mergeConfig } from 'vite';
import base from '../../../vite.config';
// Each delegate's dev server owns its dependency cache; shared UI rebuilds must
// not reuse an older optimized bundle or invalidate another engineer's server.
export default defineConfig(async env => mergeConfig(await (typeof base === 'function' ? base(env) : base), { cacheDir: '/tmp/rc-desktop/evidence/webnav/v2/vite-cache' }));
