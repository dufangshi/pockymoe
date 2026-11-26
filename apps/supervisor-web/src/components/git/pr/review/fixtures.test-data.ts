import type { DetailResponse, PullFile, PullSummary } from './reviewApi';

const repo = { host: 'github.com', owner: 'fixture', name: 'parser', kind: 'github' };
export function pull(number: number, over: Partial<PullSummary> = {}): PullSummary {
  return { url: `https://github.com/fixture/parser/pull/${number}`, number, title: `Change ${number}`, state: 'open', draft: false, author: { login: 'ada' }, repository: repo, headRef: `feat/${number}`, baseRef: 'main', headSha: `head-${number}`, createdAt: `2026-11-2${number % 9}T00:00:00Z`, updatedAt: `2026-11-2${number % 9}T10:00:00Z`, checks: 'passed', reviewDecision: null, requestedReviewers: [], additions: number, deletions: 1, changedFiles: 1, comments: 0, labels: [], ...over };
}
export const patch = 'diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n fn a() {}\n-fn b() {}\n+fn b() -> u8 { 1 }\n+fn c() {}\n';
export const files: { headSha: string; viewedOnHost: boolean; files: PullFile[] } = { headSha: 'head-7', viewedOnHost: false, files: [
  { path: 'src/a.rs', oldPath: null, status: 'modified', additions: 2, deletions: 1, patch, binary: false, digest: 'rev-a', viewed: 'unviewed' },
  { path: 'docs/b.md', oldPath: null, status: 'modified', additions: 1, deletions: 0, patch: 'diff --git a/docs/b.md b/docs/b.md\n--- a/docs/b.md\n+++ b/docs/b.md\n@@ -1 +1,2 @@\n x\n+y\n', binary: false, digest: 'rev-b', viewed: 'changed' },
] };
export function detail(over: Partial<DetailResponse['detail']> = {}): DetailResponse {
  return {
    detail: { ...pull(7, { title: 'Stream tokens', headSha: 'head-7' }), body: 'Body text', nodeId: 'node', baseSha: 'base', mergeable: true, mergeState: 'clean', autoMerge: null,
      reviewers: [{ login: 'linus', state: 'requested', team: false }], timeline: [], commits: [{ sha: 'c1', message: 'One', author: 'Ada', login: 'ada', date: '2026-11-26T00:00:00Z', url: null }],
      threads: [{ id: 't1', path: 'src/a.rs', line: 2, startLine: null, side: 'right', subject: 'line', resolved: false, outdated: false, canResolve: true, canReply: true, comments: [{ id: 'x', author: { login: 'grace' }, body: 'Why u8?', createdAt: '2026-11-26T00:00:00Z', url: null }] }],
      checkRuns: [{ id: 'k', name: 'test', state: 'failed', url: 'https://ci.example.com/1', description: null, startedAt: null, completedAt: null, rerun: 'run:1' }],
      viewer: 'me', mergeMethods: ['squash', 'merge'],
      capabilities: { submit: ['comment', 'approve', 'requestChanges'], lineComments: true, fileComments: true, reply: true, resolve: true, viewedNative: false, reviewers: true, convertToDraft: true, ready: true, close: true, reopen: true, autoMerge: true, merge: true, revert: false, rerun: true, removeCredits: true }, ...over },
    drafts: [{ id: 'd1', kind: 'line', path: 'src/a.rs', side: 'right', line: 3, startLine: null, body: 'Name it', headSha: 'head-7', createdAt: '', updatedAt: '' }],
    guide: { guide: null, stale: false, run: null }, linkedThreads: [], workspace: { id: 'ws', label: 'parser' }, removeAgentCredits: true,
  };
}
