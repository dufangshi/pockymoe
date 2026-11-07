import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ChevronLeft, ChevronRight, FileDiff, GitBranch, GitCommitHorizontal, MapPin, RefreshCw, X } from 'lucide-react';
import { CopyButton, ErrorMark, MiddleName, PathChip, Select, formatShortTimestamp, useModalFocus } from '@pockymoe/thread-ui';
import { formatNumber, translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ApiError } from '../lib/api';
import {
  fetchThreadGitCommits, fetchThreadGitDiff, fetchThreadGitStatus, fetchThreadWorkingContext,
  type GitCommits, type GitDiff, type GitDiffArea, type GitStatus, type ThreadWorkingContext, type WorkingCopy,
} from '../lib/gitApi';
import { gitText as gt } from '../lib/gitI18n';
import { fileChange, parsePatch, type FileChange, type PatchFile, type PatchLine } from './git/patch';
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

const SOURCE_KEY = { created: 'workbench.gitSourceCreated', adopted: 'workbench.gitSourceAdopted', observed: 'workbench.gitSourceObserved', configured: 'workbench.gitSourceConfigured' } as const;
const CHANGE_LETTER: Record<FileChange, string> = { untracked: 'U', added: 'A', modified: 'M', deleted: 'D', renamed: 'R', copied: 'C', conflicted: '!', typeChanged: 'T', changed: '•' };
const copyLabel = (copy: WorkingCopy) => `${copy.branch ?? translate('workbench.gitDetached')} · ${copy.path}`;
const baseName = (path: string) => path.split('/').pop() || path;
const dirName = (path: string) => path.includes('/') ? path.slice(0, path.lastIndexOf('/') + 1) : '';

interface ListedFile { path: string; change: FileChange; originalPath?: string | undefined }

function ThreadGitDialog({ threadId, deviceId, refreshKey, onClose }: ThreadGitControlProps & { onClose(): void }) {
  useI18n();
  const dialog = useRef<HTMLDivElement>(null);
  const diffPane = useRef<HTMLElement>(null);
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
  // Per-file counts and the comparison's file list come from the last whole-scope patch.
  const [scopeFiles, setScopeFiles] = useState<{ scope: string; files: PatchFile[] } | null>(null);
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

  const areaScope = JSON.stringify([selected, area, area === 'branch' ? base : '']);
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
      ]).then(([nextStatus, nextDiff]) => {
        if (controller.signal.aborted) return;
        setStatus(nextStatus); setDiff(nextDiff);
        if (!path) setScopeFiles({ scope: areaScope, files: parsePatch(nextDiff.patch).files });
      })
      : fetchThreadGitCommits(threadId, selected, offset, deviceId, controller.signal).then(value => { if (!controller.signal.aborted) setCommits(value); });
    void operation.catch(caught => { if (!controller.signal.aborted && showFailure.current) setError(caught); }).finally(() => { if (!controller.signal.aborted) { setLoading(false); showFailure.current = false; } });
    return () => controller.abort();
    // areaScope is derived from selected, area and base.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadId, deviceId, selected, context, area, path, base, tab, offset, visible]);

  const workingCopy = context?.workingCopies.find(copy => copy.id === selected);
  const latestObservation = context?.observations.filter(item => !item.agentId).sort((a, b) => b.observedAt.localeCompare(a.observedAt))[0];
  const observedOutside = latestObservation && latestObservation.workingCopyId !== context?.primaryWorkingCopyId ? latestObservation : undefined;
  const unavailable = error instanceof ApiError && [403, 404, 501].includes(error.statusCode);
  const errorMessage = error instanceof ApiError && error.statusCode === 403 ? translate('workbench.gitAccessUnavailable') : unavailable ? translate('workbench.gitUnavailable') : translate('workbench.gitLoadFailed');
  const selectCopy = (id: string) => { setSelected(id); setPath(''); setOffset(0); };
  const selectArea = (next: GitDiffArea) => { setArea(next); setPath(''); };
  const known = scopeFiles?.scope === areaScope ? scopeFiles.files : null;
  const stats = useMemo(() => new Map((known ?? []).map(file => [file.path, file])), [known]);
  const unstaged = status?.files.filter(file => file.worktreeStatus !== ' ') ?? [];
  const staged = status?.files.filter(file => file.indexStatus !== ' ' && file.indexStatus !== '?') ?? [];
  const listed: ListedFile[] = area === 'worktree' ? unstaged.map(file => ({ path: file.path, originalPath: file.originalPath, change: fileChange(file.worktreeStatus === '?' ? '?' : file.worktreeStatus, file.indexStatus) }))
    : area === 'index' ? staged.map(file => ({ path: file.path, originalPath: file.originalPath, change: fileChange(file.indexStatus, file.worktreeStatus) }))
    : (known ?? []).map(file => ({ path: file.path, originalPath: file.oldPath, change: file.created ? 'added' : file.removed ? 'deleted' : file.oldPath ? 'renamed' : 'modified' }));
  const choose = (next: string) => {
    setPath(next);
    // On a phone the list and diff stack: bring the diff to the reader.
    if (window.matchMedia?.('(max-width: 639px)').matches) requestAnimationFrame(() => diffPane.current?.scrollIntoView({ block: 'start', behavior: 'smooth' }));
  };
  const parsed = useMemo(() => diff ? parsePatch(diff.patch) : null, [diff]);
  const observedCopy = observedOutside && context?.workingCopies.find(copy => copy.id === observedOutside.workingCopyId);

  return createPortal(<div className="thread-git-backdrop">
    <button type="button" tabIndex={-1} className="ui-overlay-scrim thread-git-scrim" aria-label={translate('workbench.closeDialog')} onClick={onClose} />
    <div className="host-dialog thread-git-dialog" ref={dialog} role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}>
      <header className="thread-git-header">
        <h2 id={titleId}><GitBranch aria-hidden="true" />Git</h2>
        {context && context.workingCopies.length > 0 && <div className="thread-git-copy">
          <Select aria-label={translate('workbench.gitWorkingCopy')} value={selected} onValueChange={selectCopy}
            options={context.workingCopies.map(copy => ({ value: copy.id, label: copyLabel(copy) }))} />
        </div>}
        <div className="thread-git-actions">
          {error ? <ErrorMark message={errorMessage} detail={error instanceof Error ? error.message : undefined} kind={unavailable ? 'unavailable' : 'error'} onRetry={refresh} /> : null}
          <button type="button" className="thread-git-icon" onClick={refresh} disabled={loading} aria-label={translate('workbench.gitRefresh')} title={translate('workbench.gitRefresh')}><RefreshCw aria-hidden="true" className={loading ? 'animate-spin' : ''} /></button>
          <button type="button" data-git-close className="thread-git-icon" onClick={onClose} aria-label={translate('workbench.closeDialog')}><X aria-hidden="true" /></button>
        </div>
      </header>
      {workingCopy && <section className="thread-git-context" aria-label={translate('workbench.gitWorkingCopy')}>
        <div className="thread-git-facts">
          <span className="thread-git-branch"><GitBranch aria-hidden="true" />{status?.branch ?? workingCopy.branch ?? translate('workbench.gitDetached')}</span>
          {(status?.head ?? workingCopy.head) && <span className="thread-git-head"><code>{(status?.head ?? workingCopy.head)!.slice(0, 10)}</code><CopyButton value={(status?.head ?? workingCopy.head)!} label={gt('copyHead')} /></span>}
          <span className="thread-git-tag" data-primary={workingCopy.id === context?.primaryWorkingCopyId || undefined}>{workingCopy.id === context?.primaryWorkingCopyId ? translate('workbench.gitPrimary') : translate('workbench.gitViewing')}</span>
          <span className="thread-git-tag">{translate(SOURCE_KEY[workingCopy.source] ?? 'workbench.gitSourceConfigured')}</span>
          <PathChip path={workingCopy.path} label={translate('workbench.gitCopyAbsolutePath')} />
        </div>
        <p className="thread-git-hint"><span>{translate('workbench.gitViewOnly')}</span> <span>{gt('readOnlyHint')}</span></p>
      </section>}
      {observedOutside && <div className="thread-git-observation" role="note">
        <MapPin aria-hidden="true" />
        <span className="thread-git-observation-text"><span>{translate('workbench.gitRecentOperation')}</span>
          <span className="thread-git-observation-where"><PathChip path={observedOutside.cwd} label={translate('workbench.gitCopyAbsolutePath')} /><time dateTime={observedOutside.observedAt}>{formatShortTimestamp(observedOutside.observedAt)}</time></span></span>
        {observedCopy && observedCopy.id !== selected && <button type="button" className="thread-git-button" onClick={() => selectCopy(observedCopy.id)}>{gt('viewObserved')}</button>}
      </div>}
      {!context?.workingCopies.length && !loading && !error && <p className="thread-git-empty is-page">{translate('workbench.gitNoRepository')}</p>}
      {workingCopy && <>
        <div className="thread-git-tabs" role="group" aria-label={translate('workbench.gitScope')}>
          <button type="button" aria-pressed={tab === 'changes'} onClick={() => setTab('changes')}><FileDiff aria-hidden="true" />{translate('workbench.gitChanges')}{status && <span className="thread-git-count">{formatNumber(status.files.length)}</span>}</button>
          <button type="button" aria-pressed={tab === 'commits'} onClick={() => setTab('commits')}><GitCommitHorizontal aria-hidden="true" />{translate('workbench.gitCommits')}</button>
          {loading && <span role="status" className="thread-git-loading">{translate('workbench.gitLoading')}</span>}
        </div>
        {tab === 'changes' ? <div className="thread-git-changes">
          <aside className="thread-git-files" aria-label={translate('workbench.gitChangedFiles')}>
            <div className="thread-git-segments" role="group" aria-label={translate('workbench.gitDiffScope')}>
              <button type="button" aria-pressed={area === 'worktree'} onClick={() => selectArea('worktree')}>{translate('workbench.gitUnstaged')}{status && <span>{formatNumber(unstaged.length)}</span>}</button>
              <button type="button" aria-pressed={area === 'index'} onClick={() => selectArea('index')}>{translate('workbench.gitStaged')}{status && <span>{formatNumber(staged.length)}</span>}</button>
              <button type="button" aria-pressed={area === 'branch'} onClick={() => selectArea('branch')}>{translate('workbench.gitCompare')}</button>
            </div>
            {area === 'branch' && <form className="thread-git-base" onSubmit={event => { event.preventDefault(); setBase(baseDraft.trim() || 'HEAD'); setPath(''); }}>
              <label><span>{gt('base')}</span><input aria-label={translate('workbench.gitBase')} value={baseDraft} spellCheck={false} autoCapitalize="off" onChange={event => setBaseDraft(event.target.value)} /></label>
              <button type="submit" className="thread-git-button">{gt('apply')}</button>
            </form>}
            {status && <ul className="thread-git-file-list">
              <li><button type="button" className="thread-git-file is-all" aria-pressed={!path} onClick={() => choose('')}>
                <span className="thread-git-file-name">{translate('workbench.gitAllFiles')}</span><span className="thread-git-file-meta">{formatNumber(listed.length)}</span>
              </button></li>
              {listed.map(file => { const stat = stats.get(file.path); return <li key={file.path}>
                <button type="button" className="thread-git-file" aria-label={translate('workbench.gitViewFile', { path: file.path })} aria-pressed={path === file.path} onClick={() => choose(file.path)}>
                  <span className="thread-git-letter" data-change={file.change} title={gt(file.change)} aria-hidden="true">{CHANGE_LETTER[file.change]}</span>
                  <span className="thread-git-file-text"><MiddleName name={baseName(file.path)} className="thread-git-file-name" />
                    {(dirName(file.path) || file.originalPath) && <span className="thread-git-file-dir">{file.originalPath ? `${file.originalPath} → ` : ''}{dirName(file.path)}</span>}</span>
                  {stat && !stat.binary && <span className="thread-git-stat"><ins>+{formatNumber(stat.additions)}</ins><del>−{formatNumber(stat.deletions)}</del></span>}
                </button>
              </li>; })}
            </ul>}
            {status && !listed.length && <p className="thread-git-empty">{gt('noFiles')}</p>}
            {status?.truncated && <p role="status" className="thread-git-hint">{translate('workbench.gitTruncated')}</p>}
          </aside>
          <section className="thread-git-diff" ref={diffPane} aria-label={path || translate('workbench.gitAllFiles')}>
            {diff && <>
              <div className="thread-git-diff-head">
                {path && <button type="button" className="thread-git-icon thread-git-back" aria-label={gt('backToFiles')} onClick={() => setPath('')}><ChevronLeft aria-hidden="true" /></button>}
                <span className="thread-git-diff-title">{path ? <PathChip path={path} /> : translate('workbench.gitAllFiles')}</span>
                {diff.patch && <CopyButton value={diff.patch} label={translate('workbench.gitCopyPatch')} />}
              </div>
              {diff.truncated && <p role="status" className="thread-git-hint thread-git-truncated">{translate('workbench.gitTruncated')}</p>}
              {diff.patch && parsed ? <PatchView parsed={parsed} single={!!path} onSelect={choose} /> : <p className="thread-git-empty">{translate('workbench.gitNoDiff')}</p>}
            </>}
          </section>
        </div> : <section className="thread-git-history" aria-label={translate('workbench.gitCommits')}>
          {commits && !!commits.commits.length && <ol className="thread-git-commits">
            {commits.commits.map(commit => <li key={commit.oid}>
              <GitCommitHorizontal aria-hidden="true" className="thread-git-commit-icon" />
              <div className="thread-git-commit-main"><strong>{commit.subject}</strong>
                <span className="thread-git-commit-meta"><span>{commit.author}</span><time dateTime={commit.authoredAt}>{Number.isNaN(Date.parse(commit.authoredAt)) ? commit.authoredAt : formatShortTimestamp(commit.authoredAt)}</time></span></div>
              <span className="thread-git-head"><code>{commit.oid.slice(0, 10)}</code><CopyButton value={commit.oid} label={translate('workbench.gitCopyCommit')} /></span>
            </li>)}
          </ol>}
          {commits && !commits.commits.length && <p className="thread-git-empty">{translate('workbench.gitNoCommits')}</p>}
          <div className="thread-git-pagination">
            <button type="button" className="thread-git-button" disabled={loading || offset === 0} onClick={() => setOffset(value => Math.max(0, value - 30))}><ChevronLeft aria-hidden="true" />{translate('workbench.gitPrevious')}</button>
            <span className="thread-git-dim">{gt('page', { n: formatNumber(offset / 30 + 1) })}</span>
            <button type="button" className="thread-git-button" disabled={loading || !commits?.hasMore} onClick={() => setOffset(value => value + 30)}>{translate('workbench.gitNext')}<ChevronRight aria-hidden="true" /></button>
          </div>
        </section>}
      </>}
      {!workingCopy && loading && <p role="status" className="thread-git-empty is-page">{translate('workbench.gitLoading')}</p>}
    </div>
  </div>, document.body);
}

/** The diff, file by file, with additions and deletions colored. Text is unchanged. */
function PatchView({ parsed, single, onSelect }: { parsed: ReturnType<typeof parsePatch>; single: boolean; onSelect(path: string): void }) {
  return <div className="thread-git-patch" tabIndex={0}>
    {parsed.preamble.length > 0 && <Lines lines={parsed.preamble} />}
    {parsed.files.map((file, index) => <section key={`${file.path}:${index}`} className="thread-git-patch-file">
      {!single && <button type="button" className="thread-git-patch-file-head" aria-label={translate('workbench.gitViewFile', { path: file.path })} onClick={() => onSelect(file.path)}>
        <MiddleName name={file.path} />
        {!file.binary ? <span className="thread-git-stat"><ins>+{formatNumber(file.additions)}</ins><del>−{formatNumber(file.deletions)}</del></span> : <span className="thread-git-dim">{gt('binary')}</span>}
      </button>}
      <Lines lines={file.lines} />
    </section>)}
  </div>;
}

function Lines({ lines }: { lines: PatchLine[] }) {
  return <pre>{lines.map((line, index) => <span key={index} className="thread-git-line" data-kind={line.kind}>{line.text || ' '}{'\n'}</span>)}</pre>;
}
