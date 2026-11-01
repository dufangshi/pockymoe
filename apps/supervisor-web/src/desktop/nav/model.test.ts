import { describe, expect, it } from 'vitest';
import type { DesktopIndex, DesktopThread } from '../data/types';
import { buildTree, freezeRows, fuzzyMatch, type TreeOptions } from './model';
const now = Date.parse('2026-11-01T12:00:00Z');
const thread = (id: string, fields: Partial<DesktopThread> = {}): DesktopThread => ({ key: `d:${id}`, deviceId: 'd', id, title: id, workspaceId: 'w', provider: 'codex', status: 'idle', isPinned: false, createdAt: '2026-10-30T12:00:00Z', updatedAt: '2026-10-31T12:00:00Z', lastTurnStartedAt: null, lastTurnCompletedAt: null, activeTurnId: null, lastViewedAt: null, requests: [], resultSummary: null, ...fields });
const index = (threads: DesktopThread[]): DesktopIndex => ({ accountId: 'a', devices: [{ id: 'd', name: 'MacBook', connected: true, lastHeartbeatAt: null }], workspaces: [{ key: 'd:w', deviceId: 'd', id: 'w', label: 'Project', isFavorite: false, createdAt: '2026-10-30T12:00:00Z', lastOpenedAt: null }], threads, hydrated: true, refreshing: false, error: null });
const options: TreeOptions = { now, query: '', scope: 'all', provider: '', expanded: {} };
const treeThreads = (rows: ReturnType<typeof buildTree>) => rows.filter(row => row.id.startsWith('thread:'));
describe('desktop tree model', () => {
  it('matches fuzzy subsequences and coalesces highlight ranges', () => {
    expect(fuzzyMatch('Remote Codex', 'rmt cd')).toEqual([[0, 1], [2, 3], [4, 5], [7, 8], [9, 10]]);
    expect(fuzzyMatch('codex', 'code')).toEqual([[0, 4]]); expect(fuzzyMatch('codex', 'zzz')).toBeNull();
  });
  it('compresses focus sets, keeping pinned and active plus five recent roots', () => {
    const threads = Array.from({ length: 10 }, (_, i) => thread(String(i)));
    threads.push(thread('pin', { isPinned: true }), thread('run', { status: 'running' }));
    const rows = buildTree(index(threads), options);
    expect(treeThreads(rows)).toHaveLength(7); expect(rows.find(row => row.kind === 'more')?.label).toBe('… 其余 5 个线程');
    expect(treeThreads(buildTree(index(threads), { ...options, expanded: { 'more:d:w': true } }))).toHaveLength(12);
  });
  it('retains ancestors and automatically reveals a deeply nested active agent', () => {
    const rows = buildTree(index([thread('root'), thread('child', { parentThreadId: 'root' }), thread('agent', { parentThreadId: 'child', status: 'running' })]), { ...options, scope: 'active' });
    expect(treeThreads(rows).map(row => row.threadKey)).toEqual(['d:root', 'd:child', 'd:agent']);
    expect(treeThreads(rows)[2]?.depth).toBe(4);
  });
  it('hides closed agents behind an explicit disclosure', () => {
    const data = index([thread('root'), thread('agent', { parentThreadId: 'root', closedAt: '2026-10-31T12:00:00Z' })]);
    const expanded = { 'thread:d:root': true };
    expect(treeThreads(buildTree(data, { ...options, expanded }))).toHaveLength(1);
    expect(buildTree(data, { ...options, expanded }).find(row => row.kind === 'closed')?.label).toBe('显示已关闭的 1 个');
    expect(treeThreads(buildTree(data, { ...options, expanded: { ...expanded, 'closed:d:root': true } }))).toHaveLength(2);
  });
  it('groups inactive workspaces after 14 days and keeps favorites visible', () => {
    const data = index([thread('old', { updatedAt: '2026-09-01T00:00:00Z', lastTurnCompletedAt: '2026-09-01T00:00:00Z' })]);
    data.workspaces = [{ ...data.workspaces[0]!, createdAt: '2026-09-01T00:00:00Z' }];
    expect(buildTree(data, options).some(row => row.kind === 'inactive')).toBe(true);
    expect(buildTree(data, options).some(row => row.kind === 'workspace')).toBe(false);
    data.workspaces = [{ ...data.workspaces[0]!, isFavorite: true }];
    expect(buildTree(data, options).some(row => row.kind === 'workspace')).toBe(true);
  });
  it('filters title, agent, workspace and device with provider/scope constraints', () => {
    const data = index([thread('root'), thread('child', { parentThreadId: 'root', agentName: 'reviewer', isPinned: true, provider: 'claude' })]);
    expect(treeThreads(buildTree(data, { ...options, query: 'review', provider: 'claude', scope: 'pinned' }))).toHaveLength(2);
    expect(treeThreads(buildTree(data, { ...options, query: 'MacBook' }))).toHaveLength(2);
    expect(treeThreads(buildTree(data, { ...options, query: 'unknown' }))).toHaveLength(0);
  });
  it('sorts online devices first and pinned threads first with deterministic ties', () => {
    const data = index([thread('a'), thread('z', { isPinned: true })]);
    data.devices = [{ id: 'offline', name: 'AAA', connected: false, lastHeartbeatAt: null }, ...data.devices];
    const rows = buildTree(data, options);
    expect(rows.filter(row => row.kind === 'device').map(row => row.deviceId)).toEqual(['d', 'offline']);
    expect(treeThreads(rows)[0]?.threadKey).toBe('d:z');
  });
  it('rolls activity up on collapsed ancestors and filters failed attention at 24h', () => {
    const data = index([thread('run', { status: 'running' }), thread('fail', { status: 'failed', lastTurnCompletedAt: '2026-11-01T11:00:00Z' })]);
    const rows = buildTree(data, { ...options, expanded: { 'device:d': false } });
    expect(rows.find(row => row.kind === 'device')?.rollup).toEqual({ running: 1, attention: 1, unread: 1 });
    expect(rows.find(row => row.id === 'smart:attention')?.count).toBe(1);
    expect(buildTree(data, { ...options, now: now + 86_400_000 }).find(row => row.id === 'smart:attention')?.count).toBe(0);
  });
  it('freezes ordering and insertions but updates visible status, then accepts new order', () => {
    const previous = buildTree(index([thread('a'), thread('b')]), options);
    const desired = buildTree(index([thread('b', { isPinned: true }), thread('a'), thread('new')]), options);
    const frozen = freezeRows(previous, desired, true);
    expect(frozen.map(row => row.id)).toEqual(previous.map(row => row.id));
    expect(freezeRows(previous, desired, false)).toBe(desired);
  });
});
