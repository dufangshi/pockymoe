import { useMemo, useState } from 'react';
import { FileDiff, MessageSquarePlus, Trash2 } from 'lucide-react';
import { ErrorMark, MiddleName, PathChip } from '@pockymoe/thread-ui';
import { FormDialog } from '../../../FormDialog';
import { ReviewPatch, type LineComment } from '../../ReviewPatch';
import { parsePatch, type PatchFile } from '../../patch';
import type { Draft, PullDetail, PullFile, ReviewThread } from './reviewApi';
import { ThreadCard, type Act } from './ConversationTab';
import { Stat } from './parts';
import { rt } from './reviewText';
import '../../thread-git.css';

const baseName = (path: string) => path.split('/').pop() || path;
const dirName = (path: string) => path.includes('/') ? path.slice(0, path.lastIndexOf('/') + 1) : '';
const LETTER: Record<string, string> = { added: 'A', removed: 'D', renamed: 'R', copied: 'C', modified: 'M' };
const CHANGE: Record<string, string> = { added: 'added', removed: 'deleted', renamed: 'renamed', copied: 'copied', modified: 'modified' };
const side = (value: 'left' | 'right' | null | undefined) => value === 'left' ? 'old' : 'new';

export interface CodeTabProps {
  detail: PullDetail;
  files: { headSha: string; files: PullFile[]; viewedOnHost: boolean } | null;
  filesError: string | null;
  drafts: Draft[];
  shownHead: string;
  act: Act;
  busy: boolean;
  saveDraft(input: Partial<Draft> & { kind: Draft['kind'] }): Promise<boolean>;
  deleteDraft(id: string): void;
  setViewed(paths: string[], viewed: boolean): void;
  onAsk(context: string): void;
  onRetry(): void;
  focusPath?: string | null;
}

/** Changed files with the shared diff viewer: Viewed ticks collapse a file
    (it reopens marked Changed after a push); selected lines become drafts. */
export function CodeTab({ detail, files, filesError, drafts, shownHead, act, busy, saveDraft, deleteDraft, setViewed, onAsk, onRetry, focusPath }: CodeTabProps) {
  const [path, setPath] = useState<string>(focusPath ?? '');
  const [fileComment, setFileComment] = useState<{ path: string; body: string } | null>(null);
  const list = useMemo(() => files?.files ?? [], [files]);
  const viewedCount = list.filter(file => file.viewed === 'viewed').length;
  const chosen = path ? list.filter(file => file.path === path) : list;
  const patch = useMemo(() => chosen.map(file => file.patch ?? '').filter(Boolean).join('\n'), [chosen]);
  const parsed = useMemo(() => parsePatch(patch), [patch]);
  const viewed = useMemo(() => new Map(list.map(file => [file.path, file.viewed])), [list]);
  const lineDrafts = drafts.filter(draft => draft.kind === 'line');
  const counts = useMemo(() => {
    const map = new Map<string, number>();
    for (const item of [...detail.threads.filter(thread => !thread.resolved).map(thread => thread.path), ...drafts.filter(draft => draft.kind !== 'summary').map(draft => draft.path ?? '')]) map.set(item, (map.get(item) ?? 0) + 1);
    return map;
  }, [detail.threads, drafts]);
  const comment = (input: LineComment) => {
    void saveDraft({ kind: 'line', path: input.path, side: input.side === 'old' ? 'left' : 'right', line: input.endLine, startLine: input.startLine === input.endLine ? null : input.startLine, body: input.comment, headSha: shownHead });
    return true;
  };
  const annotation = (file: string, lineSide: 'old' | 'new', line: number) => {
    const threads = detail.threads.filter(thread => thread.subject === 'line' && thread.path === file && thread.line === line && side(thread.side) === lineSide);
    const mine = lineDrafts.filter(draft => draft.path === file && draft.line === line && side(draft.side) === lineSide);
    if (!threads.length && !mine.length) return null;
    return <>{threads.map(thread => <ThreadCard key={thread.id} thread={thread} act={act} busy={busy} compact />)}{mine.map(draft => <DraftCard key={draft.id} draft={draft} shownHead={shownHead} onDelete={() => deleteDraft(draft.id)} />)}</>;
  };
  const fileThreads = (file: string): ReviewThread[] => detail.threads.filter(thread => thread.subject === 'file' && thread.path === file);
  const fileHeader = (file: PatchFile) => {
    const state = viewed.get(file.path) ?? 'unviewed';
    return <span className="pr-file-tools">
      {state === 'changed' && <span className="pr-changed" title={rt('changedTitle')}>{rt('changedSince')}</span>}
      {detail.capabilities.lineComments && <button type="button" className="pr-icon" aria-label={`${rt('fileComment')}: ${file.path}`} title={rt('fileComment')} onClick={() => setFileComment({ path: file.path, body: '' })}><MessageSquarePlus aria-hidden="true" /></button>}
      <label className="pr-check pr-viewed"><input type="checkbox" checked={state === 'viewed'} onChange={event => setViewed([file.path], event.target.checked)} />{rt('viewed')}</label>
    </span>;
  };
  if (!files) return <div className="pr-tab-body">{filesError ? <p className="pr-empty"><ErrorMark message={filesError} onRetry={onRetry} /></p> : <p className="pr-empty" aria-busy="true">{rt('loading')}</p>}</div>;
  return <div className="pr-code">
    <aside className="pr-files" aria-label={rt('code')}>
      <div className="pr-files-head">
        <span>{list.length === 1 ? rt('oneFile') : rt('files', { n: list.length })} · {rt('viewedCount', { viewed: viewedCount, total: list.length })}</span>
        {!files.viewedOnHost && <span className="pr-dim" title={rt('viewedLocal')}>ⓘ<span className="thread-git-sr">{rt('viewedLocal')}</span></span>}
        <button type="button" className="thread-git-all" aria-pressed={!path} onClick={() => setPath('')}><FileDiff aria-hidden="true" />{rt('all')}</button>
      </div>
      <ul className="pr-file-list">
        {list.map(file => <li key={file.path} data-viewed={file.viewed}>
          <input type="checkbox" aria-label={`${rt('viewed')}: ${file.path}`} checked={file.viewed === 'viewed'} onChange={event => setViewed([file.path], event.target.checked)} />
          <button type="button" className="thread-git-file" aria-pressed={path === file.path} title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path} onClick={() => setPath(path === file.path ? '' : file.path)}>
            <span className="thread-git-letter" data-change={CHANGE[file.status] ?? 'modified'} aria-hidden="true">{LETTER[file.status] ?? 'M'}</span>
            <MiddleName name={baseName(file.path)} className="thread-git-file-name" />
            <span className="thread-git-file-dir"><bdi>{dirName(file.path)}</bdi></span>
            {file.viewed === 'changed' && <span className="pr-changed">{rt('changedSince')}</span>}
            {(counts.get(file.path) ?? 0) > 0 && <span className="pr-count" title={rt('threads')}>{counts.get(file.path)}</span>}
            <Stat additions={file.additions} deletions={file.deletions} />
          </button>
        </li>)}
      </ul>
    </aside>
    <section className="pr-diff" aria-label={path || rt('all')}>
      {chosen.flatMap(file => [...fileThreads(file.path).map(thread => <ThreadCard key={thread.id} thread={thread} act={act} busy={busy} />), ...drafts.filter(draft => draft.kind === 'file' && draft.path === file.path).map(draft => <DraftCard key={draft.id} draft={draft} shownHead={shownHead} onDelete={() => deleteDraft(draft.id)} />)])}
      {chosen.filter(file => !file.patch).map(file => <p key={file.path} className="pr-dim pr-no-patch"><PathChip path={file.path} /> {rt('noPatch')}<a href={detail.url} target="_blank" rel="noreferrer">{rt('openOnHost')}</a></p>)}
      {parsed.files.length > 0 && <ReviewPatch patch={patch} parsed={parsed} single={!!path} onSelect={setPath} split={false} collapsed={false} area="pr" write={undefined} busy={busy}
        onLineComment={detail.capabilities.lineComments ? comment : undefined} commentLabel={rt('addToReview')}
        secondaryComment={{ label: rt('askAgent'), run: input => onAsk(`${input.path}:${input.startLine}-${input.endLine}\n\`\`\`\n${input.text}\n\`\`\`\n\n${input.comment}`) }}
        annotation={annotation} fileHeader={fileHeader} defaultClosed={file => viewed.get(file) === 'viewed'} />}
    </section>
    {fileComment && <FormDialog secondary title={rt('fileComment')} onClose={() => setFileComment(null)}>
      <form className="pr-form" onSubmit={event => { event.preventDefault(); void saveDraft({ kind: 'file', path: fileComment.path, body: fileComment.body, headSha: shownHead }).then(ok => { if (ok) setFileComment(null); }); }}>
        <PathChip path={fileComment.path} />
        <textarea aria-label={rt('comment')} autoFocus value={fileComment.body} onChange={event => setFileComment({ ...fileComment, body: event.target.value })} />
        <div className="form-dialog-actions"><button type="button" data-dialog-cancel onClick={() => setFileComment(null)}>{rt('cancel')}</button><button type="submit" className="relay-button-primary" disabled={!fileComment.body.trim()}>{rt('addToReview')}</button></div>
      </form>
    </FormDialog>}
  </div>;
}

export function DraftCard({ draft, shownHead, onDelete }: { draft: Draft; shownHead: string; onDelete(): void }) {
  return <article className="pr-draft">
    <header>
      <span className="pr-tag pr-tag-pending">{rt('pendingNote')}</span>
      {draft.kind === 'file' && draft.path && <PathChip path={draft.path} />}
      {draft.headSha && shownHead && draft.headSha !== shownHead && <span className="pr-changed">{rt('staleDraft')}</span>}
      <button type="button" className="pr-icon" aria-label={rt('discard')} title={rt('discard')} onClick={onDelete}><Trash2 aria-hidden="true" /></button>
    </header>
    <p className="pr-body">{draft.body}</p>
  </article>;
}
