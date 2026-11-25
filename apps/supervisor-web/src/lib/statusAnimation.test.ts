// @vitest-environment node
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, it } from 'vitest';

// Request/loading-only exceptions. Persistent status indicators stay static.
// See docs/ui-principles.md §6.
const allowed = new Map([
  ['apps/supervisor-web/src/index.css', ['product-skeleton-pulse']],
  ['apps/supervisor-web/src/components/ResourceSettings.css', ['pm-res-spin']],
  ['apps/supervisor-web/src/desktop/devices/devices.css', ['pd-spin']],
  ['pockymoe-thread-ui/packages/thread-ui/src/styles/request-animations.css', ['request-spin', 'request-pulse']],
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
    if (/animate-(?:ping|bounce|spin|pulse)/.test(source)) unexpected.push(`${file}: unsupported perpetual utility`);
    for (const match of source.matchAll(/animation(?:-[\w-]+)?\s*:\s*([^;{}]*\binfinite\b[^;{}]*)/gi)) {
      const name = match[1]!.trim().split(/\s+/)[0]!;
      if (!allowed.get(file)?.includes(name)) unexpected.push(`${file}: ${match[0]}`);
    }
    if (/data-beat|installStatusBeat|STATUS_BEAT_MS/.test(source)) unexpected.push(`${file}: removed status clock`);
    if (/iterations\s*:\s*Infinity/.test(source)) unexpected.push(`${file}: perpetual Web Animation`);
  }
  expect(unexpected).toEqual([]);
});

it('keeps request animation classes in the reviewed request-only files', () => {
  const uses = files('apps/supervisor-web/src').filter(file => /\.tsx$/.test(file) && /request-(?:spin|pulse)/.test(readFileSync(path.join(root, file), 'utf8'))).map(file => file.replace('apps/supervisor-web/src/', '')).sort();
  expect(uses.sort()).toEqual(["components/ConnectionModelField.tsx", "components/DokploySettings.tsx", "components/PluginsSettings.tsx", "components/ThreadAutomationsControl.tsx", "components/environments/ThreadIsolationPanel.tsx", "components/git/ThreadGitPanel.tsx", "pages/RelayAccountPage.tsx", "pages/RelayHomePage.tsx", "pages/WorkspacesPage.tsx"]);
});
