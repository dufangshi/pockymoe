import { ApiError, deviceRequest } from './api';

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
  upstream?: string | null;
  ahead?: number;
  behind?: number;
  conflicted?: string[];
  stashCount?: number;
  operation?: string | null;
  truncated: boolean;
}
/** `commit`: what the commit named by `base` changed against its first parent. */
export type GitDiffArea = 'worktree' | 'index' | 'branch' | 'commit' | 'turn';
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
export function fetchThreadGitDiff(id: string, workingCopyId: string, input: { area: GitDiffArea; path?: string; base?: string; turnId?: string; ignoreWhitespace?: boolean }, deviceId?: string | null, signal?: AbortSignal) {
  const params = new URLSearchParams({ workingCopyId, area: input.area });
  if (input.path) params.set('path', input.path);
  if (input.base) params.set('base', input.base);
  if (input.turnId) params.set('turnId', input.turnId);
  if (input.ignoreWhitespace) params.set('ignoreWhitespace', '1');
  return deviceRequest<GitDiff>(deviceId, `${threadPath(id)}/git/diff?${params}`, { cache: 'no-store', ...(signal ? { signal } : {}) }, id);
}
export function fetchThreadGitCommits(id: string, workingCopyId: string, offset: number, deviceId?: string | null, signal?: AbortSignal) {
  return deviceRequest<GitCommits>(deviceId, `${threadPath(id)}/git/commits?${new URLSearchParams({ workingCopyId, limit: '30', offset: String(offset) })}`, { cache: 'no-store', ...(signal ? { signal } : {}) }, id);
}

export interface TurnCheckpoint {
  turnId: string; turnIndex: number; status: string; startedAt: string; completedAt?: string;
  workingCopies: { workingCopyId: string; available: boolean; reason?: string; files: number; additions: number; deletions: number }[];
}
export interface GitBranches { current: string | null; detached: boolean; branches: { name: string; upstream?: string; ahead: number; behind: number; remote: boolean }[] }
export function fetchCheckpoints(id: string, deviceId?: string | null, signal?: AbortSignal) {
  return deviceRequest<{ turns: TurnCheckpoint[] }>(deviceId, `${threadPath(id)}/checkpoints`, { cache: 'no-store', ...(signal ? { signal } : {}) }, id);
}
export function fetchGitBranches(id: string, workingCopyId: string, deviceId?: string | null) {
  return deviceRequest<GitBranches>(deviceId, `${threadPath(id)}/git/branches?${new URLSearchParams({ workingCopyId })}`, {}, id);
}
export async function gitWrite<T = { status: GitStatus }>(id: string, action: string, input: Record<string, unknown>, deviceId?: string | null) {
  const send = () => deviceRequest<T>(deviceId, `${threadPath(id)}/${action}`, { method: 'POST', body: JSON.stringify(input) }, id);
  try { return await send(); }
  catch (error) {
    // A definitive refusal before any write is safe to retry once while the
    // thread finishes a capability load or its turn settlement.
    if (!(error instanceof ApiError) || error.statusCode !== 409 || error.payload.code !== 'checkpoint_busy' || !['checkpoints/restore', 'checkpoints/undo'].includes(action)) throw error;
    await new Promise(resolve => setTimeout(resolve, 750));
    return send();
  }
}
