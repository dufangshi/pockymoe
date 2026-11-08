import { deviceRequest } from './api';

export interface WorkingCopy {
  id: string;
  repositoryId: string;
  path: string;
  branch: string | null;
  head: string | null;
  managed: boolean;
  source: 'configured' | 'created' | 'adopted' | 'observed';
  lastObservedAt: string | null;
}
export interface WorkingObservation {
  workingCopyId: string;
  cwd: string;
  source: string;
  toolCallId: string | null;
  agentId: string | null;
  observedAt: string;
}
export interface ThreadWorkingContext {
  configuredCwd: string;
  primaryWorkingCopyId: string | null;
  workingCopies: WorkingCopy[];
  observations: WorkingObservation[];
}
export interface GitStatus {
  workingCopyId: string;
  branch: string | null;
  head: string | null;
  files: { path: string; indexStatus: string; worktreeStatus: string; originalPath?: string }[];
  truncated: boolean;
}
export type GitDiffArea = 'worktree' | 'index' | 'branch';
export interface GitDiff {
  workingCopyId: string;
  area: GitDiffArea;
  patch: string;
  truncated: boolean;
}
export interface GitCommit {
  oid: string;
  subject: string;
  author: string;
  authoredAt: string;
}
export interface GitCommits { commits: GitCommit[]; hasMore: boolean }

const threadPath = (id: string) => `api/threads/${encodeURIComponent(id)}`;
export function fetchThreadWorkingContext(id: string, deviceId?: string | null, signal?: AbortSignal) {
  return deviceRequest<ThreadWorkingContext>(deviceId, `${threadPath(id)}/working-context`, { cache: 'no-store', ...(signal ? { signal } : {}) }, id);
}
export function fetchThreadGitStatus(id: string, workingCopyId: string, deviceId?: string | null, signal?: AbortSignal) {
  return deviceRequest<GitStatus>(deviceId, `${threadPath(id)}/git/status?${new URLSearchParams({ workingCopyId })}`, { cache: 'no-store', ...(signal ? { signal } : {}) }, id);
}
export function fetchThreadGitDiff(id: string, workingCopyId: string, input: { area: GitDiffArea; path?: string; base?: string }, deviceId?: string | null, signal?: AbortSignal) {
  const params = new URLSearchParams({ workingCopyId, area: input.area });
  if (input.path) params.set('path', input.path);
  if (input.base) params.set('base', input.base);
  return deviceRequest<GitDiff>(deviceId, `${threadPath(id)}/git/diff?${params}`, { cache: 'no-store', ...(signal ? { signal } : {}) }, id);
}
export function fetchThreadGitCommits(id: string, workingCopyId: string, offset: number, deviceId?: string | null, signal?: AbortSignal) {
  return deviceRequest<GitCommits>(deviceId, `${threadPath(id)}/git/commits?${new URLSearchParams({ workingCopyId, limit: '30', offset: String(offset) })}`, { cache: 'no-store', ...(signal ? { signal } : {}) }, id);
}
