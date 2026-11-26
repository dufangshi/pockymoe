import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { CircleCheck, CircleDashed, CircleDot, CircleX, GitMerge, GitPullRequest, GitPullRequestClosed, GitPullRequestDraft, MessageSquareWarning, ThumbsUp } from 'lucide-react';
import { formatShortTimestamp, useMenuKeyboard } from '@pockymoe/thread-ui';
import { getLocale } from '@pockymoe/thread-ui/i18n';
import type { ChecksState, PullState, ReviewDecision } from './reviewApi';
import { rt } from './reviewText';

/** A PR's state by shape: open, draft, merged and closed differ without colour. */
export function PullStateMark({ state, draft }: { state: PullState; draft: boolean }) {
  const label = state === 'merged' ? rt('merged') : state === 'closed' ? rt('closed') : draft ? rt('draftState') : rt('open');
  const Icon = state === 'merged' ? GitMerge : state === 'closed' ? GitPullRequestClosed : draft ? GitPullRequestDraft : GitPullRequest;
  return <span className="pr-mark" data-state={draft && state === 'open' ? 'draft' : state} role="img" aria-label={label} title={label}><Icon aria-hidden="true" /></span>;
}

/** CI state: check, cross, open ring; nothing when the host has no checks. */
export function ChecksMark({ checks }: { checks: ChecksState | null }) {
  if (!checks || checks === 'none') return null;
  const label = checks === 'passed' ? rt('passing') : checks === 'failed' ? rt('failing') : rt('pending');
  const Icon = checks === 'passed' ? CircleCheck : checks === 'failed' ? CircleX : CircleDashed;
  return <span className="pr-mark" data-checks={checks} role="img" aria-label={`${rt('checks')}: ${label}`} title={`${rt('checks')}: ${label}`}><Icon aria-hidden="true" /></span>;
}

export function DecisionMark({ decision }: { decision: ReviewDecision | null }) {
  if (!decision) return null;
  const label = rt(decision === 'approved' ? 'approved' : decision === 'changesRequested' ? 'changesRequested' : 'reviewRequired');
  const Icon = decision === 'approved' ? ThumbsUp : decision === 'changesRequested' ? MessageSquareWarning : CircleDot;
  return <span className="pr-mark" data-decision={decision} role="img" aria-label={label} title={label}><Icon aria-hidden="true" /></span>;
}

/** Relative time ("3h", "2d") with the full timestamp on hover and for readers. */
export function Ago({ value }: { value: string }) {
  const date = Date.parse(value);
  if (Number.isNaN(date)) return null;
  const minutes = Math.max(0, Math.round((Date.now() - date) / 60_000));
  const format = new Intl.RelativeTimeFormat(getLocale(), { numeric: 'auto', style: 'narrow' });
  const text = minutes < 60 ? format.format(-minutes, 'minute') : minutes < 1440 ? format.format(-Math.round(minutes / 60), 'hour') : minutes < 43_200 ? format.format(-Math.round(minutes / 1440), 'day') : new Date(date).toLocaleDateString(getLocale(), { month: 'short', day: 'numeric', year: minutes > 525_600 ? 'numeric' : undefined });
  return <time dateTime={value} title={formatShortTimestamp(value)}>{text}</time>;
}

export function Stat({ additions, deletions }: { additions: number | null; deletions: number | null }) {
  if (!additions && !deletions) return null;
  return <span className="thread-git-stat">{additions ? <ins>+{additions}</ins> : null}{deletions ? <del>−{deletions}</del> : null}</span>;
}

export interface MenuItem { label: string; icon?: ReactNode; onSelect?: () => void; danger?: boolean; disabled?: boolean; checked?: boolean; separator?: boolean; href?: string }

/** A ⋯-style menu: role=menu at the page root, keyboard per §3, closes on
    outside press, scroll of the page or resize. */
export function MenuButton({ label, icon, items, className = '', text }: { label: string; icon: ReactNode; items: MenuItem[]; className?: string; text?: ReactNode }) {
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  useMenuKeyboard(menu, Boolean(position), restore => { setPosition(null); if (restore) trigger.current?.focus(); });
  useEffect(() => {
    if (!position) return;
    const outside = (event: PointerEvent) => { if (!menu.current?.contains(event.target as Node) && !trigger.current?.contains(event.target as Node)) setPosition(null); };
    const scroll = (event: Event) => { if (!menu.current?.contains(event.target as Node)) setPosition(null); };
    const resize = () => setPosition(null);
    document.addEventListener('pointerdown', outside);
    window.addEventListener('scroll', scroll, true);
    window.addEventListener('resize', resize);
    return () => { document.removeEventListener('pointerdown', outside); window.removeEventListener('scroll', scroll, true); window.removeEventListener('resize', resize); };
  }, [position]);
  useLayoutEffect(() => {
    if (!position || !menu.current) return;
    const box = menu.current.getBoundingClientRect();
    const viewport = window.visualViewport;
    const width = viewport?.width ?? innerWidth, height = viewport?.height ?? innerHeight;
    const left = Math.max(8, Math.min(position.left, width - box.width - 8));
    const top = position.top + box.height > height - 8 ? Math.max(8, position.top - box.height - 44) : position.top;
    if (left !== position.left || top !== position.top) setPosition({ left, top });
  }, [position]);
  return <>
    <button ref={trigger} type="button" className={`pr-icon ${className}`} aria-label={label} title={label} aria-haspopup="menu" aria-expanded={Boolean(position)}
      onClick={() => { if (position) { setPosition(null); return; } const box = trigger.current!.getBoundingClientRect(); setPosition({ left: box.right - 240, top: box.bottom + 4 }); }}>{icon}{text}</button>
    {position && createPortal(<div ref={menu} role="menu" aria-label={label} className="pr-menu" style={{ top: position.top, left: position.left }}>
      {items.map((item, index) => item.separator ? <div key={index} role="separator" /> : item.href
        ? <a key={index} role="menuitem" href={item.href} target="_blank" rel="noreferrer" onClick={() => setPosition(null)}>{item.icon}{item.label}</a>
        : <button key={index} type="button" role={item.checked === undefined ? 'menuitem' : 'menuitemradio'} aria-checked={item.checked} data-danger={item.danger || undefined} disabled={item.disabled}
          onClick={() => { setPosition(null); item.onSelect?.(); }}>{item.icon}{item.label}</button>)}
    </div>, document.body)}
  </>;
}

/** Host Markdown (GitHub-flavoured); raw HTML stays text, links open beside. */
export function Body({ text, empty }: { text: string; empty?: string }) {
  if (!text.trim()) return empty ? <p className="pr-dim">{empty}</p> : null;
  return <div className="pr-body"><ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: ({ href, children }) => <a href={href} target="_blank" rel="noreferrer">{children}</a> }}>{text}</ReactMarkdown></div>;
}
