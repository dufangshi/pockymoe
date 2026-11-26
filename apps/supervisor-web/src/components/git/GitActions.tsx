import { useEffect, useRef, useState } from 'react';
import { Archive, ArchiveRestore, ArrowDownToLine, ArrowUpFromLine, Download, GitBranch, Plus } from 'lucide-react';
import { confirmAsync, ErrorMark, Select } from '@pockymoe/thread-ui';
import { gitText as gt } from '../../lib/gitI18n';
import { fetchGitBranches, gitWrite, type GitBranches, type GitStatus } from '../../lib/gitApi';

export type Write = (action: string, input?: Record<string, unknown>) => Promise<unknown>;
export function useGitWrite(threadId: string, workingCopyId: string, deviceId: string | null | undefined, running: boolean, refresh: () => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const accepted = useRef(false);
  useEffect(() => { accepted.current = false; }, [running, threadId, workingCopyId]);
  const write: Write = async (action, input = {}) => {
    if (busy) return;
    if (running && !accepted.current) {
      if (!await confirmAsync({ title: 'Git', description: gt('runningConfirm'), confirmLabel: gt('continue') })) return;
      accepted.current = true;
    }
    if (action === 'git/discard' && !await confirmAsync({ title: gt('discard'), description: gt('discardConfirm'), confirmLabel: gt('discard') })) return;
    setBusy(true); setError(null);
    try { const result = await gitWrite(threadId, action, { ...input, workingCopyId, confirmRunning: running && accepted.current }, deviceId); refresh(); return result; }
    catch (caught) { setError(caught); throw caught; }
    finally { setBusy(false); }
  };
  return { write, busy, error };
}
export function GitActions({ threadId, workingCopyId, deviceId, status, write, busy }: { threadId: string; workingCopyId: string; deviceId?: string | null | undefined; status: GitStatus | null; write: Write; busy: boolean }) {
  const [branches, setBranches] = useState<GitBranches | null>(null);
  const [name, setName] = useState('');
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<unknown>(null);
  useEffect(() => { setBranches(null); }, [workingCopyId, status?.branch]);
  const run = (action: string, input?: Record<string, unknown>) => { void write(`git/${action}`, input).catch(() => {}); };
  return <div className="thread-git-actions">
    <button type="button" aria-label={gt('branch')} title={gt('branch')} disabled={busy} onClick={() => { void fetchGitBranches(threadId, workingCopyId, deviceId).then(setBranches).catch(setError); }}><GitBranch aria-hidden="true" /></button>
    {branches && <Select aria-label={gt('branch')} value={status?.branch ?? ''} options={branches.branches.map(branch => ({ value: branch.name, label: `${branch.name} ↑${branch.ahead} ↓${branch.behind}` }))} onValueChange={value => run('checkout', { branch: value })} />}
    <button type="button" aria-label={gt('createBranch')} title={gt('createBranch')} disabled={busy} onClick={() => setCreating(value => !value)}><Plus aria-hidden="true" /></button>
    {creating && <form onSubmit={event => { event.preventDefault(); void write('git/branchCreate', { name: name.trim() }).then(result => { if (result) { setCreating(false); setName(''); } }).catch(() => {}); }}><input aria-label={gt('branchName')} value={name} onChange={event => setName(event.target.value)} /><button disabled={busy || !name.trim()}>{gt('createBranch')}</button></form>}
    {(['fetch', 'pull', 'push', 'stashPush', 'stashPop'] as const).map(action => <button key={action} type="button" aria-label={gt(action)} title={gt(action)} disabled={busy || action === 'stashPop' && !status?.stashCount} onClick={() => run(action, action === 'pull' ? { mode: 'ffOnly' } : action === 'push' ? { setUpstream: !status?.upstream } : {})}>{(() => { const Icon = { fetch: Download, pull: ArrowDownToLine, push: ArrowUpFromLine, stashPush: Archive, stashPop: ArchiveRestore }[action]; return <Icon aria-hidden="true" />; })()}{action === 'push' && !!status?.ahead ? ` ↑${status.ahead}` : action === 'pull' && !!status?.behind ? ` ↓${status.behind}` : action === 'stashPop' && !!status?.stashCount ? ` (${status.stashCount})` : ''}</button>)}
    {error ? <ErrorMark message={gt('branch')} detail={String(error)} /> : null}
  </div>;
}
export function CommitBox({ staged, write, busy }: { staged: boolean; write: Write; busy: boolean }) {
  const [message, setMessage] = useState('');
  const [amend, setAmend] = useState(false);
  return <form className="thread-git-commit-box" onSubmit={event => { event.preventDefault(); void write('git/commit', { message, amend, all: !staged }).then(result => { if (result) setMessage(''); }).catch(() => {}); }}>
    <textarea aria-label={gt('commitMessage')} placeholder={gt('commitMessage')} value={message} onChange={event => setMessage(event.target.value)} />
    <div className="thread-git-actions"><button type="button" disabled={busy} onClick={() => { void write('git/commitMessage').then(result => { if (result && typeof result === 'object' && 'message' in result) setMessage(String(result.message)); }).catch(() => {}); }}>{gt('generate')}</button>
      <Select aria-label={gt('amend')} value={amend ? 'amend' : 'commit'} options={[{ value: 'commit', label: gt('commit') }, { value: 'amend', label: gt('amend') }]} onValueChange={value => setAmend(value === 'amend')} />
      <button type="submit" disabled={busy || !message.trim()}>{gt(staged ? 'commit' : 'commitAll')}</button></div>
  </form>;
}
