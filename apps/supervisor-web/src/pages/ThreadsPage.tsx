import { useHostScope } from '../lib/hostCapabilities';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState } from 'react';
import { Link, Navigate, useNavigate, useSearchParams } from 'react-router-dom';
import { MessageSquarePlus, Plus, Settings } from 'lucide-react';
import { MatterWorkbench } from '@pockymoe/thread-ui';
import type { ThreadDto, WorkspaceDto } from '@pockymoe/shared';
import { fetchThreads, fetchWorkspaces } from '../lib/api';
import { currentNewThreadHref, currentRelayDeviceIdFromPath, currentThreadHref, currentWorkspacesHref } from '../lib/relayRoutes';
import { useAppShellNav } from '../components/AppShellNavContext';
import { RecentThreadMenu } from '../components/RecentThreadMenu';
import { TourLauncherButton } from '../components/onboarding/TourProvider';
import { useWorkbenchNavigation } from './useWorkbenchNavigation';
import { useThreadListPolling } from './useThreadListPolling';
import { appUrl } from '../lib/webBase';
import { workbenchUrls } from '../lib/workbenchUrls';

// Legacy workspace-list URLs now resolve directly into the conversation shell.
export function ThreadsPage() {
  useI18n();
  const hostScope = useHostScope();
  const [params] = useSearchParams();
  const workspaceId = params.get('workspaceId');
  const deviceId = currentRelayDeviceIdFromPath();
  const navigate = useNavigate();
  const shell = useAppShellNav();
  const [threads, setThreads] = useState<ThreadDto[]>([]);
  const [workspace, setWorkspace] = useState<WorkspaceDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const navigation = useWorkbenchNavigation(null, threads, deviceId);
  // The empty-workspace page only waits for a first thread to open it.
  useThreadListPolling({ enabled: !loading && !!workspace, setThreads, intervalMs: 30_000 });
  useEffect(() => {
    let active = true;
    setLoading(true); setError(null); setWorkspace(null); setThreads([]);
    if (!workspaceId && !hostScope) return;
    void Promise.all([fetchThreads(Boolean(hostScope)), fetchWorkspaces()]).then(([items, workspaces]) => {
      if (!active) return;
      const found = workspaces.find(value => value.id === workspaceId) ?? (hostScope ? workspaces[0] : undefined);
      if (!found && hostScope) { setThreads(items); return; }
      if (!found) throw new Error(translate("workbench.workspaceNotFound"));
      setWorkspace(found); setThreads(items);
    }).catch(caught => {
      if (active) setError(caught instanceof Error ? caught.message : translate("workbench.unableToOpenWorkspace"));
    }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [workspaceId, deviceId, retry, hostScope]);
  if (!workspaceId && !hostScope) return <Navigate to={currentWorkspacesHref()} replace />;
  const next = threads.filter(thread => (!hostScope || workspaceId) ? thread.workspaceId === workspaceId : !thread.parentThreadId)
    .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt) || a.id.localeCompare(b.id))[0];
  if (!loading && !error && next) return <Navigate to={currentThreadHref(next.id)} replace />;
  const createHref = currentNewThreadHref(workspaceId ?? workspace?.id);
  return <div className="thread-ui-shell thread-ui-viewport-constrained h-full" data-theme-effective={shell?.effectiveTheme ?? 'light'} data-theme-mode={shell?.themeMode ?? 'system'}>
    <MatterWorkbench
      title={workspace?.label ?? translate("workbench.workspace")} homeHref={hostScope ? undefined : appUrl(currentWorkspacesHref())}
      options={workbenchUrls({ ...navigation, emptyWorkspace: true, workspacePath: workspace?.absPath ?? '', activeView: 'chat', terminalEnabled: false, onViewChange: () => {}, onSearch: () => {}, onNavigate: navigate,
        renderThreadMenu: (thread, context) => <RecentThreadMenu thread={thread} context={context} currentKey={navigation.currentKey} onFavorite={navigation.onToggleThreadFavorite} onRenamed={navigation.onThreadRenamed} onRemoved={navigation.onThreadRemoved} onNavigate={navigate} /> })}
      deviceMonitor={hostScope ? null : <TourLauncherButton />}
      settings={!hostScope && <button aria-label={translate("workbench.openSettings")} onClick={shell?.openSettings}><Settings data-icon="settings" /></button>}
      newThread={!loading && !error && (!hostScope || workspace) && <Link to={createHref} aria-label={translate("workbench.newThread_02057e")}><Plus data-icon="new-thread" /></Link>}
      actions={null} threadMenu={null} connection={null} explorer={null} revealExplorer={0}
    >
      <div className="flex h-full flex-col items-center justify-center gap-4 px-6 text-center">
        {loading ? <p role="status" className="workspace-opening">{translate("workbench.openingWorkspace")}</p> : error ? <><p role="alert">{error}</p><button className="relay-button-secondary rounded-lg px-4 py-2" onClick={() => setRetry(value => value + 1)}>{translate("workbench.retry")}</button></> : <>
          <MessageSquarePlus size={32} className="text-[var(--theme-fg-muted)]" />
          <h1 className="text-xl font-semibold">{translate("workbench.startAThreadIn")} {workspace?.label}</h1>
          <p className="text-sm text-[var(--theme-fg-muted)]">{translate("workbench.yourConversationsInThisWorkspaceWillAppear")}</p>
          <Link to={createHref} className="relay-button-primary inline-flex items-center gap-2 rounded-lg px-5 py-3 text-sm font-semibold"><Plus size={18} />{translate("workbench.createThread")}</Link>
        </>}
      </div>
    </MatterWorkbench>
  </div>;
}
