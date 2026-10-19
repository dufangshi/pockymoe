import { useHostScope } from '../lib/hostCapabilities';
import { narrationQuery } from '../lib/transcriptPreferences';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, Copy, ExternalLink, MoreHorizontal, Pencil, Star, Trash2, TriangleAlert } from 'lucide-react';
import type { ThreadDetailDto } from '@pockymoe/shared';
import { copyText, useMenuKeyboard, type ThreadMenuContext, type WorkbenchThread } from '@pockymoe/thread-ui';
import { fetchRelayAccess, isInvisibleThreadError, request } from '../lib/api';
import { RenameDialog } from './RenameDialog';
import { ConfirmDialog } from './ConfirmDialog';
import { threadsHref } from '../lib/relayRoutes';

/** A thread's menu. With `context` it opens where the thread was right-clicked
    (a tab or a row), has no ⋯ button of its own, and reports when it is done. */
export function RecentThreadMenu({ thread, onFavorite, onRenamed, onRemoved, onNavigate, currentKey, context }: {
  thread: WorkbenchThread;
  context?: ThreadMenuContext | undefined;
  onFavorite: (key: string) => Promise<void>;
  onRenamed: (key: string, title: string) => Promise<void>;
  onRemoved: (key: string) => Promise<void>;
  currentKey: string;
  onNavigate: (href: string, options?: { replace?: boolean }) => void;
}) {
  useI18n();
  const hostScope = useHostScope();
  const place = (left: number, top: number) => ({ left: Math.min(Math.max(8, left), innerWidth - 268), top: Math.max(8, Math.min(top, innerHeight - 340)) });
  const [position, setPosition] = useState<{ top: number; left: number } | null>(() => context ? place(context.x, context.y) : null);
  const [detail, setDetail] = useState<ThreadDetailDto | null>(null);
  const [owner, setOwner] = useState(false);
  const [notice, setNotice] = useState('');
  const [renaming, setRenaming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [title, setTitle] = useState(thread.title);
  const [busy, setBusy] = useState(false);
  // Copy feedback shows on the item that copied, as in every other menu.
  const [copied, setCopied] = useState<{ key: string; state: 'copied' | 'failed' } | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const split = thread.key.indexOf(':');
  const device = thread.key.slice(0, split);
  const id = thread.key.slice(split + 1);
  const base = `${device === 'local' ? '' : `relay/devices/${encodeURIComponent(device)}/`}api/threads/${encodeURIComponent(id)}`;
  useEffect(() => {
    if (!position) return;
    let active = true;
    setNotice(''); setDetail(null); setOwner(false); setCopied(null);
    void Promise.all([
      request<ThreadDetailDto>(`${base}?view=summary&limit=1&compact=1${narrationQuery()}`),
      device === 'local' ? Promise.resolve({ kind: 'owner' }) : fetchRelayAccess({ deviceId: device, threadId: id }),
    ]).then(([value, access]) => { if (active) { setDetail(value); setOwner(access.kind === 'owner'); } })
      .catch(error => { if (active && hostScope && isInvisibleThreadError(error)) { void onRemoved(thread.key); setPosition(null); return; } if (active) setNotice(translate("workbench.deviceUnavailableYouCanStillManageThis")); });
    const close = (event: Event) => {
      if (event.type === 'keydown' && (event as KeyboardEvent).key !== 'Escape') return;
      if (event.type === 'pointerdown' && (menu.current?.contains(event.target as Node) || trigger.current?.contains(event.target as Node))) return;
      setPosition(null);
      if (event.type === 'keydown') (trigger.current ?? context?.returnFocus)?.focus();
    };
    // The menu belongs to its row: scrolling the list away closes it.
    const scroll = (event: Event) => { if (!menu.current?.contains(event.target as Node)) setPosition(null); };
    document.addEventListener('pointerdown', close);
    document.addEventListener('keydown', close);
    window.addEventListener('resize', close);
    window.addEventListener('scroll', scroll, true);
    menu.current?.querySelector('button')?.focus();
    return () => { active = false; document.removeEventListener('pointerdown', close); document.removeEventListener('keydown', close); window.removeEventListener('resize', close); window.removeEventListener('scroll', scroll, true); };
  }, [position, base, device, id]);
  useMenuKeyboard(menu, Boolean(position), (restore) => { setPosition(null); if (restore) (trigger.current ?? context?.returnFocus)?.focus(); }, 'button:not([disabled])');
  // Opened from a right-click, it is done once its menu and any window it opened close.
  const dismiss = useRef(context?.onClose);
  dismiss.current = context?.onClose;
  useEffect(() => {
    if (context && !position && !renaming && !deleting) dismiss.current?.();
  }, [Boolean(context), position, renaming, deleting]);
  const copyItem = (key: string, label: string, text: string | null | undefined) => {
    const state = copied?.key === key ? copied.state : null;
    return (
      <button data-copy-state={state ?? undefined} disabled={!text} onClick={async () => {
        if (!text) return;
        const ok = await copyText(text);
        setCopied({ key, state: ok ? 'copied' : 'failed' });
        if (ok) window.setTimeout(() => { setCopied(null); setPosition(null); }, 700);
      }}>
        {state === 'copied' ? <Check size={15} /> : state === 'failed' ? <TriangleAlert size={15} /> : <Copy data-icon="copy" size={15} />}
        {state ? translate(state === 'copied' ? 'common.copied' : 'common.copyFailed') : label}
      </button>
    );
  };
  return <>
    {!context && <button ref={trigger} className="recent-thread-actions" aria-label={translate("workbench.actionsFor", { value1: thread.title })} aria-expanded={!!position} aria-haspopup="dialog" onClick={() => {
      if (position) { setPosition(null); return; }
      const box = trigger.current!.getBoundingClientRect();
      setPosition(place(box.right - 260, box.bottom + 6));
    }}><MoreHorizontal data-icon="more" size={16} /></button>}
    {position && createPortal(<div ref={menu} role="dialog" aria-label={translate("workbench.threadActions", { value1: thread.title })} className="recent-thread-menu" style={position}>
      <p className="recent-thread-menu-title">{thread.title}</p>
      <button disabled={busy} onClick={async () => { setBusy(true); try { await onFavorite(thread.key); setPosition(null); } finally { setBusy(false); } }}><Star data-icon="shortcut" size={15} fill={thread.favorite ? 'currentColor' : 'none'} />{thread.favorite ? translate("workbench.unstarThread") : translate("workbench.starThread")}</button>
      <button onClick={() => { setPosition(null); onNavigate(thread.href); }}><ExternalLink data-icon="open" size={15} />{translate("workbench.openThread")}</button>
      {copyItem('pockymoe', translate("workbench.copyPockymoeSessionID"), id)}
      {copyItem('harness', translate("workbench.copyHarnessSessionID"), detail?.thread.providerSessionId)}
      {detail?.thread.providerSessionId && (detail.thread.agentId === 'codex' || detail.thread.provider === 'codex') && copyItem('codex', translate("workbench.copyCodexDeeplink"), `codex://threads/${encodeURIComponent(detail.thread.providerSessionId)}`)}
      {owner && <button onClick={() => { setTitle(detail?.thread.title ?? thread.title); setNotice(''); setPosition(null); setRenaming(true); }}><Pencil data-icon="rename" size={15} />{translate("workbench.renameThread")}</button>}
      {owner && <button className="recent-thread-delete" onClick={() => { setNotice(''); setPosition(null); setDeleting(true); }}><Trash2 data-icon="delete" size={15} />{translate("workbench.deleteThread")}</button>}
      {notice && <p role="status">{notice}</p>}
    </div>, document.body)}
    <RenameDialog open={renaming} title={translate("workbench.renameThread")} label={translate("workbench.threadTitle")} value={title} busy={busy} error={notice} onChange={setTitle} onCancel={() => setRenaming(false)} onSubmit={async () => {
      setBusy(true); setNotice('');
      try { await request(base, { method: 'PATCH', body: JSON.stringify({ title: title.trim() }) }); await onRenamed(thread.key, title.trim()); setRenaming(false); }
      catch (error) { setNotice(error instanceof Error ? error.message : translate("workbench.couldNotRenameThread")); }
      finally { setBusy(false); }
    }} />
    <ConfirmDialog open={deleting} title={translate("workbench.deleteThread_c8f2c4")} description={translate("workbench.deleteAndItsPockymoeHistoryThis", { value1: thread.title })} busy={busy} error={notice} onCancel={() => setDeleting(false)} onConfirm={async () => {
      setBusy(true); setNotice('');
      try {
        await request(base, { method: 'DELETE' });
        if (thread.key === currentKey) onNavigate(threadsHref(detail?.thread.workspaceId, device === 'local' ? null : device), { replace: true });
        await onRemoved(thread.key);
        setDeleting(false);
      } catch (error) { setNotice(error instanceof Error ? error.message : translate("workbench.couldNotDeleteThread")); }
      finally { setBusy(false); }
    }} />
  </>;
}
