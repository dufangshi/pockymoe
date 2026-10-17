import { describe, expect, it } from 'vitest';
import type { RelayEffectiveAccessDto } from '@pockymoe/shared';
import { canReadLinkedFiles } from './api';

const shared = (scope: RelayEffectiveAccessDto['scope'], workspaceAccess: RelayEffectiveAccessDto['workspaceAccess']) =>
  ({ kind: 'shared', shareId: null, scope, threadAccess: 'control', workspaceAccess, workspaceId: null }) as RelayEffectiveAccessDto;

describe('canReadLinkedFiles', () => {
  it('opens files outside the workspace for the owner and a whole device shared with write access', () => {
    expect(canReadLinkedFiles({ kind: 'owner', shareId: null, threadAccess: 'control', workspaceAccess: 'write', workspaceId: null })).toBe(true);
    expect(canReadLinkedFiles(shared('device', 'write'))).toBe(true);
    expect(canReadLinkedFiles(shared('device', 'read'))).toBe(false);
    expect(canReadLinkedFiles(shared('workspace', 'write'))).toBe(false);
    expect(canReadLinkedFiles(shared('thread', 'write'))).toBe(false);
    expect(canReadLinkedFiles(null)).toBe(false);
  });
});
