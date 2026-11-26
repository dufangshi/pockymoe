import { request } from '../../../../lib/api';
import { sourceRoot } from '../api';

/** The Supervisor's normalized PR shapes (crates/runtime/src/source_control/review). */
export interface Person { login: string; avatarUrl?: string }
export interface Repository { host: string; owner: string; name: string; kind: string }
export type PullState = 'open' | 'closed' | 'merged';
export type ChecksState = 'passed' | 'failed' | 'pending' | 'none';
export type ReviewDecision = 'approved' | 'changesRequested' | 'reviewRequired';
export interface PullSummary {
  url: string; number: number; title: string; state: PullState; draft: boolean; author: Person | null;
  repository: Repository; headRef: string; baseRef: string; headSha: string; createdAt: string; updatedAt: string;
  checks: ChecksState | null; reviewDecision: ReviewDecision | null; requestedReviewers: string[];
  additions: number | null; deletions: number | null; changedFiles: number | null; comments: number | null;
  labels: { name: string; color?: string }[];
  stack?: StackRef | null;
}
export interface Reviewer { login: string; state: 'requested' | 'approved' | 'changesRequested' | 'commented' | 'dismissed' | 'pending'; team: boolean }
export interface TimelineEntry { kind: 'comment' | 'review'; id: string; author: Person | null; body: string; createdAt: string; url: string | null; state: string | null; commit: string | null }
export interface ThreadComment { id: string; author: Person | null; body: string; createdAt: string; url: string | null }
export interface ReviewThread {
  id: string; path: string; line: number | null; startLine: number | null; side: 'left' | 'right'; subject: 'line' | 'file';
  resolved: boolean; outdated: boolean; canResolve: boolean; canReply: boolean; comments: ThreadComment[];
}
export interface Commit { sha: string; message: string; author: string; login: string | null; date: string; url: string | null }
export interface Check { id: string; name: string; state: 'passed' | 'failed' | 'pending' | 'skipped' | 'neutral' | 'cancelled'; url: string | null; description: string | null; startedAt: string | null; completedAt: string | null; rerun: string | null }
export interface Capabilities {
  submit: ReviewEvent[]; lineComments: boolean; fileComments: boolean; reply: boolean; resolve: boolean; viewedNative: boolean;
  reviewers: boolean; convertToDraft: boolean; ready: boolean; close: boolean; reopen: boolean; autoMerge: boolean;
  merge: boolean; revert: boolean; rerun: boolean; removeCredits: boolean;
}
export interface PullDetail extends PullSummary {
  body: string; nodeId: string | null; baseSha: string | null; mergeable: boolean | null; mergeState: string | null;
  autoMerge: string | null; reviewers: Reviewer[]; timeline: TimelineEntry[]; threads: ReviewThread[]; commits: Commit[];
  checkRuns: Check[]; viewer: string | null; mergeMethods: string[]; capabilities: Capabilities;
}
export interface PullFile { path: string; oldPath: string | null; status: string; additions: number; deletions: number; patch: string | null; binary: boolean; digest: string; viewed: 'unviewed' | 'viewed' | 'changed' }
export interface Draft { id: string; kind: 'line' | 'file' | 'summary'; path: string | null; side: 'left' | 'right' | null; line: number | null; startLine: number | null; body: string; headSha: string | null; createdAt: string; updatedAt: string }
export type ReviewEvent = 'comment' | 'approve' | 'requestChanges';
export interface GuideExcerpt { path: string; side: 'new' | 'old'; start: number; end: number; caption: string; patch: string }
export interface GuideStop {
  title: string; summary: string; attention: { kind: 'tradeoff' | 'contract' | 'product' | 'risk' | 'accessibility'; note: string } | null;
  excerpts: GuideExcerpt[]; files: { path: string; note: string }[]; notes: { path: string; line: number; title: string; body: string }[]; questions: string[]; added?: boolean;
}
export interface Guide { headSha: string; baseSha: string; generatedAt: string; source: string; stops: GuideStop[] }
export interface GuideState { guide: Guide | null; stale: boolean; run: { state: 'running'; headSha: string; startedAt: string } | { state: 'failed'; headSha: string; failedAt: string; detail: string } | null }
export interface DetailResponse {
  detail: PullDetail; drafts: Draft[]; guide: GuideState; linkedThreads: { id: string; title: string }[];
  workspace: { id: string | null; label: string | null } | null; removeAgentCredits: boolean;
  stack?: Stack | null;
}
export interface PageResponse {
  state: string; pulls: PullSummary[]; fetchedAt: string; viewers: Record<string, string | null>;
  repositories: { repository: Repository; workspaceId: string | null; workspaceLabel: string | null; count?: number; error?: string }[];
}
export interface SavedView { id: string; name: string; query: ListQuery }
export interface ListQuery {
  state: 'open' | 'closed' | 'merged' | 'all'; involvement: 'all' | 'authored' | 'reviewing'; author: string;
  draft: 'all' | 'only' | 'hide'; review: 'all' | 'approved' | 'changesRequested' | 'reviewRequired' | 'none';
  checks: 'all' | 'passed' | 'failed' | 'pending'; repository: string; search: string; sort: SortKey;
}
export type SortKey = 'ready' | 'updated' | 'newest' | 'oldest' | 'largest' | 'smallest';

export function fetchPullRequests(deviceId: string | null | undefined, state: string, signal?: AbortSignal) {
  return request<PageResponse>(`${sourceRoot(deviceId)}/source-control/pull-requests?state=${encodeURIComponent(state)}`, signal ? { signal } : undefined);
}
export function reviewAction<T = unknown>(deviceId: string | null | undefined, action: string, url: string, input: Record<string, unknown> = {}, signal?: AbortSignal) {
  return request<T>(`${sourceRoot(deviceId)}/source-control/pull-requests/${action}`, { method: 'POST', body: JSON.stringify({ ...input, url }), ...(signal ? { signal } : {}) });
}
export function fetchViews(deviceId: string | null | undefined) {
  return request<{ views: SavedView[] }>(`${sourceRoot(deviceId)}/source-control/pull-request-views`);
}
export function saveViews(deviceId: string | null | undefined, views: SavedView[]) {
  return request<{ views: SavedView[] }>(`${sourceRoot(deviceId)}/source-control/pull-request-views`, { method: 'PUT', body: JSON.stringify({ views }) });
}

/** A PR's place in a stack; position 1 sits on the base branch. */
export interface StackRef { id: string; position: number; size: number; native: boolean }
export interface StackLayer { number: number; url: string; title: string; state: PullState; draft: boolean; headRef: string; headSha: string; baseRef: string }
/** Bottom to top. `native`: the host keeps the stack (GitHub); else inferred from bases. */
export interface Stack { id: string; native: boolean; base: string; layers: StackLayer[] }
