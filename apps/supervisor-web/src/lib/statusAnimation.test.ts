// @vitest-environment node
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, it } from 'vitest';

// Request/loading-only exceptions. A new perpetual indicator must use the
// document beat, not expand this list. See docs/ui-principles.md §6.
const allowed = new Map([
  ['apps/supervisor-web/src/index.css', ['product-skeleton-pulse']],
  ['apps/supervisor-web/src/components/ResourceSettings.css', ['pm-res-spin']],
  ['apps/supervisor-web/src/desktop/devices/devices.css', ['pd-spin']],
  ['pockymoe-thread-ui/packages/thread-ui/src/styles/base.css', ['history-lift']],
  ['pockymoe-thread-ui/packages/thread-ui/src/styles/timeline-shell.css', ['thread-loading-rotate', 'thread-loading-pulse']],
  ['pockymoe-thread-ui/packages/thread-ui/src/styles/layout-workspace.css', ['thread-graph-download-sweep']],
  ['pockymoe-thread-ui/packages/thread-ui/src/styles/matter-workbench.css', ['workbench-picker-spin']],
]);
const root = fileURLToPath(new URL('../../../../', import.meta.url));
function files(dir: string): string[] {
  return readdirSync(path.join(root, dir), { withFileTypes: true }).flatMap(entry => {
    const name = `${dir}/${entry.name}`;
    return entry.isDirectory() ? files(name) : /\.(css|tsx?|jsx?)$/.test(name) && !/\.(test|spec)\./.test(name) ? [name] : [];
  });
}
it('allows infinite animations only in the explicit request/loading list', () => {
  const unexpected: string[] = [];
  for (const file of ['apps/supervisor-web/src', 'themes', 'pockymoe-thread-ui/packages/thread-ui/src'].flatMap(files)) {
    const source = readFileSync(path.join(root, file), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    if (/animate-(?:ping|bounce)/.test(source)) unexpected.push(`${file}: unsupported perpetual utility`);
    for (const match of source.matchAll(/animation(?:-[\w-]+)?\s*:\s*([^;{}]*\binfinite\b[^;{}]*)/gi)) {
      const name = match[1]!.trim().split(/\s+/)[0]!;
      if (!allowed.get(file)?.includes(name)) unexpected.push(`${file}: ${match[0]}`);
    }
    if (/iterations\s*:\s*Infinity/.test(source)) unexpected.push(`${file}: perpetual Web Animation`);
  }
  expect(unexpected).toEqual([]);
});
