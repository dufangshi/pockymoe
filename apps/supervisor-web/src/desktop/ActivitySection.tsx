import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { memo, useCallback, useMemo, useRef, useState } from 'react';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { StatusMark, type WorkbenchThread } from '@pockymoe/thread-ui';
import { RecentThreadMenu } from '../components/RecentThreadMenu';
import { workbenchThreadStatus } from '../pages/useWorkbenchNavigation';
import type { ActivityItem } from './host';
import { useDesktopSelector, useDesktopStore, mergeRows } from './data';
import { buildActivityItems } from './data/activity';
import { threadHref } from '../lib/relayRoutes';
import { appUrl } from '../lib/webBase';
import type { NavigationActions } from './context';
import { useEffect } from 'react';
import { formatRunTime } from './duration';

function RunningTime({ since }: { since: string }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(timer); }, []);
  return <span className="desktop-activity-time">{formatRunTime(now - Date.parse(since))}</span>;
}
const ActivityRow = memo(function ActivityRow({ item, favorite, onOpen, menu }: { item: ActivityItem; favorite: boolean; onOpen(key: string): void; menu(key: string, favorite: boolean): React.ReactNode }) {
  useI18n();
  const state = item.state === 'needsInput' ? 'waiting' : item.state === 'completed' ? 'unread' : item.state;
  return <div className="matter-thread-row desktop-activity-row" data-activity-key={item.key}>
    <a href={appUrl(threadHref(item.threadId, item.deviceId))} onClick={event => { if (!event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey && event.button === 0) { event.preventDefault(); onOpen(item.key); } }} title={item.title}>
      <StatusMark status={state} /><span className="matter-thread-copy"><span>{item.title}</span><small>{item.deviceName} · {item.workspaceLabel}</small></span>
      {item.state === 'running' ? <RunningTime since={item.since} /> : item.request ? <span className="desktop-activity-request">{item.request.kind === 'approval' ? translate('desktopShell.approval') : translate('desktopShell.input')}</span> : null}
    </a>{menu(item.key, favorite)}
  </div>;
});
/** Desktop-only: the persistent account index also drives native alerts. Ordinary
 * browsers keep their existing sidebar (docs/ui-principles.md, desktop exception). */
export function ActivitySection({ navigation, onOpen }: { navigation: NavigationActions; onOpen(key: string): void }) {
  useI18n();
  const store = useDesktopStore();
  const index = useDesktopSelector(index => index);
  const previous = useRef<readonly ActivityItem[]>([]);
  const items = useMemo(() => { previous.current = mergeRows(previous.current, buildActivityItems(index), row => row.key); return previous.current; }, [index]);
  const latest = useRef(navigation); latest.current = navigation;
  const [open, setOpen] = useState(() => {
    try { return JSON.parse(localStorage.getItem('pockymoe.sidebar-sections.v1') ?? '{}').activity !== false; } catch { return true; }
  });
  const renderMenu = useCallback((key: string, favorite: boolean) => {
    const state = store.getSnapshot(); const thread = state.threads.find(row => row.key === key); if (!thread) return null;
    const workspace = state.workspaces.find(row => row.key === `${thread.deviceId}:${thread.workspaceId}`);
    const row: WorkbenchThread = { key, title: thread.title, href: appUrl(threadHref(thread.id, thread.deviceId)), subtitle: workspace?.label ?? '', status: workbenchThreadStatus(thread, thread.lastViewedAt), favorite,
      ...(thread.parentThreadId ? { parentKey: `${thread.deviceId}:${thread.parentThreadId}` } : {}), ...(thread.rootThreadId ? { rootKey: `${thread.deviceId}:${thread.rootThreadId}` } : {}) };
    return <RecentThreadMenu thread={row} currentKey={latest.current.currentKey} onNavigate={href => onOpen(href)}
      onFavorite={target => latest.current.onToggleThreadFavorite(target)}
      onRenamed={async (target, title) => { await latest.current.onThreadRenamed(target, title); await store.refreshDevice(thread.deviceId); }}
      onRemoved={async target => { await latest.current.onThreadRemoved(target); await store.refreshDevice(thread.deviceId); }} />;
  }, [onOpen, store]);
  const favorites = new Set(navigation.threads?.filter(thread => thread.favorite).map(thread => thread.key));
  const groups = [
    { id: 'attention', label: translate('desktopShell.needsYou'), items: items.filter(item => ['needsInput', 'failed', 'interrupted'].includes(item.state)) },
    { id: 'running', label: translate('desktopShell.running'), items: items.filter(item => item.state === 'running') },
    { id: 'completed', label: translate('desktopShell.justCompleted'), items: items.filter(item => item.state === 'completed') },
  ];
  return <section className="desktop-activity" aria-label={translate('desktopShell.activity')}>
    <button className="matter-section-heading" aria-expanded={open} onClick={() => setOpen(current => {
      const next = !current;
      try { localStorage.setItem('pockymoe.sidebar-sections.v1', JSON.stringify({ ...JSON.parse(localStorage.getItem('pockymoe.sidebar-sections.v1') ?? '{}'), activity: next })); } catch { /* The fold still holds in this visit. */ }
      return next;
    })}>{open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}<span>{translate('desktopShell.activity')}</span><span className="matter-section-count">{new Set(items.map(item => item.rootThreadKey ?? item.key)).size}</span></button>
    {open && groups.map(group => {
      const families = new Map<string, ActivityItem[]>();
      for (const item of group.items) { const key = item.rootThreadKey ?? item.key; families.set(key, [...(families.get(key) ?? []), item]); }
      return <div key={group.id} className="desktop-activity-group" data-activity-group={group.id}><div className="desktop-activity-heading">{group.label}<span>{families.size}</span></div>{[...families].map(([key, members]) => {
        const item = members[0]!;
        const root = index.threads.find(thread => thread.key === key);
        if (!root || (members.length === 1 && key === item.key)) return <ActivityRow key={item.key} item={item} favorite={favorites.has(item.key)} onOpen={onOpen} menu={renderMenu} />;
        // Agent activity always retains its conversation; a family appears once
        // in a group, with its individual requests folded underneath.
        return <details key={key} className="matter-agent-threads desktop-activity-family"><summary><ChevronRight size={11} /><StatusMark status={item.state === 'needsInput' ? 'waiting' : item.state === 'running' ? 'agents-running' : item.state === 'completed' ? 'unread' : item.state} /><span>{root.title}</span><span>{members.length}</span></summary>{members.map(member => <ActivityRow key={member.key} item={member} favorite={favorites.has(member.key)} onOpen={onOpen} menu={renderMenu} />)}</details>;
      })}</div>;
    })}
  </section>;
}
