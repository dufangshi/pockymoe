import { useCallback, useEffect, useState } from 'react';
import { GitPullRequest, RefreshCw } from 'lucide-react';
import { ErrorMark } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { FormDialog } from '../../FormDialog';
import { request } from '../../../lib/api';
import { sourceAction, sourceRoot, type ItemLink } from './api';
import { prText as t } from './text';
import './pr.css';

/** Owner-only thread details; the Git panel supplies editing and host actions. */
export function ThreadSourceLinksControl({ threadId, deviceId, refreshKey }: {
  threadId: string; deviceId?: string | null | undefined; refreshKey?: string | undefined;
}) {
  useI18n();
  const [open, setOpen] = useState(false);
  const [links, setLinks] = useState<ItemLink[]>([]);
  const [watch, setWatch] = useState<{ active?: boolean; stoppedReason?: string }>({});
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const reload = useCallback(async () => {
    const [items, watching] = await Promise.all([
      request<{ links: ItemLink[] }>(`${sourceRoot(deviceId)}/threads/${encodeURIComponent(threadId)}/pull-requests`),
      sourceAction<{ state: typeof watch }>(threadId, 'watchStatus', {}, deviceId),
    ]);
    return { links: items.links, watch: watching.state };
  }, [deviceId, threadId]);
  useEffect(() => {
    let alive = true;
    // Coalesce transcript updates; a PR link often arrives with several events.
    const timer = window.setTimeout(() => { void reload().then(value => {
      if (alive) { setLinks(value.links); setWatch(value.watch); }
    }).catch(caught => { if (alive) setError(String(caught)); }); }, 300);
    return () => { alive = false; window.clearTimeout(timer); };
  }, [reload, refreshKey]);
  const refresh = async () => {
    setBusy(true); setError('');
    try { const value = await reload(); setLinks(value.links); setWatch(value.watch); }
    catch (caught) { setError(String(caught)); }
    finally { setBusy(false); }
  };
  return <>
    <button type="button" aria-label={t('linkedItems')} title={t('linkedItems')} onClick={() => { setOpen(true); void refresh(); }}>
      <GitPullRequest size={15} /><span>{t('linkedItems')}{watch.active ? ` · ${t('watching')}` : links.length ? ` · ${links.length}` : ''}</span>
    </button>
    {open && <FormDialog secondary title={t('linkedItems')} onClose={() => setOpen(false)}>
      <div className="pr-panel">
        <button type="button" disabled={busy} aria-label={t('refresh')} onClick={() => { void refresh(); }}><RefreshCw size={15} /></button>
        {error && <ErrorMark message={error} />}
        {watch.active && <p>{t('watching')}</p>}
        {!watch.active && watch.stoppedReason && <p>{watch.stoppedReason}</p>}
        {!links.length && <p>{t('empty')}</p>}
        <ul>{links.map(link => <li key={link.url}><a href={link.url} target="_blank" rel="noreferrer">{link.repository.owner}/{link.repository.name} · {link.kind === 'pr' ? 'PR' : t('issues')} #{link.number}</a></li>)}</ul>
        {watch.active && <button type="button" disabled={busy} onClick={() => { setBusy(true); void sourceAction(threadId, 'unwatch', {}, deviceId).then(refresh).catch(caught => setError(String(caught))).finally(() => setBusy(false)); }}>{t('unwatch')}</button>}
      </div>
    </FormDialog>}
  </>;
}
