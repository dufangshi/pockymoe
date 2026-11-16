import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const shared = resolve(root, process.env.STATUS_SHARED_DIST ?? 'pockymoe-thread-ui/packages/thread-ui/dist');
const web = resolve(root, process.env.STATUS_WEB_DIST ?? 'apps/supervisor-web/dist');
function files(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap(entry => entry.isDirectory()
    ? files(join(dir, entry.name)) : /\.(js|css)$/.test(entry.name) ? [join(dir, entry.name)] : []);
}
function check(label, dir) {
  // tsup injects CSS as string literals; Vite may retain the injected strings or
  // emit CSS assets. Inspect the delivered bytes, never the source stylesheets.
  const built = files(dir).map(file => readFileSync(file, 'utf8')).join('\n')
    .replace(/\\[nrt]/g, ' ').replace(/[\s'"\\]/g, '');
  for (const fragment of [
    '.matter-status-dot[data-status=running]',
    '.matter-status-dot[data-status=agents-running]',
    'box-shadow:inset0001.5pxvar(--thread-map-live)',
    'box-shadow:0002pxvar(--tab-ring)',
    '.matter-group-toggle.is-badge[data-running]>svg{color:var(--matter-agents-glow,#20bc87)',
    '.request-spin{animation:request-spin',
    '.request-pulse{animation:request-pulse',
  ]) assert.ok(built.includes(fragment), `${label}: built status rule missing: ${fragment}`);
  for (const removed of ['data-beat', 'installStatusBeat', 'STATUS_BEAT_MS']) {
    assert.ok(!built.includes(removed), `${label}: removed status clock remains: ${removed}`);
  }
  console.log(`${label}: static running styles and request exceptions present; no status clock`);
}
check('tracked shared UI', shared);
if (!process.argv.includes('--shared-only')) check('production Web', web);
