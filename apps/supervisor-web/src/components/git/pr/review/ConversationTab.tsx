import { useState } from 'react';
import { Check, ChevronDown, ChevronRight, MessageSquare, Plus, RotateCcw, X } from 'lucide-react';
import { PathChip } from '@pockymoe/thread-ui';
import type { DetailResponse, ReviewThread } from './reviewApi';
import { Ago, Body } from './parts';
import { rt, type ReviewMessage } from './reviewText';

export type Act = (action: string, input?: Record<string, unknown>, options?: { files?: boolean }) => Promise<unknown>;

const REVIEW_VERB: Record<string, ReviewMessage> = { approved: 'reviewedApproved', changesRequested: 'reviewedChanges', commented: 'reviewedCommented', dismissed: 'reviewedDismissed' };
const REVIEWER_STATE: Record<string, ReviewMessage> = { requested: 'stateRequested', approved: 'approved', changesRequested: 'changesRequested', commented: 'stateCommented', dismissed: 'stateDismissed', pending: 'pending' };

/** One review thread: where, the comments, then Reply and Resolve. Used in the
    conversation and under its line in the code. */
export function ThreadCard({ thread, act, busy, compact = false }: { thread: ReviewThread; act: Act; busy: boolean; compact?: boolean }) {
  const [open, setOpen] = useState(!thread.resolved);
  const [reply, setReply] = useState<string | null>(null);
  return <article className="pr-thread" data-resolved={thread.resolved || undefined}>
    <header>
      <button type="button" className="pr-icon" aria-expanded={open} onClick={() => setOpen(value => !value)} aria-label={open ? rt('back') : rt('threads')}>{open ? <ChevronDown aria-hidden="true" /> : <ChevronRight aria-hidden="true" />}</button>
      {!compact && <PathChip path={thread.line ? `${thread.path}:${thread.startLine ? `${thread.startLine}–` : ''}${thread.line}` : thread.path} />}
      {thread.resolved && <span className="pr-tag">{rt('resolved')}</span>}
      {thread.outdated && <span className="pr-tag">{rt('outdated')}</span>}
      {compact && !open && <span className="pr-dim pr-thread-preview">{thread.comments[0]?.author?.login}: {thread.comments[0]?.body.slice(0, 80)}</span>}
    </header>
    {open && <>
      {thread.comments.map(comment => <div key={comment.id} className="pr-comment">
        <div className="pr-comment-head"><strong>{comment.author?.login ?? '—'}</strong><Ago value={comment.createdAt} />{comment.url && <a className="pr-dim" href={comment.url} target="_blank" rel="noreferrer">↗</a>}</div>
        <Body text={comment.body} />
      </div>)}
      <div className="pr-thread-actions">
        {thread.canReply && reply === null && <button type="button" onClick={() => setReply('')}><MessageSquare aria-hidden="true" />{rt('reply')}</button>}
        {thread.canResolve && <button type="button" disabled={busy} onClick={() => { void act('resolve', { threadId: thread.id, resolved: !thread.resolved }); }}>{thread.resolved ? <RotateCcw aria-hidden="true" /> : <Check aria-hidden="true" />}{thread.resolved ? rt('unresolve') : rt('resolve')}</button>}
      </div>
      {reply !== null && <form className="pr-reply" onSubmit={event => { event.preventDefault(); void act('reply', { threadId: thread.id, body: reply }).then(ok => { if (ok) setReply(null); }); }}>
        <textarea aria-label={rt('reply')} placeholder={rt('replyPlaceholder')} value={reply} autoFocus onChange={event => setReply(event.target.value)}
          onKeyDown={event => { if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') event.currentTarget.form?.requestSubmit(); if (event.key === 'Escape') { event.stopPropagation(); setReply(null); } }} />
        <div className="pr-reply-actions"><button type="button" onClick={() => setReply(null)}>{rt('cancel')}</button><button type="submit" className="pr-primary" disabled={busy || !reply.trim()}>{rt('reply')}</button></div>
      </form>}
    </>}
  </article>;
}

export function ConversationTab({ data, act, busy }: { data: DetailResponse; act: Act; busy: boolean }) {
  const { detail } = data;
  const [comment, setComment] = useState('');
  const [adding, setAdding] = useState<string | null>(null);
  const [showResolved, setShowResolved] = useState(false);
  const threads = detail.threads.filter(thread => showResolved || !thread.resolved);
  const resolvedCount = detail.threads.filter(thread => thread.resolved).length;
  return <div className="pr-tab-body">
    <section className="pr-section" aria-label={rt('description')}>
      <div className="pr-comment-head"><strong>{detail.author?.login ?? '—'}</strong><Ago value={detail.createdAt} /></div>
      <Body text={detail.body} empty={rt('noDescription')} />
    </section>
    <section className="pr-section" aria-label={rt('reviewers')}>
      <h3>{rt('reviewers')}</h3>
      <ul className="pr-reviewers">
        {detail.reviewers.map(reviewer => <li key={reviewer.login} data-state={reviewer.state}>
          <span className="pr-reviewer-login">{reviewer.login}</span><span className="pr-dim">{rt(REVIEWER_STATE[reviewer.state] ?? 'stateRequested')}</span>
          {detail.capabilities.reviewers && reviewer.state === 'requested' && <button type="button" className="pr-icon" disabled={busy} aria-label={rt('removeReviewer', { login: reviewer.login })} title={rt('removeReviewer', { login: reviewer.login })} onClick={() => { void act('reviewers', { remove: [reviewer.login] }); }}><X aria-hidden="true" /></button>}
        </li>)}
        {!detail.reviewers.length && <li className="pr-dim">{rt('noReviewers')}</li>}
      </ul>
      {detail.capabilities.reviewers && (adding === null
        ? <button type="button" className="pr-link" onClick={() => setAdding('')}><Plus aria-hidden="true" />{rt('requestReview')}</button>
        : <form className="pr-inline-form" onSubmit={event => { event.preventDefault(); void act('reviewers', { add: adding.split(/[\s,]+/).filter(Boolean) }).then(ok => { if (ok) setAdding(null); }); }}>
          <input aria-label={rt('reviewerLogin')} placeholder={rt('reviewerLogin')} value={adding} autoFocus spellCheck={false} autoCapitalize="off" onChange={event => setAdding(event.target.value)} onKeyDown={event => { if (event.key === 'Escape') { event.stopPropagation(); setAdding(null); } }} />
          <button type="button" onClick={() => setAdding(null)}>{rt('cancel')}</button><button type="submit" className="pr-primary" disabled={busy || !adding.trim()}>{rt('requestReview')}</button>
        </form>)}
    </section>
    {detail.timeline.length > 0 && <ol className="pr-timeline" aria-label={rt('conversation')}>
      {detail.timeline.map(entry => <li key={`${entry.kind}:${entry.id}`} data-state={entry.state ?? undefined}>
        <div className="pr-comment-head"><strong>{entry.author?.login ?? '—'}</strong><span className="pr-dim">{entry.kind === 'review' ? rt(REVIEW_VERB[entry.state ?? ''] ?? 'reviewedCommented') : rt('commented')}</span><Ago value={entry.createdAt} />{entry.url && <a className="pr-dim" href={entry.url} target="_blank" rel="noreferrer">↗</a>}</div>
        <Body text={entry.body} />
      </li>)}
    </ol>}
    {detail.threads.length > 0 && <section className="pr-section" aria-label={rt('threads')}>
      <h3>{rt('threads')} <span className="pr-dim">{detail.threads.length}</span></h3>
      {resolvedCount > 0 && <label className="pr-check"><input type="checkbox" checked={showResolved} onChange={event => setShowResolved(event.target.checked)} />{rt('showResolved')} ({resolvedCount})</label>}
      {threads.map(thread => <ThreadCard key={thread.id} thread={thread} act={act} busy={busy} />)}
    </section>}
    <form className="pr-composer" onSubmit={event => { event.preventDefault(); void act('comment', { body: comment }).then(ok => { if (ok) setComment(''); }); }}>
      <textarea aria-label={rt('comment')} placeholder={rt('commentPlaceholder')} value={comment} onChange={event => setComment(event.target.value)}
        onKeyDown={event => { if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') event.currentTarget.form?.requestSubmit(); }} />
      <div className="pr-reply-actions"><button type="submit" className="pr-primary" disabled={busy || !comment.trim()}>{rt('postComment')}</button></div>
    </form>
  </div>;
}
