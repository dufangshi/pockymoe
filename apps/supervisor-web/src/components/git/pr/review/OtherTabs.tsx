import { useMemo, useState } from 'react';
import { Hourglass, ChevronLeft, ChevronRight, CircleCheck, CircleDashed, CircleMinus, CircleX, ExternalLink, MessageCircleQuestion, RotateCw, TriangleAlert } from 'lucide-react';
import { CopyButton, ErrorMark, PathChip } from '@pockymoe/thread-ui';
import { ReviewPatch, type LineComment } from '../../ReviewPatch';
import { parsePatch } from '../../patch';
import type { Check, Draft, GuideState, GuideStop, PullDetail } from './reviewApi';
import type { Act } from './ConversationTab';
import { Ago } from './parts';
import { checkCounts } from './listModel';
import { rt, type ReviewMessage } from './reviewText';

const CHECK_ICON = { passed: CircleCheck, failed: CircleX, pending: CircleDashed, skipped: CircleMinus, neutral: CircleMinus, cancelled: CircleMinus } as const;
const CHECK_LABEL: Record<Check['state'], ReviewMessage> = { passed: 'passing', failed: 'failing', pending: 'pending', skipped: 'checksAll', neutral: 'checksAll', cancelled: 'failing' };

function duration(check: Check) {
  if (!check.startedAt || !check.completedAt) return null;
  const seconds = Math.round((Date.parse(check.completedAt) - Date.parse(check.startedAt)) / 1000);
  if (!Number.isFinite(seconds) || seconds < 0) return null;
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}

export function ChecksTab({ detail, act, busy }: { detail: PullDetail; act: Act; busy: boolean }) {
  const counts = checkCounts(detail.checkRuns);
  const reruns = [...new Set(detail.checkRuns.map(check => check.rerun).filter((token): token is string => !!token))];
  const order: Record<string, number> = { failed: 0, cancelled: 1, pending: 2, passed: 3, neutral: 4, skipped: 5 };
  const sorted = [...detail.checkRuns].sort((a, b) => (order[a.state] ?? 9) - (order[b.state] ?? 9) || a.name.localeCompare(b.name));
  return <div className="pr-tab-body">
    <div className="pr-section-bar">
      <span>{detail.checkRuns.length ? rt('checksSummary', counts) : rt('noChecks')}</span>
      {detail.capabilities.rerun && reruns.map((token, index) => <button key={token} type="button" disabled={busy} onClick={() => { void act('rerun', { token }); }}><RotateCw aria-hidden="true" />{rt('rerun')}{reruns.length > 1 ? ` ${index + 1}` : ''}</button>)}
    </div>
    <ul className="pr-checks">
      {sorted.map(check => { const Icon = CHECK_ICON[check.state]; return <li key={check.id} data-state={check.state}>
        <span className="pr-mark" data-checks={check.state === 'passed' ? 'passed' : check.state === 'pending' ? 'pending' : check.state === 'failed' || check.state === 'cancelled' ? 'failed' : 'none'} role="img" aria-label={rt(CHECK_LABEL[check.state])}><Icon aria-hidden="true" /></span>
        <span className="pr-check-main"><span className="pr-check-name">{check.name}</span>{check.description && <span className="pr-dim">{check.description}</span>}</span>
        {duration(check) && <span className="pr-dim">{duration(check)}</span>}
        {check.url && <a className="pr-icon" href={check.url} target="_blank" rel="noreferrer" aria-label={`${rt('logs')}: ${check.name}`} title={rt('logs')}><ExternalLink aria-hidden="true" /></a>}
      </li>; })}
    </ul>
  </div>;
}

export function CommitsTab({ detail }: { detail: PullDetail }) {
  if (!detail.commits.length) return <div className="pr-tab-body"><p className="pr-empty">{rt('noCommits')}</p></div>;
  return <div className="pr-tab-body"><ol className="pr-commits">
    {[...detail.commits].reverse().map(commit => <li key={commit.sha}>
      <span className="pr-commit-main"><span className="pr-commit-subject">{commit.message.split('\n')[0]}</span>
        <span className="pr-row-meta"><code>{commit.sha.slice(0, 7)}</code><CopyButton value={commit.sha} label={rt('copyLink')} /><span>{commit.login ?? commit.author}</span><Ago value={commit.date} /></span></span>
      {commit.url && <a className="pr-icon" href={commit.url} target="_blank" rel="noreferrer" aria-label={rt('openHost')} title={rt('openHost')}><ExternalLink aria-hidden="true" /></a>}
    </li>)}
  </ol></div>;
}

const ATTENTION: Record<string, ReviewMessage> = { tradeoff: 'attentionTradeoff', contract: 'attentionContract', product: 'attentionProduct', risk: 'attentionRisk', accessibility: 'attentionAccessibility' };

/** The review guide: a rail of numbered stops, one stop at a time. Lines in an
    excerpt can become review drafts or a question for an agent. */
export function GuideTab({ detail, guide, shownHead, act, busy, saveDraft, onAsk, onOpenFile, drafts }: {
  detail: PullDetail; guide: GuideState; shownHead: string; act: Act; busy: boolean; drafts: Draft[];
  saveDraft(input: Partial<Draft> & { kind: Draft['kind'] }): Promise<boolean>; onAsk(context: string): void; onOpenFile(path: string): void;
}) {
  const stops = guide.guide?.stops ?? [];
  const [index, setIndex] = useState(0);
  const [read, setRead] = useState<Set<number>>(() => new Set([0]));
  const stop: GuideStop | undefined = stops[Math.min(index, stops.length - 1)];
  const go = (next: number) => { setIndex(next); setRead(value => new Set(value).add(next)); };
  const running = guide.run?.state === 'running';
  const failed = guide.run?.state === 'failed' ? guide.run : null;
  const comment = (input: LineComment) => { void saveDraft({ kind: 'line', path: input.path, side: input.side === 'old' ? 'left' : 'right', line: input.endLine, startLine: input.startLine === input.endLine ? null : input.startLine, body: input.comment, headSha: shownHead }); return true; };
  const pending = useMemo(() => drafts.filter(draft => draft.kind === 'line'), [drafts]);
  if (!guide.guide) return <div className="pr-tab-body pr-guide-empty">
    {running ? <p aria-busy="true"><Hourglass aria-hidden="true" /> {rt('guideRunning')}</p> : <>
      <h3>{failed ? rt('guideFailed') : rt('guideEmpty')}</h3>
      {failed ? <ErrorMark message={failed.detail} /> : <p className="pr-dim">{rt('guideHint')}</p>}
      <button type="button" className="pr-primary" disabled={busy} onClick={() => { void act('guide'); }}>{failed ? rt('tryAgain') : rt('writeGuide')}</button>
    </>}
  </div>;
  return <div className="pr-guide">
    {(guide.stale || running || failed) && <div className="pr-banner">
      {running ? <span><Hourglass aria-hidden="true" /> {rt('guideRunning')}</span> : failed ? <><span>{rt('guideFailed')}</span><ErrorMark message={failed.detail} /></> : <span>{rt('guideStale')}</span>}
      {!running && <button type="button" disabled={busy} onClick={() => { void act('guide'); }}>{rt('rewriteGuide')}</button>}
    </div>}
    <nav className="pr-guide-rail" aria-label={rt('guide')}>
      <span className="pr-dim">{rt('read', { read: read.size, n: stops.length })}</span>
      <ol>{stops.map((item, number) => <li key={number}><button type="button" aria-current={number === index ? 'step' : undefined} data-read={read.has(number) || undefined} onClick={() => go(number)}>
        <span className="pr-stop-number">{number + 1}</span><span className="pr-stop-title">{item.title}</span>{item.attention && <span className="pr-look" title={rt(ATTENTION[item.attention.kind] ?? 'lookClosely')}><TriangleAlert aria-hidden="true" /><span className="thread-git-sr">{rt('lookClosely')}</span></span>}
      </button></li>)}</ol>
    </nav>
    {stop && <article className="pr-stop" aria-label={stop.title}>
      <header className="pr-stop-head">
        <span className="pr-dim">{rt('stopOf', { i: index + 1, n: stops.length })}</span>
        <button type="button" className="pr-icon" disabled={index === 0} onClick={() => go(index - 1)} aria-label={rt('previousStop')} title={rt('previousStop')}><ChevronLeft aria-hidden="true" /></button>
        <button type="button" className="pr-icon" disabled={index >= stops.length - 1} onClick={() => go(index + 1)} aria-label={rt('nextStop')} title={rt('nextStop')}><ChevronRight aria-hidden="true" /></button>
      </header>
      <h3>{stop.title}</h3>
      {stop.summary && <p>{stop.summary}</p>}
      {stop.attention && <aside className="pr-attention" data-kind={stop.attention.kind}><strong><TriangleAlert aria-hidden="true" />{rt(ATTENTION[stop.attention.kind] ?? 'lookClosely')}</strong><p>{stop.attention.note}</p></aside>}
      {stop.excerpts.map(excerpt => <figure key={`${excerpt.path}:${excerpt.start}`} className="pr-excerpt">
        <figcaption><button type="button" className="pr-link" onClick={() => onOpenFile(excerpt.path)} title={rt('openInCode')}>{excerpt.path}:{excerpt.start}–{excerpt.end}</button>{excerpt.caption && <span className="pr-dim"> · {excerpt.caption}</span>}</figcaption>
        <ReviewPatch patch={excerpt.patch} parsed={parsePatch(excerpt.patch)} single onSelect={() => {}} split={false} collapsed={false} area="guide" write={undefined} busy={busy} hideFind
          onLineComment={detail.capabilities.lineComments ? comment : undefined} commentLabel={rt('addToReview')}
          secondaryComment={{ label: rt('askAgent'), run: input => onAsk(`${rt('guide')} · ${stop.title}\n${input.path}:${input.startLine}-${input.endLine}\n\`\`\`\n${input.text}\n\`\`\`\n\n${input.comment}`) }}
          annotation={(path, side, line) => { const notes = stop.notes.filter(note => note.path === path && note.line === line && (side === 'new') === (excerpt.side === 'new')); const mine = pending.filter(draft => draft.path === path && draft.line === line && (draft.side === 'left' ? 'old' : 'new') === side); return notes.length || mine.length ? <>{notes.map(note => <aside key={note.title} className="pr-note"><strong>{note.title}</strong><p>{note.body}</p></aside>)}{mine.map(draft => <p key={draft.id} className="pr-draft pr-body">{draft.body}</p>)}</> : null; }} />
      </figure>)}
      {stop.files.length > 0 && <section className="pr-stop-files"><h4>{stop.excerpts.length ? rt('alsoInStop') : rt('files', { n: stop.files.length })}</h4><ul>{stop.files.map(file => <li key={file.path}><button type="button" className="pr-link" onClick={() => onOpenFile(file.path)}><PathChip path={file.path} /></button>{file.note && <span className="pr-dim">{file.note}</span>}</li>)}</ul></section>}
      {stop.questions.length > 0 && <section><h4>{rt('questions')}</h4><ul>{stop.questions.map(question => <li key={question}>{question}</li>)}</ul></section>}
      <div className="pr-stop-actions">
        <button type="button" onClick={() => onAsk(`${rt('guide')} · ${stop.title}\n\n${stop.summary}`)}><MessageCircleQuestion aria-hidden="true" />{rt('askAboutStop')}</button>
        {index < stops.length - 1 && <button type="button" className="pr-primary" onClick={() => go(index + 1)}>{rt('nextStop')}: {stops[index + 1]!.title}</button>}
      </div>
    </article>}
  </div>;
}
