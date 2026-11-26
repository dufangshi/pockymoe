import { useCallback, useEffect, useRef, useState } from 'react';
import { ChevronLeft, ExternalLink, FileCode2, GitCommitHorizontal, GitMerge, Link2, ListChecks, Map as MapIcon, MessageSquareText, MessagesSquare, MoreHorizontal, RefreshCw, type LucideIcon } from 'lucide-react';
import { ErrorMark, Select, confirmAsync, copyText } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { useNavigate } from 'react-router-dom';
import { FormDialog } from '../../../FormDialog';
import { threadHref } from '../../../../lib/relayRoutes';
import { seedThreadDraft } from '../../../../pages/useThreadDrafts';
import { reviewAction, type DetailResponse, type Draft, type GuideState, type PullFile, type ReviewEvent } from './reviewApi';
import { ConversationTab } from './ConversationTab';
import { CodeTab } from './CodeTab';
import { ChecksTab, CommitsTab, GuideTab } from './OtherTabs';
import { checkCounts } from './listModel';
import { Ago, ChecksMark, DecisionMark, MenuButton, PullStateMark, type MenuItem } from './parts';
import { rt, type ReviewMessage } from './reviewText';
import './review.css';

export type ReviewTab = 'conversation' | 'code' | 'commits' | 'checks' | 'guide';
const TABS: ReviewTab[] = ['conversation', 'code', 'commits', 'checks', 'guide'];
const TAB_LABEL: Record<ReviewTab, ReviewMessage> = { conversation: 'conversation', code: 'code', commits: 'commits', checks: 'checksTab', guide: 'guide' };
// Narrow, tabs are icons (the open one keeps its name) so all five fit a phone.
const TAB_ICON: Record<ReviewTab, LucideIcon> = { conversation: MessagesSquare, code: FileCode2, commits: GitCommitHorizontal, checks: ListChecks, guide: MapIcon };
const EVENT_LABEL: Record<ReviewEvent, ReviewMessage> = { comment: 'eventComment', approve: 'eventApprove', requestChanges: 'eventRequestChanges' };
const METHOD_LABEL: Record<string, ReviewMessage> = { squash: 'squash', merge: 'mergeCommit', rebase: 'rebase' };
const MERGE_STATE: Record<string, ReviewMessage> = { clean: 'mergeable', conflicts: 'conflicts', blocked: 'blocked', behind: 'behind', unstable: 'unstable' };
const POLL = 60_000;

const message = (caught: unknown) => caught instanceof Error ? caught.message : String(caught);

export interface PullRequestReviewProps {
  url: string;
  deviceId?: string | null | undefined;
  tab?: ReviewTab | undefined;
  onTab?: ((tab: ReviewTab) => void) | undefined;
  onBack?: (() => void) | undefined;
  /** Called after a write that changes the list row (state, merge, review). */
  onChanged?: (() => void) | undefined;
}

/** One pull request under review: the header with the actions the host allows,
    a notice when the head moved, then Conversation, Code, Commits, Checks and
    Guide. Every review and merge carries the head the reader loaded. */
export function PullRequestReview({ url, deviceId, tab: controlledTab, onTab, onBack, onChanged }: PullRequestReviewProps) {
  useI18n();
  const navigate = useNavigate();
  const [data, setData] = useState<DetailResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [ownTab, setOwnTab] = useState<ReviewTab>('conversation');
  const tab = controlledTab ?? ownTab;
  const setTab = (next: ReviewTab) => { setOwnTab(next); onTab?.(next); };
  const [files, setFiles] = useState<{ headSha: string; files: PullFile[]; viewedOnHost: boolean } | null>(null);
  const [filesError, setFilesError] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [guide, setGuide] = useState<GuideState>({ guide: null, stale: false, run: null });
  const [shownHead, setShownHead] = useState('');
  const [latestHead, setLatestHead] = useState('');
  const [dialog, setDialog] = useState<null | 'review' | 'merge' | 'ask'>(null);
  const [ask, setAsk] = useState('');
  const [focusPath, setFocusPath] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const filesWanted = useRef(false);

  const load = useCallback(async (moveHead: boolean) => {
    setLoading(true);
    try {
      const next = await reviewAction<DetailResponse>(deviceId, 'detail', url);
      setData(next); setDrafts(next.drafts); setGuide(next.guide); setError(null);
      setShownHead(current => !current || moveHead ? next.detail.headSha : current);
      setLatestHead(next.detail.headSha);
      return next;
    } catch (caught) { setError(message(caught)); return null; } finally { setLoading(false); }
  }, [deviceId, url]);
  const loadFiles = useCallback(async () => {
    filesWanted.current = true;
    try { setFiles(await reviewAction(deviceId, 'files', url)); setFilesError(null); } catch (caught) { setFilesError(message(caught)); }
  }, [deviceId, url]);
  useEffect(() => { setData(null); setFiles(null); setShownHead(''); filesWanted.current = false; void load(true); }, [load]);
  useEffect(() => { if ((tab === 'code' || tab === 'guide') && !filesWanted.current) void loadFiles(); }, [tab, loadFiles]);
  // New commits during a review: poll the head (cheap), and when the page shows again.
  useEffect(() => {
    if (!data || data.detail.state !== 'open') return;
    const check = () => { if (document.visibilityState === 'visible') void reviewAction<{ headSha: string }>(deviceId, 'head', url).then(value => setLatestHead(value.headSha)).catch(() => {}); };
    const timer = window.setInterval(check, POLL);
    document.addEventListener('visibilitychange', check);
    return () => { window.clearInterval(timer); document.removeEventListener('visibilitychange', check); };
  }, [data, deviceId, url]);
  // Another device may have changed the drafts: reread them when the page shows.
  useEffect(() => {
    const reread = () => { if (document.visibilityState === 'visible') void reviewAction<{ drafts: Draft[] }>(deviceId, 'drafts', url).then(value => setDrafts(value.drafts)).catch(() => {}); };
    document.addEventListener('visibilitychange', reread);
    return () => document.removeEventListener('visibilitychange', reread);
  }, [deviceId, url]);
  const running = guide.run?.state === 'running';
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => { void reviewAction<GuideState>(deviceId, 'guideShow', url, { headSha: shownHead }).then(setGuide).catch(() => {}); }, 5000);
    return () => window.clearInterval(timer);
  }, [running, deviceId, url, shownHead]);

  const act = useCallback(async (action: string, input: Record<string, unknown> = {}, options: { files?: boolean } = {}) => {
    setBusy(true); setActionError(null);
    try {
      const result = await reviewAction<Record<string, unknown>>(deviceId, action, url, input);
      if (action === 'guide') setGuide(result as unknown as GuideState);
      else if (action !== 'viewed') { await load(false); onChanged?.(); }
      if (options.files) void loadFiles();
      return result ?? true;
    } catch (caught) {
      const text = message(caught);
      setActionError(text);
      if (/head moved/i.test(text)) void reviewAction<{ headSha: string }>(deviceId, 'head', url).then(value => setLatestHead(value.headSha)).catch(() => {});
      return false;
    } finally { setBusy(false); }
  }, [deviceId, url, load, loadFiles, onChanged]);

  const saveDraft = useCallback(async (input: Partial<Draft> & { kind: Draft['kind'] }) => {
    try { const result = await reviewAction<{ drafts: Draft[] }>(deviceId, 'draftSave', url, input); setDrafts(result.drafts); return true; }
    catch (caught) { setActionError(message(caught)); return false; }
  }, [deviceId, url]);
  const deleteDraft = useCallback((id: string) => {
    void reviewAction<{ drafts: Draft[] }>(deviceId, 'draftDelete', url, { id }).then(result => setDrafts(result.drafts)).catch(caught => setActionError(message(caught)));
  }, [deviceId, url]);
  const setViewed = useCallback((paths: string[], viewed: boolean) => {
    // Shown at once; the store or the host follows.
    setFiles(current => current && { ...current, files: current.files.map(file => paths.includes(file.path) ? { ...file, viewed: viewed ? 'viewed' : 'unviewed' } : file) });
    const revisions = Object.fromEntries((files?.files ?? []).filter(file => paths.includes(file.path)).map(file => [file.path, file.digest]));
    void reviewAction(deviceId, 'viewed', url, { paths, viewed, revisions }).catch(caught => { setActionError(message(caught)); void loadFiles(); });
  }, [deviceId, url, files, loadFiles]);
  const loadNew = async () => { const next = await load(true); if (next && filesWanted.current) void loadFiles(); };
  const openAsk = (context: string) => { setAsk(context); setDialog('ask'); };

  if (!data) return <div className="pr-review" aria-busy={loading}>
    <div className="pr-review-head">{onBack && <button type="button" className="pr-icon" onClick={onBack} aria-label={rt('back')} title={rt('back')}><ChevronLeft aria-hidden="true" /></button>}</div>
    {error ? <p className="pr-empty"><ErrorMark message={error} onRetry={() => { void load(true); }} /></p> : <p className="pr-empty">{rt('loading')}</p>}
  </div>;
  const { detail } = data;
  const caps = detail.capabilities;
  const open = detail.state === 'open';
  const moved = open && !!latestHead && !!shownHead && latestHead !== shownHead;
  const pendingCount = drafts.filter(draft => draft.kind !== 'summary').length;
  const counts = checkCounts(detail.checkRuns);
  const menu: MenuItem[] = [
    { label: rt('openHost'), icon: <ExternalLink aria-hidden="true" />, href: detail.url },
    { label: rt('copyLink'), icon: <Link2 aria-hidden="true" />, onSelect: () => { void copyText(detail.url); } },
    ...(data.workspace ? [{ label: rt('workOn'), onSelect: () => { void act('workOn').then(result => { const thread = (result as { thread?: { id: string } })?.thread; if (thread) navigate(threadHref(thread.id, deviceId ?? undefined)); }); } }] : []),
    ...data.linkedThreads.slice(0, 5).map(thread => ({ label: `${rt('openBeside')}: ${thread.title}`, onSelect: () => navigate(`${threadHref(thread.id, deviceId ?? undefined)}?${new URLSearchParams({ pr: detail.url })}`) })),
    { label: '', separator: true },
    ...(open && detail.draft && caps.ready ? [{ label: rt('ready'), onSelect: () => { void act('state', { state: 'ready' }); } }] : []),
    ...(open && !detail.draft && caps.convertToDraft ? [{ label: rt('toDraft'), onSelect: () => { void act('state', { state: 'draft' }); } }] : []),
    ...(open && detail.autoMerge && caps.autoMerge ? [{ label: rt('autoMergeOff'), onSelect: () => { void act('autoMerge', { enable: false }); } }] : []),
    ...(data.workspace ? [{ label: guide.guide ? rt('rewriteGuide') : rt('writeGuide'), disabled: running, onSelect: () => { setTab('guide'); void act('guide'); } }] : []),
    { label: '', separator: true },
    ...(open && caps.close ? [{ label: rt('close'), danger: true, onSelect: () => { void confirmAsync({ title: rt('closeConfirm', { n: detail.number }), description: rt('closeBody'), confirmLabel: rt('close'), tone: 'danger' }).then(ok => { if (ok) void act('state', { state: 'close' }); }); } }] : []),
    ...(detail.state === 'closed' && caps.reopen ? [{ label: rt('reopen'), onSelect: () => { void act('state', { state: 'reopen' }); } }] : []),
    ...(detail.state === 'merged' && caps.revert ? [{ label: rt('revert'), danger: true, onSelect: () => { void confirmAsync({ title: rt('revertConfirm', { n: detail.number }), description: rt('revertBody'), confirmLabel: rt('revert'), tone: 'primary' }).then(ok => { if (ok) void act('revert').then(result => { const opened = result as { url?: string }; if (opened?.url) setNotice(`${rt('revertOpened')}: ${opened.url}`); }); }); } }] : []),
  ].filter((item, index, all) => !(item.separator && (index === 0 || all[index - 1]?.separator || index === all.length - 1)));
  return <div className="pr-review" data-testid="pr-review">
    <header className="pr-review-head">
      {onBack && <button type="button" className="pr-icon pr-back" onClick={onBack} aria-label={rt('back')} title={rt('back')}><ChevronLeft aria-hidden="true" /></button>}
      <PullStateMark state={detail.state} draft={detail.draft} />
      <div className="pr-review-title">
        <h2><span>{detail.title}</span> <span className="pr-dim">#{detail.number}</span></h2>
        <p className="pr-row-meta">
          <span>{detail.repository.owner}/{detail.repository.name}</span>
          {detail.author && <span>{detail.author.login}</span>}
          <span className="pr-branches"><code>{detail.headRef}</code>→<code>{detail.baseRef}</code></span>
          <Ago value={detail.updatedAt} />
          <ChecksMark checks={detail.checks} /><DecisionMark decision={detail.reviewDecision} />
        </p>
      </div>
      <div className="pr-review-actions">
        <button type="button" className="pr-icon" onClick={() => { void load(false); if (filesWanted.current) void loadFiles(); }} disabled={loading} aria-label={rt('refresh')} title={rt('refresh')}><RefreshCw aria-hidden="true" className={loading ? 'request-spin' : undefined} /></button>
        {caps.submit.length > 0 && <button type="button" className="pr-button" onClick={() => setDialog('review')}><MessageSquareText aria-hidden="true" />{rt('reviewButton')}{pendingCount > 0 && <span className="pr-badge" aria-label={pendingCount === 1 ? rt('pendingOne') : rt('pendingComments', { n: pendingCount })}>{pendingCount}</span>}</button>}
        {open && (caps.merge || caps.autoMerge) && (detail.autoMerge
          ? <span className="pr-tag" title={rt('autoMergeArmed')}>{rt('autoMerge')}</span>
          : <button type="button" className="pr-button pr-merge" onClick={() => setDialog('merge')}><GitMerge aria-hidden="true" />{rt('merge')}</button>)}
        <MenuButton label={rt('more')} icon={<MoreHorizontal aria-hidden="true" />} items={menu} />
      </div>
    </header>
    {moved && <div className="pr-banner" role="status"><span>{rt('newCommits', { sha: shownHead.slice(0, 7) })}</span><button type="button" onClick={() => { void loadNew(); }}>{rt('loadNew')}</button></div>}
    {(actionError || notice) && <div className="pr-banner" data-tone={actionError ? 'error' : undefined}>{actionError ? <><span>{rt('failed')}</span><ErrorMark message={actionError} /></> : <span>{notice}</span>}</div>}
    <div className="pr-tabs" role="tablist" aria-label={rt('page')}>
      {TABS.filter(item => item !== 'guide' || data.workspace || guide.guide).map(item => { const Icon = TAB_ICON[item]; return <button key={item} type="button" role="tab" aria-selected={tab === item} aria-label={rt(TAB_LABEL[item])} title={rt(TAB_LABEL[item])} onClick={() => setTab(item)}>
        <Icon aria-hidden="true" className="pr-tab-icon" /><span className="pr-tab-label">{rt(TAB_LABEL[item])}</span>
        {item === 'code' && detail.changedFiles ? <span className="pr-count">{detail.changedFiles}</span> : null}
        {item === 'commits' && detail.commits.length ? <span className="pr-count">{detail.commits.length}</span> : null}
        {item === 'checks' && counts.failed ? <span className="pr-count" data-tone="failed">{counts.failed}</span> : null}
        {item === 'conversation' && detail.threads.some(thread => !thread.resolved) ? <span className="pr-count">{detail.threads.filter(thread => !thread.resolved).length}</span> : null}
      </button>; })}
    </div>
    {detail.mergeState && open && MERGE_STATE[detail.mergeState] && tab === 'conversation' && <p className="pr-merge-state" data-state={detail.mergeState}>{rt(MERGE_STATE[detail.mergeState]!)}{detail.checkRuns.length ? ` · ${rt('checksSummary', counts)}` : ''}</p>}
    <div className="pr-tab-panel" role="tabpanel">
      {tab === 'conversation' && <ConversationTab data={data} act={act} busy={busy} />}
      {tab === 'code' && <CodeTab key={focusPath ?? ''} detail={detail} files={files} filesError={filesError} drafts={drafts} shownHead={shownHead} act={act} busy={busy} saveDraft={saveDraft} deleteDraft={deleteDraft} setViewed={setViewed} onAsk={openAsk} onRetry={() => { void loadFiles(); }} focusPath={focusPath} />}
      {tab === 'commits' && <CommitsTab detail={detail} />}
      {tab === 'checks' && <ChecksTab detail={detail} act={act} busy={busy} />}
      {tab === 'guide' && <GuideTab detail={detail} guide={guide} shownHead={shownHead} act={act} busy={busy} drafts={drafts} saveDraft={saveDraft} onAsk={openAsk} onOpenFile={path => { setFocusPath(path); setTab('code'); }} />}
    </div>
    {dialog === 'review' && <SubmitDialog drafts={drafts} events={caps.submit} head={shownHead} moved={moved} busy={busy} host={detail.repository.kind}
      onSummary={body => { void saveDraft({ kind: 'summary', body }); }} onDiscardAll={() => { void reviewAction<{ drafts: Draft[] }>(deviceId, 'draftDelete', url, { all: true }).then(result => setDrafts(result.drafts)); }}
      onLoadNew={() => { void loadNew(); }} onClose={() => setDialog(null)}
      onSubmit={async (event, body) => { const ok = await act('submit', { event, body, expectedHead: shownHead }); if (ok) setDialog(null); return !!ok; }} />}
    {dialog === 'merge' && <MergeDialog data={data} head={shownHead} moved={moved} busy={busy} deviceId={deviceId} onClose={() => setDialog(null)} onLoadNew={() => { void loadNew(); }}
      onMerge={async (input, auto) => { const ok = await act(auto ? 'autoMerge' : 'merge', { ...input, expectedHead: shownHead, enable: true }); if (ok) setDialog(null); }} />}
    {dialog === 'ask' && <AskDialog context={ask} data={data} onClose={() => setDialog(null)} onSend={(target, prompt) => {
      setDialog(null);
      const key = `${deviceId ?? 'local'}:${target}`;
      if (target !== 'new') { seedThreadDraft(key, prompt, false); navigate(threadHref(target, deviceId ?? undefined)); return; }
      const params = new URLSearchParams({ prompt, title: `PR #${detail.number}` });
      if (data.workspace?.id) params.set('workspaceId', data.workspace.id);
      navigate(`${deviceId ? `/devices/${encodeURIComponent(deviceId)}` : ''}/threads/new?${params}`);
    }} />}
  </div>;
}

function SubmitDialog({ drafts, events, head, moved, busy, host, onSummary, onDiscardAll, onSubmit, onClose, onLoadNew }: {
  drafts: Draft[]; events: ReviewEvent[]; head: string; moved: boolean; busy: boolean; host: string;
  onSummary(body: string): void; onDiscardAll(): void; onSubmit(event: ReviewEvent, body: string): Promise<boolean>; onClose(): void; onLoadNew(): void;
}) {
  const saved = drafts.find(draft => draft.kind === 'summary')?.body ?? '';
  const [body, setBody] = useState(saved);
  const [event, setEvent] = useState<ReviewEvent>(events.includes('comment') ? 'comment' : events[0]!);
  const pending = drafts.filter(draft => draft.kind !== 'summary');
  const needsBody = event === 'requestChanges' && host === 'forgejo';
  const ready = event === 'approve' || !!body.trim() || (!needsBody && pending.length > 0);
  // The summary is a draft too: kept on the device while the window is closed.
  const persist = () => { if (body !== saved) onSummary(body); };
  return <FormDialog secondary title={rt('submitReview')} description={rt('reviewHead', { sha: head.slice(0, 7) })} onClose={() => { persist(); onClose(); }} busy={busy}>
    <form className="pr-form" onSubmit={eventObject => { eventObject.preventDefault(); persist(); void onSubmit(event, body); }}>
      <div className="product-segmented pr-events" role="radiogroup" aria-label={rt('reviewButton')}>
        {events.map(item => <button key={item} type="button" role="radio" className="product-segment" aria-checked={event === item} data-event={item} onClick={() => setEvent(item)}>{rt(EVENT_LABEL[item])}</button>)}
      </div>
      <label>{needsBody ? rt('summaryRequired') : rt('summaryOptional')}<textarea value={body} onChange={change => setBody(change.target.value)} onBlur={persist}
        onKeyDown={key => { if ((key.metaKey || key.ctrlKey) && key.key === 'Enter') key.currentTarget.form?.requestSubmit(); }} /></label>
      <p className="pr-dim pr-pending-line">{pending.length ? (pending.length === 1 ? rt('pendingOne') : rt('pendingComments', { n: pending.length })) : rt('noPending')}
        {pending.length > 0 && <button type="button" className="pr-link pr-danger" onClick={() => { void confirmAsync({ title: rt('discardConfirm', { n: pending.length }), description: rt('pendingNote'), confirmLabel: rt('discard'), tone: 'danger' }).then(ok => { if (ok) onDiscardAll(); }); }}>{rt('discardAll')}</button>}</p>
      {moved && <div className="pr-banner"><span>{rt('newCommits', { sha: head.slice(0, 7) })}</span><button type="button" onClick={onLoadNew}>{rt('loadNew')}</button></div>}
      <div className="form-dialog-actions"><button type="button" data-dialog-cancel onClick={() => { persist(); onClose(); }}>{rt('cancel')}</button><button type="submit" className="relay-button-primary" disabled={busy || !ready || (needsBody && !body.trim())}>{rt('submitReview')}</button></div>
    </form>
  </FormDialog>;
}

function MergeDialog({ data, head, moved, busy, deviceId, onClose, onMerge, onLoadNew }: {
  data: DetailResponse; head: string; moved: boolean; busy: boolean; deviceId?: string | null | undefined;
  onClose(): void; onMerge(input: { method: string; removeAgentCredits: boolean }, auto: boolean): Promise<void>; onLoadNew(): void;
}) {
  const { detail } = data;
  const [method, setMethod] = useState(detail.mergeMethods[0] ?? 'squash');
  const [strip, setStrip] = useState(data.removeAgentCredits);
  const [preview, setPreview] = useState<{ title: string; body: string } | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const canStrip = detail.capabilities.removeCredits && method !== 'rebase';
  const checksPassing = detail.checks === 'passed' || detail.checks === 'none';
  useEffect(() => {
    if (!strip || !canStrip) { setPreview(null); return; }
    let alive = true;
    setPreview(null); setPreviewError(null);
    void reviewAction<{ title: string; body: string }>(deviceId, 'mergeMessage', detail.url, { method }).then(value => { if (alive) setPreview(value); }).catch(caught => { if (alive) setPreviewError(message(caught)); });
    return () => { alive = false; };
  }, [strip, canStrip, method, deviceId, detail.url]);
  return <FormDialog secondary title={rt('mergeTitle', { n: detail.number })} description={rt('mergeHead', { sha: head.slice(0, 7) })} onClose={onClose} busy={busy}>
    <form className="pr-form" onSubmit={event => { event.preventDefault(); void onMerge({ method, removeAgentCredits: strip && canStrip }, false); }}>
      {detail.mergeMethods.length > 1 && <label>{rt('mergeMethod')}<Select value={method} onValueChange={setMethod} options={detail.mergeMethods.map(item => ({ value: item, label: rt(METHOD_LABEL[item] ?? 'mergeCommit') }))} /></label>}
      {detail.capabilities.removeCredits && <label className="pr-check"><input type="checkbox" checked={strip && canStrip} disabled={!canStrip} onChange={event => setStrip(event.target.checked)} />{rt('removeCredits')}</label>}
      <p className="pr-dim">{canStrip ? rt('removeCreditsHint') : detail.capabilities.removeCredits ? rt('removeCreditsRebase') : null}</p>
      {strip && canStrip && (preview ? <div className="pr-message-preview" aria-label={rt('mergeMessage')}><strong>{preview.title}</strong>{preview.body && <pre>{preview.body}</pre>}</div> : previewError ? <ErrorMark message={previewError} /> : <p className="pr-dim">{rt('loading')}</p>)}
      {detail.mergeState && MERGE_STATE[detail.mergeState] && <p className="pr-merge-state" data-state={detail.mergeState}>{rt(MERGE_STATE[detail.mergeState]!)}</p>}
      {moved && <div className="pr-banner"><span>{rt('newCommits', { sha: head.slice(0, 7) })}</span><button type="button" onClick={onLoadNew}>{rt('loadNew')}</button></div>}
      <div className="form-dialog-actions">
        <button type="button" data-dialog-cancel onClick={onClose}>{rt('cancel')}</button>
        {detail.capabilities.autoMerge && !checksPassing && <button type="button" disabled={busy || moved} onClick={() => { void onMerge({ method, removeAgentCredits: strip && canStrip }, true); }}>{rt('autoMergeOn')}</button>}
        {detail.capabilities.merge && <button type="submit" className="ui-action-danger pr-dialog-danger" disabled={busy || moved || detail.mergeable === false}>{rt('merge')}</button>}
      </div>
    </form>
  </FormDialog>;
}

function AskDialog({ context, data, onClose, onSend }: { context: string; data: DetailResponse; onClose(): void; onSend(target: string, prompt: string): void }) {
  const [question, setQuestion] = useState(context.includes('\n\n') ? context.slice(context.lastIndexOf('\n\n') + 2) : '');
  const [target, setTarget] = useState(data.linkedThreads[0]?.id ?? 'new');
  const quoted = context.includes('\n\n') ? context.slice(0, context.lastIndexOf('\n\n')) : context;
  const prompt = `${data.detail.url}\n${quoted}\n\n${question}`.trim();
  return <FormDialog secondary title={rt('askTitle')} onClose={onClose}>
    <form className="pr-form" onSubmit={event => { event.preventDefault(); onSend(target, prompt); }}>
      {quoted && <pre className="pr-ask-context">{quoted}</pre>}
      <label>{rt('askQuestion')}<textarea autoFocus value={question} onChange={event => setQuestion(event.target.value)} /></label>
      <label>{rt('askTarget')}<Select value={target} onValueChange={setTarget} options={[...data.linkedThreads.map(thread => ({ value: thread.id, label: thread.title })), ...(data.workspace?.id ? [{ value: 'new', label: rt('askNew') }] : [])]} /></label>
      <div className="form-dialog-actions"><button type="button" data-dialog-cancel onClick={onClose}>{rt('cancel')}</button><button type="submit" className="relay-button-primary" disabled={!question.trim() || (target === 'new' && !data.workspace?.id)}>{rt('askSend')}</button></div>
    </form>
  </FormDialog>;
}
