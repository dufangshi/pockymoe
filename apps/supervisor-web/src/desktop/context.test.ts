import { expect, it } from 'vitest';
import { hasWorkspaceContext } from './context';
it.each([
  ['/', '', false], ['/desktop/devices', '', false], ['/relay-account', '', false], ['/relay-portal', '', false],
  ['/devices/a/workspaces', '', false], ['/devices/a/threads/new', '', false], ['/devices/a/threads/import', '', false],
  ['/threads/t', '', true], ['/devices/a/threads/t', '', true], ['/devices/a/threads', '?workspaceId=w', true],
  ['/threads', '?workspaceId=w', true], ['/threads', '', false], ['/devices/a/threads/t/', '', true],
] as const)('gates workspace controls for %s%s', (path, search, expected) => {
  expect(hasWorkspaceContext(path, search)).toBe(expected);
});
