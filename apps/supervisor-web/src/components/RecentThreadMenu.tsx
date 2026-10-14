import { useHostScope } from '../lib/hostCapabilities';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Copy, ExternalLink, MoreHorizontal, Pencil, Star, Trash2 } from 'lucide-react';
import type { ThreadDetailDto } from '@pockymoe/shared';
import type { WorkbenchThread } from '@pockymoe/thread-ui';
import { fetchRelayAccess, isInvisibleThreadError, request } from '../lib/api';
import { RenameDialog } from './RenameDialog';
import { ConfirmDialog } from './ConfirmDialog';
import { threadsHref } from '../lib/relayRoutes';

export function RecentThreadMenu({ thread, onFavorite, onRenamed, onRemoved, onNavigate, currentKey }: {
  thread: WorkbenchThread;
  onFavorite: (key: string) => Promise<void>;
  onRenamed: (key: string, title: string) => Promise<void>;
  onRemoved: (key: string) => Promise<void>;
  currentKey: string;
  onNavigate: (href: string, options?: { replace?: boolean }) => void;
}) {
  useI18n();
  const hostScope = useHostScope();
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null);
  const [detail, setDetail] = useState<ThreadDetailDto | null>(null);
  const [owner, setOwner] = useState(false);
  const [notice, setNotice] = useState('');
  const [renaming, setRenaming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [title, setTitle] = useState(thread.title);
  const [busy, setBusy] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const split = thread.key.indexOf(':');
  const device = thread.key.slice(0, split);
  const id = thread.key.slice(split + 1);
  const base = `${device === 'local' ? '' : `relay/devices/${encodeURIComponent(device)}/`}api/threads/${encodeURIComponent(id)}`;
  useEffect(() => {
    if (!position) return;
    let active = true;
    setNotice(''); setDetail(null); setOwner(false);
    void Promise.all([
      request<ThreadDetailDto>(`${base}?view=summary&limit=1&compact=1`),
      device === 'local' ? Promise.resolve({ kind: 'owner' }) : fetchRelayAccess({ deviceId: device, threadId: id }),
    ]).then(([value, access]) => { if (active) { setDetail(value); setOwner(access.kind === 'owner'); } })
      .catch(error => { if (active && hostScope && isInvisibleThreadError(error)) { void onRemoved(thread.key); setPosition(null); return; } if (active) setNotice(translate("workbench.deviceUnavailableYouCanStillManageThis")); });
    const close = (event: Event) => {
      if (event.type === 'keydown' && (event as KeyboardEvent).key !== 'Escape') return;
      if (event.type === 'pointerdown' && (menu.current?.contains(event.target as Node) || trigger.current?.contains(event.target as Node))) return;
      setPosition(null);
      if (event.type === 'keydown') trigger.current?.focus();
    };
    document.addEventListener('pointerdown', close);
    document.addEventListener('keydown', close);
    window.addEventListener('resize', close);
    menu.current?.querySelector('button')?.focus();
    return () => { active = false; document.removeEventListener('pointerdown', close); document.removeEventListener('keydown', close); window.removeEventListener('resize', close); };
  }, [position, base, device, id]);
  const copy = async (text: string) => {
    try { await navigator.clipboard.writeText(text); setNotice(translate("workbench.copied_dd2ce2")); }
    catch { setNotice(translate("workbench.couldNotCopyCheckBrowserClipboardPermissions")); }
  };
  return <>
    <button ref={trigger} className="recent-thread-actions" aria-label={translate("workbench.actionsFor", { value1: thread.title })} aria-expanded={!!position} aria-haspopup="dialog" onClick={() => {
      if (position) { setPosition(null); return; }
      const box = trigger.current!.getBoundingClientRect();
      setPosition({ left: Math.min(Math.max(8, box.right - 260), innerWidth - 268), top: Math.max(8, Math.min(box.bottom + 6, innerHeight - 340)) });
    }}><MoreHorizontal size={16} /></button>
    {position && createPortal(<div ref={menu} role="dialog" aria-label={translate("workbench.threadActions", { value1: thread.title })} className="recent-thread-menu" style={position}>
      <p className="recent-thread-menu-title">{thread.title}</p>
      <button disabled={busy} onClick={async () => { setBusy(true); try { await onFavorite(thread.key); setPosition(null); } finally { setBusy(false); } }}><Star size={15} fill={thread.favorite ? 'currentColor' : 'none'} />{thread.favorite ? translate("workbench.unstarThread") : translate("workbench.starThread")}</button>
      <button onClick={() => { setPosition(null); onNavigate(thread.href); }}><ExternalLink size={15} />{translate("workbench.openThread")}</button>
      <button onClick={() => void copy(id)}><Copy size={15} />{translate("workbench.copyPockymoeSessionID")}</button>
      <button disabled={!detail?.thread.providerSessionId} onClick={() => void copy(detail!.thread.providerSessionId!)}><Copy size={15} />{translate("workbench.copyHarnessSessionID")}</button>
      {detail?.thread.providerSessionId && (detail.thread.agentId === 'codex' || detail.thread.provider === 'codex') && <button onClick={() => void copy(`codex://threads/${encodeURIComponent(detail.thread.providerSessionId!)}`)}><Copy size={15} />{translate("workbench.copyCodexDeeplink")}</button>}
      {owner && <button onClick={() => { setTitle(detail?.thread.title ?? thread.title); setNotice(''); setPosition(null); setRenaming(true); }}><Pencil size={15} />{translate("workbench.renameThread")}</button>}
      {owner && <button className="recent-thread-delete" onClick={() => { setNotice(''); setPosition(null); setDeleting(true); }}><Trash2 size={15} />{translate("workbench.deleteThread")}</button>}
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
