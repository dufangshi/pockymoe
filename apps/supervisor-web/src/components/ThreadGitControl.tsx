import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { GitBranch, RefreshCw, X } from 'lucide-react';
import { CopyButton, ErrorMark, MiddleName, PathChip, Select, useModalFocus } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ApiError } from '../lib/api';
import {
  fetchThreadGitCommits, fetchThreadGitDiff, fetchThreadGitStatus, fetchThreadWorkingContext,
  type GitCommits, type GitDiff, type GitDiffArea, type GitStatus, type ThreadWorkingContext,
} from '../lib/gitApi';
import './thread-git.css';

export interface ThreadGitControlProps {
  threadId: string;
  deviceId?: string | null | undefined;
  /** Existing thread updates invalidate the open panel; no polling is started. */
  refreshKey?: string | undefined;
}

export function ThreadGitControl({ threadId, deviceId, refreshKey }: ThreadGitControlProps) {
  useI18n();
  const [open, setOpen] = useState(false);
  return <>
    <button type="button" aria-label={translate('workbench.gitOpen')} title={translate('workbench.gitOpen')} onClick={() => setOpen(true)}>
      <GitBranch aria-hidden="true" /><span className="sr-only">Git</span>
    </button>
    {open && <ThreadGitDialog key={`${deviceId ?? 'local'}:${threadId}`} threadId={threadId} deviceId={deviceId} refreshKey={refreshKey} onClose={() => setOpen(false)} />}
  </>;
}

function ThreadGitDialog({ threadId, deviceId, refreshKey, onClose }: ThreadGitControlProps & { onClose(): void }) {
  const { locale } = useI18n();
  const dialog = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useModalFocus(dialog, true, onClose, '[data-git-close]');
  const [context, setContext] = useState<ThreadWorkingContext | null>(null);
  const [selected, setSelected] = useState('');
  const [tab, setTab] = useState<'changes' | 'commits'>('changes');
  const [area, setArea] = useState<GitDiffArea>('worktree');
  const [path, setPath] = useState('');
  const [baseDraft, setBaseDraft] = useState('HEAD');
  const [base, setBase] = useState('HEAD');
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [diff, setDiff] = useState<GitDiff | null>(null);
  const [commits, setCommits] = useState<GitCommits | null>(null);
  const [offset, setOffset] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<unknown>(null);
  const [revision, setRevision] = useState(0);
  const [visible, setVisible] = useState(document.visibilityState !== 'hidden');
  const showFailure = useRef(true);
  const loadedScope = useRef('');
  const refresh = useCallback(() => { showFailure.current = true; setRevision(value => value + 1); }, []);
  useEffect(() => {
    const update = () => { const next = document.visibilityState !== 'hidden'; setVisible(next); if (next) { showFailure.current = false; setRevision(value => value + 1); } };
    document.addEventListener('visibilitychange', update);
    return () => document.removeEventListener('visibilitychange', update);
  }, [refresh]);

  useEffect(() => {
    if (!visible) return;
    const controller = new AbortController();
    setError(null);
    setLoading(true);
    void fetchThreadWorkingContext(threadId, deviceId, controller.signal).then(value => {
      if (controller.signal.aborted) return;
      setContext(value);
      setSelected(current => value.workingCopies.some(copy => copy.id === current) ? current : value.primaryWorkingCopyId ?? value.workingCopies[0]?.id ?? '');
    }).catch(caught => { if (!controller.signal.aborted) { if (showFailure.current) setError(caught); setLoading(false); } });
    return () => controller.abort();
  }, [threadId, deviceId, revision, refreshKey, visible]);

  useEffect(() => {
    if (!visible || !context) return;
    if (!selected) { setLoading(false); return; }
    const controller = new AbortController();
    setLoading(true);
    setError(null);
    const scope = JSON.stringify([selected, tab, area, path, base, offset]);
    if (scope !== loadedScope.current) { setStatus(null); setDiff(null); setCommits(null); showFailure.current = true; }
    loadedScope.current = scope;
    const operation = tab === 'changes'
      ? Promise.all([
        fetchThreadGitStatus(threadId, selected, deviceId, controller.signal),
        fetchThreadGitDiff(threadId, selected, { area, ...(path ? { path } : {}), ...(area === 'branch' ? { base } : {}) }, deviceId, controller.signal),
      ]).then(([nextStatus, nextDiff]) => { if (!controller.signal.aborted) { setStatus(nextStatus); setDiff(nextDiff); } })
      : fetchThreadGitCommits(threadId, selected, offset, deviceId, controller.signal).then(value => { if (!controller.signal.aborted) setCommits(value); });
    void operation.catch(caught => { if (!controller.signal.aborted && showFailure.current) setError(caught); }).finally(() => { if (!controller.signal.aborted) { setLoading(false); showFailure.current = false; } });
    return () => controller.abort();
  }, [threadId, deviceId, selected, context, area, path, base, tab, offset, visible]);

  const workingCopy = context?.workingCopies.find(copy => copy.id === selected);
  const latestObservation = context?.observations.filter(item => !item.agentId).sort((a, b) => b.observedAt.localeCompare(a.observedAt))[0];
  const unavailable = error instanceof ApiError && [403, 404, 501].includes(error.statusCode);
  const errorMessage = error instanceof ApiError && error.statusCode === 403 ? translate('workbench.gitAccessUnavailable') : unavailable ? translate('workbench.gitUnavailable') : translate('workbench.gitLoadFailed');
  const selectCopy = (id: string) => { setSelected(id); setPath(''); setOffset(0); };
  return createPortal(<div className="thread-git-backdrop">
    <button type="button" tabIndex={-1} className="ui-overlay-scrim thread-git-scrim" aria-label={translate('workbench.closeDialog')} onClick={onClose} />
    <div className="host-dialog thread-git-dialog" ref={dialog} role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}>
      <header className="thread-git-header">
        <h2 id={titleId}>Git</h2>
        <div className="thread-git-actions">
          {error ? <ErrorMark message={errorMessage} detail={error instanceof Error ? error.message : undefined} kind={unavailable ? 'unavailable' : 'error'} onRetry={refresh} /> : null}
          <button type="button" className="host-icon-button" onClick={refresh} disabled={loading} aria-label={translate('workbench.gitRefresh')}><RefreshCw aria-hidden="true" className={loading ? 'animate-spin' : ''} /></button>
          <button type="button" data-git-close className="host-icon-button" onClick={onClose} aria-label={translate('workbench.closeDialog')}><X aria-hidden="true" /></button>
        </div>
      </header>
      <div className="thread-git-scroll">
        {context && <section className="thread-git-context" aria-label={translate('workbench.gitWorkingCopy')}>
          <label><span>{translate('workbench.gitWorkingCopy')}</span><Select aria-label={translate('workbench.gitWorkingCopy')} value={selected} onValueChange={selectCopy} options={context.workingCopies.map(copy => ({ value: copy.id, label: `${copy.branch ?? translate('workbench.gitDetached')} · ${copy.path}` }))} /></label>
          {workingCopy && <>
            <PathChip path={workingCopy.path} label={translate('workbench.gitCopyAbsolutePath')} />
            <div className="thread-git-meta"><span>{workingCopy.id === context.primaryWorkingCopyId ? translate('workbench.gitPrimary') : translate('workbench.gitViewing')}</span><span>{translate(`workbench.gitSource${workingCopy.source === 'created' ? 'Created' : workingCopy.source === 'adopted' ? 'Adopted' : workingCopy.source === 'observed' ? 'Observed' : 'Configured'}`)}</span><span>{status?.branch ?? workingCopy.branch ?? translate('workbench.gitDetached')}</span><code>{(status?.head ?? workingCopy.head)?.slice(0, 12)}</code></div>
          </>}
          {latestObservation && latestObservation.workingCopyId !== context.primaryWorkingCopyId && <div className="thread-git-observation"><span>{translate('workbench.gitRecentOperation')}</span><PathChip path={latestObservation.cwd} label={translate('workbench.gitCopyAbsolutePath')} /></div>}
          <p className="thread-git-hint">{translate('workbench.gitViewOnly')}</p>
        </section>}
        {!context?.workingCopies.length && !loading && !error && <p className="thread-git-empty">{translate('workbench.gitNoRepository')}</p>}
        {workingCopy && <>
          <div className="thread-git-tabs" role="group" aria-label={translate('workbench.gitScope')}>
            <button type="button" aria-pressed={tab === 'changes'} onClick={() => setTab('changes')}>{translate('workbench.gitChanges')}</button>
            <button type="button" aria-pressed={tab === 'commits'} onClick={() => setTab('commits')}>{translate('workbench.gitCommits')}</button>
          </div>
          {tab === 'changes' ? <section className="thread-git-changes" aria-label={translate('workbench.gitChanges')}>
            <div className="thread-git-diff-controls"><Select aria-label={translate('workbench.gitDiffScope')} value={area} onValueChange={value => { setArea(value as GitDiffArea); setPath(''); }} options={[
              { value: 'worktree', label: translate('workbench.gitUnstaged') }, { value: 'index', label: translate('workbench.gitStaged') }, { value: 'branch', label: translate('workbench.gitCompare') },
            ]} />{area === 'branch' && <form onSubmit={event => { event.preventDefault(); setBase(baseDraft.trim() || 'HEAD'); }}><input aria-label={translate('workbench.gitBase')} value={baseDraft} onChange={event => setBaseDraft(event.target.value)} /><button type="submit">{translate('workbench.gitCompare')}</button></form>}</div>
            {status && <>
              <div className="thread-git-files" aria-label={translate('workbench.gitChangedFiles')}>
                <button type="button" aria-pressed={!path} onClick={() => setPath('')}>{translate('workbench.gitAllFiles')} <span>{status.files.length.toLocaleString(locale)}</span></button>
                {status.files.map(file => <div className="thread-git-file" key={file.path}><button type="button" aria-label={translate('workbench.gitViewFile', { path: file.path })} aria-pressed={path === file.path} onClick={() => setPath(file.path)}><code aria-label={translate('workbench.gitFileStatus')}>{file.indexStatus}{file.worktreeStatus}</code><MiddleName name={file.path.split('/').pop() ?? file.path} /></button><PathChip path={file.path} label={translate('common.copyPath')} /></div>)}
              </div>
              {status.truncated && <p role="status" className="thread-git-hint">{translate('workbench.gitTruncated')}</p>}
            </>}
            {diff && <>
              <div className="thread-git-patch-heading"><span>{path ? <PathChip path={path} /> : translate('workbench.gitAllFiles')}</span><CopyButton value={diff.patch} label={translate('workbench.gitCopyPatch')} /></div>
              {diff.patch ? <pre className="thread-git-patch" tabIndex={0}>{diff.patch}</pre> : <p className="thread-git-empty">{translate('workbench.gitNoDiff')}</p>}
              {diff.truncated && <p role="status" className="thread-git-hint">{translate('workbench.gitTruncated')}</p>}
            </>}
          </section> : <section className="thread-git-history" aria-label={translate('workbench.gitCommits')}>
            {commits?.commits.map(commit => <article key={commit.oid}><strong>{commit.subject}</strong><div className="thread-git-meta"><code>{commit.oid.slice(0, 12)}</code><span>{commit.author}</span><time dateTime={commit.authoredAt}>{Number.isNaN(Date.parse(commit.authoredAt)) ? commit.authoredAt : new Date(commit.authoredAt).toLocaleString(locale)}</time><CopyButton value={commit.oid} label={translate('workbench.gitCopyCommit')} /></div></article>)}
            {commits && !commits.commits.length && <p className="thread-git-empty">{translate('workbench.gitNoCommits')}</p>}
            <div className="thread-git-pagination"><button type="button" disabled={loading || offset === 0} onClick={() => setOffset(value => Math.max(0, value - 30))}>{translate('workbench.gitPrevious')}</button><button type="button" disabled={loading || !commits?.hasMore} onClick={() => setOffset(value => value + 30)}>{translate('workbench.gitNext')}</button></div>
          </section>}
        </>}
        {loading && <p role="status" className="thread-git-hint">{translate('workbench.gitLoading')}</p>}
      </div>
    </div>
  </div>, document.body);
}
