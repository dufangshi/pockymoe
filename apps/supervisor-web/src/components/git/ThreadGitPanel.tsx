import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Check, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, FileDiff, FileQuestion, GitBranch, Info, MapPin, RefreshCw } from 'lucide-react';
import { CopyButton, ErrorMark, MiddleName, PathChip, Select, formatShortTimestamp } from '@pockymoe/thread-ui';
import { formatNumber, getLocale, translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ApiError } from '../../lib/api';
import {
  fetchThreadGitCommits, fetchThreadGitDiff, fetchThreadGitStatus, fetchThreadWorkingContext,
  type GitCommit, type GitCommits, type GitDiff, type GitDiffArea, type GitStatus, type ThreadWorkingContext, type WorkingCopy,
} from '../../lib/gitApi';
import { gitText as gt } from '../../lib/gitI18n';
import { fileChange, parsePatch, type FileChange, type PatchFile, type PatchLine } from './patch';
import './thread-git.css';

export interface ThreadGitPanelProps {
  threadId: string;
  deviceId?: string | null | undefined;
  /** Existing thread updates invalidate the open panel; no polling is started. */
  refreshKey?: string | undefined;
}

type View = 'worktree' | 'index' | 'branch' | 'commits';
const SOURCE_KEY = { created: 'workbench.gitSourceCreated', adopted: 'workbench.gitSourceAdopted', observed: 'workbench.gitSourceObserved', configured: 'workbench.gitSourceConfigured' } as const;
const CHANGE_LETTER: Record<FileChange, string> = { untracked: 'U', added: 'A', modified: 'M', deleted: 'D', renamed: 'R', copied: 'C', conflicted: '!', typeChanged: 'T', changed: '•' };
const PAGE = 30;
const baseName = (path: string) => path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;
const dirName = (path: string) => path.includes('/') ? path.slice(0, path.lastIndexOf('/') + 1) : '';
const shortOid = (oid: string) => oid.slice(0, 7);

/** A time in as few words as it needs: the hour today, the day this year, else the date.
    The full timestamp stays in the title and, for operations, in the details. */
function compactTime(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const now = new Date();
  const options: Intl.DateTimeFormatOptions = date.toDateString() === now.toDateString() ? { hour: 'numeric', minute: '2-digit' }
    : date.getFullYear() === now.getFullYear() ? { month: 'short', day: 'numeric' } : { year: 'numeric', month: 'short', day: 'numeric' };
  return date.toLocaleString(getLocale(), options);
}
function Time({ value }: { value: string }) {
  return <time dateTime={value} title={Number.isNaN(Date.parse(value)) ? undefined : formatShortTimestamp(value)}>{compactTime(value)}</time>;
}

interface ListedFile { path: string; change: FileChange; originalPath?: string | undefined }

/** The thread's Git inspector, docked beside the conversation (read only).
    One line says where you are (working copy, HEAD, details, refresh), one row
    picks what to look at, and the rest is the file list and the diff. Narrow,
    the list and a file's diff take turns; wide (a dragged-out dock), they sit
    side by side (docs/worktrees.md). */
export function ThreadGitPanel({ threadId, deviceId, refreshKey }: ThreadGitPanelProps) {
  useI18n();
  const detailsId = useId();
  const listRef = useRef<HTMLElement>(null);
  const diffRef = useRef<HTMLElement>(null);
  const [context, setContext] = useState<ThreadWorkingContext | null>(null);
  const [selected, setSelected] = useState('');
  const [view, setView] = useState<View>('worktree');
  // A commit opened from the list; its changes show in place of the list.
  const [commit, setCommit] = useState<GitCommit | null>(null);
  const [path, setPath] = useState('');
  // Narrow only: whether the diff (of `path`, or of all files) replaces the list.
  const [opened, setOpened] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
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
  const returnFocus = useRef<string | null>(null);
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

  const area: GitDiffArea | null = view === 'commits' ? commit ? 'commit' : null : view;
  const diffBase = area === 'commit' ? commit?.oid ?? '' : base;
  const areaScope = JSON.stringify([selected, area, area === 'branch' || area === 'commit' ? diffBase : '']);
  useEffect(() => {
    if (!visible || !context) return;
    if (!selected) { setLoading(false); return; }
    const controller = new AbortController();
    setLoading(true);
    setError(null);
    const scope = JSON.stringify([selected, area, path, diffBase, offset]);
    if (scope !== loadedScope.current) { setDiff(null); setCommits(null); showFailure.current = true; }
    loadedScope.current = scope;
    const operation = area
      ? Promise.all([
        fetchThreadGitStatus(threadId, selected, deviceId, controller.signal),
        fetchThreadGitDiff(threadId, selected, { area, ...(path ? { path } : {}), ...(area === 'branch' || area === 'commit' ? { base: diffBase } : {}) }, deviceId, controller.signal),
      ]).then(([nextStatus, nextDiff]) => {
        if (controller.signal.aborted) return;
        setStatus(nextStatus); setDiff(nextDiff);
        if (!path) setScopeFiles({ scope: areaScope, files: parsePatch(nextDiff.patch).files });
      })
      : fetchThreadGitCommits(threadId, selected, offset, deviceId, controller.signal).then(value => { if (!controller.signal.aborted) setCommits(value); });
    void operation.catch(caught => { if (!controller.signal.aborted && showFailure.current) setError(caught); }).finally(() => { if (!controller.signal.aborted) { setLoading(false); showFailure.current = false; } });
    return () => controller.abort();
    // areaScope is derived from selected, area and diffBase.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadId, deviceId, selected, context, area, path, diffBase, offset, visible]);

  const workingCopy = context?.workingCopies.find(copy => copy.id === selected);
  const manyCopies = (context?.workingCopies.length ?? 0) > 1;
  const latestObservation = context?.observations.filter(item => !item.agentId).sort((a, b) => b.observedAt.localeCompare(a.observedAt))[0];
  const observedOutside = latestObservation && latestObservation.workingCopyId !== context?.primaryWorkingCopyId ? latestObservation : undefined;
  const observedCopy = observedOutside && context?.workingCopies.find(copy => copy.id === observedOutside.workingCopyId);
  const unavailable = error instanceof ApiError && [403, 404, 501].includes(error.statusCode);
  const errorMessage = error instanceof ApiError && error.statusCode === 403 ? translate('workbench.gitAccessUnavailable') : unavailable ? translate('workbench.gitUnavailable') : translate('workbench.gitLoadFailed');

  const reset = () => { setPath(''); setOpened(false); };
  const selectCopy = (id: string) => { setSelected(id); setStatus(null); setCommit(null); setOffset(0); reset(); };
  const selectView = (next: View) => { setView(next); setCommit(null); reset(); };
  const openCommit = (next: GitCommit) => { setCommit(next); reset(); };

  const known = scopeFiles?.scope === areaScope ? scopeFiles.files : null;
  const stats = useMemo(() => new Map((known ?? []).map(file => [file.path, file])), [known]);
  const unstaged = status?.files.filter(file => file.worktreeStatus !== ' ') ?? [];
  const staged = status?.files.filter(file => file.indexStatus !== ' ' && file.indexStatus !== '?') ?? [];
  const listed: ListedFile[] = area === 'worktree' ? unstaged.map(file => ({ path: file.path, originalPath: file.originalPath, change: fileChange(file.worktreeStatus === '?' ? '?' : file.worktreeStatus, file.indexStatus) }))
    : area === 'index' ? staged.map(file => ({ path: file.path, originalPath: file.originalPath, change: fileChange(file.indexStatus, file.worktreeStatus) }))
    : (known ?? []).map(file => ({ path: file.path, originalPath: file.oldPath, change: file.created ? 'added' : file.removed ? 'deleted' : file.oldPath ? 'renamed' : 'modified' }));
  const listReady = area === 'worktree' || area === 'index' ? !!status : !!known;
  const totals = useMemo(() => (known ?? []).reduce((sum, file) => ({ additions: sum.additions + file.additions, deletions: sum.deletions + file.deletions }), { additions: 0, deletions: 0 }), [known]);
  const parsed = useMemo(() => diff ? parsePatch(diff.patch) : null, [diff]);
  const current = path ? listed.findIndex(file => file.path === path) : -1;

  const choose = (next: string) => {
    if (next === '') returnFocus.current = null;
    setPath(next); setOpened(true);
  };
  const back = () => { returnFocus.current = path || '\u0000all'; reset(); };
  // The list and the diff take turns when narrow: the reader lands on the diff's
  // back button, and Back returns to the row they opened.
  useLayoutEffect(() => {
    const narrow = (element: HTMLElement | null) => !!element && element.offsetParent === null;
    if (opened) {
      diffRef.current?.scrollTo?.({ top: 0 });
      if (narrow(listRef.current) && listRef.current?.contains(document.activeElement)) diffRef.current?.querySelector<HTMLElement>('.thread-git-back')?.focus();
    } else if (returnFocus.current && !narrow(listRef.current) && diffRef.current?.offsetParent === null) {
      const target = returnFocus.current === '\u0000all'
        ? listRef.current?.querySelector<HTMLElement>('.thread-git-all')
        : Array.from(listRef.current?.querySelectorAll<HTMLElement>('[data-path]') ?? []).find(element => element.dataset.path === returnFocus.current);
      target?.focus();
      returnFocus.current = null;
    }
  }, [opened, path]);

  const counts = { worktree: status ? unstaged.length : null, index: status ? staged.length : null };
  const scopeEmpty = area === 'worktree' ? (status?.files.length ? gt('noUnstaged') : gt('clean'))
    : area === 'index' ? gt('noStaged') : area === 'branch' ? gt('noDifference', { base }) : gt('noFiles');

  return <div className="thread-git-panel" data-testid="thread-git" data-opened={opened || undefined}>
    {/* Where: the working copy (a switch when there are several), HEAD, details and refresh, on one line. */}
    {(workingCopy || !!error) && <div className="thread-git-where">
      {workingCopy && (manyCopies && context
        ? <div className="thread-git-copy"><Select aria-label={translate('workbench.gitWorkingCopy')} value={selected} onValueChange={selectCopy}
          options={context.workingCopies.map(copy => ({ value: copy.id, label: <CopyOption copy={copy} primary={copy.id === context.primaryWorkingCopyId} /> }))} /></div>
        : <span className="thread-git-branch" title={workingCopy.branch ?? undefined}><GitBranch aria-hidden="true" /><span>{status?.branch ?? workingCopy.branch ?? translate('workbench.gitDetached')}</span></span>)}
      {workingCopy && (status?.head ?? workingCopy.head) && <span className="thread-git-head" title={(status?.head ?? workingCopy.head)!}>
        <code>{shortOid((status?.head ?? workingCopy.head)!)}</code><CopyButton value={(status?.head ?? workingCopy.head)!} label={gt('copyHead')} />
      </span>}
      <span className="thread-git-where-end">
        {error ? <ErrorMark message={errorMessage} detail={error instanceof Error ? error.message : undefined} kind={unavailable ? 'unavailable' : 'error'} onRetry={refresh} /> : null}
        {workingCopy && <button type="button" className="thread-git-icon" aria-expanded={detailsOpen} aria-controls={detailsId} onClick={() => setDetailsOpen(value => !value)}
          aria-label={gt('details')} title={gt('details')}><Info aria-hidden="true" /></button>}
        <button type="button" className="thread-git-icon" onClick={refresh} disabled={loading} aria-label={translate('workbench.gitRefresh')} title={translate('workbench.gitRefresh')}><RefreshCw aria-hidden="true" className={loading ? 'request-spin' : ''} /></button>
      </span>
    </div>}
    {/* A short request: the refresh icon turns; readers hear the words. */}
    <span role="status" className="thread-git-sr">{loading && workingCopy ? translate('workbench.gitLoading') : ''}</span>
    {workingCopy && detailsOpen && <dl className="thread-git-details" id={detailsId} aria-label={gt('details')}>
      <dt>{gt('path')}</dt><dd><PathChip path={workingCopy.path} label={translate('workbench.gitCopyAbsolutePath')} /></dd>
      {(manyCopies || workingCopy.source !== 'configured') && <><dt>{gt('kind')}</dt><dd className="thread-git-tags">
        {manyCopies && <span className="thread-git-tag" data-primary={workingCopy.id === context?.primaryWorkingCopyId || undefined}>{workingCopy.id === context?.primaryWorkingCopyId ? translate('workbench.gitPrimary') : translate('workbench.gitViewing')}</span>}
        {workingCopy.source !== 'configured' && <span className="thread-git-tag">{translate(SOURCE_KEY[workingCopy.source] ?? 'workbench.gitSourceConfigured')}</span>}
      </dd></>}
      {observedOutside && <><dt>{gt('recent')}</dt><dd className="thread-git-recent-where"><PathChip path={observedOutside.cwd} label={translate('workbench.gitCopyAbsolutePath')} /><time dateTime={observedOutside.observedAt}>{formatShortTimestamp(observedOutside.observedAt)}</time></dd></>}
      {manyCopies && <dd className="thread-git-hint">{translate('workbench.gitViewOnly')}</dd>}
    </dl>}
    {/* The agent's latest operation was in another working copy: say where, once, with a way there. */}
    {workingCopy && observedOutside && <div className="thread-git-recent" role="note">
      <MapPin aria-hidden="true" />
      <span className="thread-git-recent-text">{observedCopy ? observedCopy.id === selected ? gt('recentHere') : gt('recentIn', { name: copyName(observedCopy) }) : gt('recentElsewhere')}</span>
      <Time value={observedOutside.observedAt} />
      {observedCopy && observedCopy.id !== selected && <button type="button" className="thread-git-link" onClick={() => selectCopy(observedCopy.id)} aria-label={gt('viewObservedLabel')}>{gt('viewObserved')}</button>}
    </div>}
    {!context?.workingCopies.length && !loading && !error && <p className="thread-git-empty is-page">{translate('workbench.gitNoRepository')}</p>}
    {!workingCopy && loading && <p className="thread-git-empty is-page" aria-hidden="true">{translate('workbench.gitLoading')}</p>}
    {workingCopy && <>
      <div className="thread-git-views" role="group" aria-label={translate('workbench.gitScope')}>
        <button type="button" aria-pressed={view === 'worktree'} onClick={() => selectView('worktree')}>{translate('workbench.gitUnstaged')}<Count value={counts.worktree} /></button>
        <button type="button" aria-pressed={view === 'index'} onClick={() => selectView('index')}>{translate('workbench.gitStaged')}<Count value={counts.index} /></button>
        <button type="button" aria-pressed={view === 'branch'} onClick={() => selectView('branch')}>{translate('workbench.gitCompare')}</button>
        <button type="button" aria-pressed={view === 'commits'} onClick={() => selectView('commits')}>{translate('workbench.gitCommits')}</button>
      </div>
      {view === 'branch' && <form className="thread-git-bar thread-git-base" onSubmit={event => { event.preventDefault(); setBase(baseDraft.trim() || 'HEAD'); reset(); }}>
        <label><span>{gt('base')}</span><input aria-label={translate('workbench.gitBase')} value={baseDraft} spellCheck={false} autoCapitalize="off" autoCorrect="off" onChange={event => setBaseDraft(event.target.value)} /></label>
        <button type="submit" className="thread-git-button">{gt('apply')}</button>
      </form>}
      {commit && <div className="thread-git-bar thread-git-commit-bar">
        <button type="button" className="thread-git-icon" onClick={() => { setCommit(null); reset(); }} aria-label={gt('backToCommits')} title={gt('backToCommits')}><ChevronLeft aria-hidden="true" /></button>
        <span className="thread-git-head"><code>{shortOid(commit.oid)}</code><CopyButton value={commit.oid} label={translate('workbench.gitCopyCommit')} /></span>
        <span className="thread-git-commit-bar-subject" title={commit.subject}>{commit.subject}</span>
      </div>}
      {area ? <div className="thread-git-changes" data-empty={listReady && !listed.length || undefined}>
        <aside ref={listRef} className="thread-git-files" aria-label={translate('workbench.gitChangedFiles')}>
          {listReady && listed.length > 0 && <div className="thread-git-summary">
            <span className="thread-git-summary-text">{gt(listed.length === 1 ? 'oneFile' : 'files', { n: formatNumber(listed.length) })}{known && <Stat file={totals} />}</span>
            <button type="button" className="thread-git-all" aria-pressed={!path} onClick={() => choose('')}><FileDiff aria-hidden="true" />{gt('allDiffs')}</button>
          </div>}
          {listReady && listed.length > 0 && <ul className="thread-git-file-list">
            {listed.map(file => { const stat = stats.get(file.path); return <li key={file.path}>
              <button type="button" className="thread-git-file" data-path={file.path} title={file.originalPath ? `${file.originalPath} → ${file.path}` : file.path}
                aria-label={translate('workbench.gitViewFile', { path: file.path })} aria-pressed={path === file.path} onClick={() => choose(file.path)}>
                <span className="thread-git-letter" data-change={file.change} aria-hidden="true">{CHANGE_LETTER[file.change]}</span>
                <MiddleName name={baseName(file.path)} className="thread-git-file-name" />
                <span className="thread-git-file-dir"><bdi>{file.originalPath ? `← ${baseName(file.originalPath)} ` : ''}{dirName(file.path)}</bdi></span>
                {stat?.binary ? <span className="thread-git-dim thread-git-file-kind">{gt('binary')}</span> : stat && <Stat file={stat} />}
                <span className="thread-git-sr">{gt(file.change)}</span>
              </button>
            </li>; })}
          </ul>}
          {listReady && !listed.length && <p className="thread-git-empty">{area === 'worktree' && !status?.files.length ? <Check aria-hidden="true" /> : null}{scopeEmpty}</p>}
          {status?.truncated && <p role="status" className="thread-git-hint thread-git-note">{translate('workbench.gitTruncated')}</p>}
        </aside>
        <section ref={diffRef} className="thread-git-diff" aria-label={path || translate('workbench.gitAllFiles')}>
          <div className="thread-git-diff-head">
            <button type="button" className="thread-git-icon thread-git-back" aria-label={gt('backToFiles')} title={gt('backToFiles')} onClick={back}><ChevronLeft aria-hidden="true" /></button>
            <span className="thread-git-diff-title">{path ? <PathChip path={path} /> : <span>{translate('workbench.gitAllFiles')}</span>}</span>
            {path && current >= 0 && <>
              <button type="button" className="thread-git-icon" disabled={current <= 0} onClick={() => choose(listed[current - 1]!.path)} aria-label={gt('previousFile')} title={gt('previousFile')}><ChevronUp aria-hidden="true" /></button>
              <button type="button" className="thread-git-icon" disabled={current >= listed.length - 1} onClick={() => choose(listed[current + 1]!.path)} aria-label={gt('nextFile')} title={gt('nextFile')}><ChevronDown aria-hidden="true" /></button>
            </>}
            {diff?.patch && <CopyButton value={diff.patch} label={translate('workbench.gitCopyPatch')} />}
          </div>
          {diff?.truncated && <p role="status" className="thread-git-hint thread-git-note">{translate('workbench.gitTruncated')}</p>}
          {diff && (parsed && (parsed.files.length || parsed.preamble.length)
            ? <PatchView parsed={parsed} single={!!path} onSelect={choose} />
            : <p className="thread-git-empty">{path && ['untracked', 'added'].includes(listed[current]?.change ?? '') ? gt('emptyFile') : gt('noDiff')}</p>)}
        </section>
      </div> : <section className="thread-git-history" aria-label={translate('workbench.gitCommits')}>
        {commits && !!commits.commits.length && <ol className="thread-git-commits">
          {commits.commits.map(item => <li key={item.oid}>
            {/* A commit opens what it changed, in place of the list. */}
            <button type="button" className="thread-git-commit" onClick={() => openCommit(item)} aria-label={translate('workbench.gitOpenCommit', { subject: item.subject })}>
              <span className="thread-git-commit-main"><span className="thread-git-commit-subject">{item.subject}</span>
                <span className="thread-git-commit-meta"><code>{shortOid(item.oid)}</code><span className="thread-git-commit-author">{item.author}</span><Time value={item.authoredAt} /></span></span>
              <ChevronRight aria-hidden="true" className="thread-git-chevron" />
            </button>
          </li>)}
        </ol>}
        {commits && !commits.commits.length && <p className="thread-git-empty">{translate('workbench.gitNoCommits')}</p>}
        {(offset > 0 || commits?.hasMore) && <div className="thread-git-pagination">
          <button type="button" className="thread-git-icon" disabled={loading || offset === 0} onClick={() => setOffset(value => Math.max(0, value - PAGE))} aria-label={translate('workbench.gitPrevious')} title={translate('workbench.gitPrevious')}><ChevronLeft aria-hidden="true" /></button>
          <span className="thread-git-dim">{gt('page', { n: formatNumber(offset / PAGE + 1) })}</span>
          <button type="button" className="thread-git-icon" disabled={loading || !commits?.hasMore} onClick={() => setOffset(value => value + PAGE)} aria-label={translate('workbench.gitNext')} title={translate('workbench.gitNext')}><ChevronRight aria-hidden="true" /></button>
        </div>}
      </section>}
    </>}
  </div>;
}

/** A working copy's short name: its branch, or its directory when detached. */
function copyName(copy: WorkingCopy) { return copy.branch ?? baseName(copy.path); }

/** A working copy in the switch: the branch, then (in the list only) its directory and role. */
function CopyOption({ copy, primary }: { copy: WorkingCopy; primary: boolean }) {
  return <span className="thread-git-option" data-detached={!copy.branch || undefined}>
    <span className="thread-git-option-branch">{copy.branch ?? translate('workbench.gitDetached')}</span>
    <span className="thread-git-option-meta"> · {baseName(copy.path)}{primary ? ` · ${gt('primaryShort')}` : ''}</span>
  </span>;
}

/** A scope's file count; zero is shown dimmed, an unknown count not at all. */
function Count({ value }: { value: number | null }) {
  return value === null ? null : <span className="thread-git-count" data-zero={value === 0 || undefined}>{formatNumber(value)}</span>;
}

/** The diff, file by file, with additions and deletions colored. Text is unchanged;
    Git's header lines become the file's heading and a note in words. */
function PatchView({ parsed, single, onSelect }: { parsed: ReturnType<typeof parsePatch>; single: boolean; onSelect(path: string): void }) {
  return <div className="thread-git-patch" tabIndex={0}>
    {parsed.preamble.length > 0 && <Lines lines={parsed.preamble} />}
    {parsed.files.map((file, index) => <section key={`${file.path}:${index}`} className="thread-git-patch-file">
      {!single && <button type="button" className="thread-git-patch-file-head" aria-label={translate('workbench.gitViewFile', { path: file.path })} onClick={() => onSelect(file.path)}>
        <MiddleName name={file.path} />
        {file.created || file.removed ? <span className="thread-git-dim">{gt(file.created ? 'newFile' : 'deletedFile')}</span> : null}
        {file.binary ? <span className="thread-git-dim">{gt('binary')}</span> : <Stat file={file} />}
      </button>}
      <FileNote file={file} />
      {file.lines.some(line => line.kind !== 'meta') && <Lines lines={file.lines} />}
    </section>)}
  </div>;
}

/** What the patch cannot show as lines, in words: binary contents, an empty file, a rename. */
function FileNote({ file }: { file: PatchFile }) {
  const content = file.lines.some(line => line.kind !== 'meta');
  const note = file.binary ? gt(file.created ? 'binaryNew' : file.removed ? 'binaryDeleted' : 'binaryChanged')
    : !content && file.created ? gt('emptyFile')
    : file.oldPath ? gt(content ? 'renamedFrom' : 'renamedOnly', { path: file.oldPath })
    : !content ? gt('noContentChange') : null;
  if (!note) return null;
  return <p className="thread-git-file-note">{file.binary ? <FileQuestion aria-hidden="true" /> : null}<span>{note}</span></p>;
}

/** Lines added and removed; a side with none is left out, not shown as a red 0. */
function Stat({ file }: { file: Pick<PatchFile, 'additions' | 'deletions'> }) {
  if (!file.additions && !file.deletions) return null;
  return <span className="thread-git-stat" title={gt('statTitle', { added: formatNumber(file.additions), removed: formatNumber(file.deletions) })}>
    {file.additions > 0 && <ins>+{formatNumber(file.additions)}</ins>}
    {file.deletions > 0 && <del>−{formatNumber(file.deletions)}</del>}
  </span>;
}

function Lines({ lines }: { lines: PatchLine[] }) {
  return <pre>{lines.map((line, index) => line.kind === 'meta' ? null : <span key={index} className="thread-git-line" data-kind={line.kind}>{line.text || ' '}{'\n'}</span>)}</pre>;
}
