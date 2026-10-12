import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';

import { compileThemePack, validatePackFiles } from './pack';

const root = path.resolve(__dirname, '../../../../../themes/plum-pocket');
function files(dir: string, prefix = ''): [string, Uint8Array][] {
  return readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    return statSync(full).isDirectory() ? files(full, `${prefix}${name}/`) : [[`${prefix}${name}`, new Uint8Array(readFileSync(full))] as [string, Uint8Array]];
  });
}

describe('built-in Plum Pocket pack', () => {
  it('validates and compiles with prefixed keyframes and fonts', () => {
    const pack = validatePackFiles(files(root));
    const css = compileThemePack(pack, (p) => `/themes/plum-pocket/${p}?v=test`);
    if (process.env.DUMP_PLUM) require('node:fs').writeFileSync(process.env.DUMP_PLUM, css);
    expect(css).not.toContain(':theme');
    expect(css).not.toContain('asset:');
    expect(css).toContain('@keyframes plum-pocket--plum-shell-hop');
    expect(css).toContain('animation: plum-pocket--plum-shell-hop 700ms ease');
    expect(css).toContain('"plum-pocket--Nunito"');
    expect(css).toContain(':root:root:root[data-theme-preset="plum-pocket"]');
    expect(css).toContain('--theme-agent-avatar: url("/themes/plum-pocket/assets/agents/codex.webp?v=test")');
  });
});
