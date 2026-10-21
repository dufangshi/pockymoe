// @vitest-environment node
import { describe, expect, it } from 'vitest';
import type { RelayEffectiveAccessDto } from '@pockymoe/shared';
import { canUseTerminal, filesConfinedToWorkspace, newThreadScope, pathInsideWorkspace, workspaceShare } from './shareRules';

const owner: RelayEffectiveAccessDto = { kind: 'owner', shareId: null, scope: 'owner', threadAccess: 'control', workspaceAccess: 'write', workspaceId: null };
const shared = (scope: 'thread' | 'workspace' | 'device', patch: Partial<RelayEffectiveAccessDto> = {}): RelayEffectiveAccessDto => ({
  kind: 'shared', shareId: null, grantId: 'g', scope, threadAccess: 'control', workspaceAccess: 'write',
  workspaceId: scope === 'device' ? null : 'ws', workspaceScope: 'all', canCreateThreads: true, ...patch,
});

describe('share rules', () => {
  it('keeps the terminal for the owner and never shows it to a share', () => {
    expect(canUseTerminal(owner, true)).toBe(true);
    expect(canUseTerminal(null, false)).toBe(true);
    for (const scope of ['thread', 'workspace', 'device'] as const)
      for (const threadAccess of ['read', 'control'] as const)
        expect(canUseTerminal(shared(scope, { threadAccess }), true)).toBe(false);
    // Until the relay answers, a relay route shows no terminal.
    expect(canUseTerminal(null, true)).toBe(false);
  });

  it('confines thread and workspace shares to their workspace', () => {
    expect(filesConfinedToWorkspace(owner, true)).toBe(false);
    expect(filesConfinedToWorkspace(null, false)).toBe(false);
    expect(filesConfinedToWorkspace(shared('thread'), true)).toBe(true);
    expect(filesConfinedToWorkspace(shared('workspace'), true)).toBe(true);
    // A whole device shared with write keeps reaching linked files outside workspaces.
    expect(filesConfinedToWorkspace(shared('device'), true)).toBe(false);
    expect(filesConfinedToWorkspace(shared('device', { workspaceAccess: 'read' }), true)).toBe(true);
    for (const path of ['src/a.ts', 'a.md', 'docs/./b.md'])
      expect(pathInsideWorkspace(path)).toBe(true);
    for (const path of ['', '/etc/passwd', '../secret', 'src/../../x', 'C:/Users/me', 'c:\\x', 'a/..', 'x\0y'])
      expect(pathInsideWorkspace(path)).toBe(false);
  });

  it('starts threads only with control and the choice, and a workspace share only there', () => {
    expect(newThreadScope(owner, true)).toEqual({ allowed: true, workspaceId: null });
    expect(newThreadScope(shared('device'), true)).toEqual({ allowed: true, workspaceId: null });
    expect(newThreadScope(shared('workspace'), true)).toEqual({ allowed: true, workspaceId: 'ws' });
    expect(newThreadScope(shared('workspace', { canCreateThreads: false }), true).allowed).toBe(false);
    expect(newThreadScope(shared('workspace', { threadAccess: 'read' }), true).allowed).toBe(false);
    expect(newThreadScope(shared('thread'), true).allowed).toBe(false);
    expect(workspaceShare(shared('workspace'))).toBe(true);
    expect(workspaceShare(shared('workspace', { workspaceId: null }))).toBe(false);
    expect(workspaceShare(shared('device'))).toBe(false);
  });
});
