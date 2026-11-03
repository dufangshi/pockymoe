import { RelayUserMenu } from '../components/RelayUserMenu';
import { relayModeActive } from '../lib/api';
import { useSyncExternalStore, useMemo, type ReactNode } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { FileInput, LayoutDashboard, Plus, Settings } from 'lucide-react';
import { MatterWorkbench, Select } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useDesktopSurface, useDesktopWorkbench } from './context';
import { useDesktopSelector } from './data';
import { useWorkbenchNavigation } from '../pages/useWorkbenchNavigation';
import { appUrl } from '../lib/webBase';
import { workbenchUrls } from '../lib/workbenchUrls';
import { currentRelayScopedPath, workspacesHref } from '../lib/relayRoutes';
import { useAppShellNav } from '../components/AppShellNavContext';
import { RecentThreadMenu } from '../components/RecentThreadMenu';
import { DeviceStateMark } from '../pages/relayAdmin/ui';

const desktopWidth = () => window.matchMedia('(min-width: 640px)').matches;
const subscribeWidth = (changed: () => void) => { const media = window.matchMedia('(min-width: 640px)'); media.addEventListener('change', changed); return () => media.removeEventListener('change', changed); };
export function useDesktopWorkspaceLayout() {
  const surface = useDesktopSurface();
  const wide = useSyncExternalStore(subscribeWidth, desktopWidth);
  return Boolean(surface) && wide;
}

/** Desktop navigation lives in the sidebar. Phones and plain browsers retain
 * ProductHeader (§9 exception in docs/ui-principles.md). */
export function WorkspacesFrame({ deviceId, children, sharedOnly }: { deviceId: string | null; children: ReactNode; sharedOnly: boolean }) {
  useI18n();
  const index = useDesktopSelector(index => index);
  const device = index.devices.find(device => device.id === deviceId);
  const threads = useMemo(() => index.threads.filter(thread => thread.deviceId === deviceId), [deviceId, index.threads]);
  const navigation = useWorkbenchNavigation(null, threads, deviceId, undefined, { sharesDeviceThreads: true });
  const desktop = useDesktopWorkbench(navigation);
  const surface = useDesktopSurface();
  const shell = useAppShellNav(); const navigate = useNavigate();
  const actions = <div className="desktop-workspace-actions">
    {!sharedOnly && <Link to={currentRelayScopedPath('/workspaces/new')}><Plus size={16} aria-hidden />{translate('files.addWorkspace')}</Link>}
    {!sharedOnly && <Link to={currentRelayScopedPath('/threads/import')}><FileInput size={16} aria-hidden />{translate('files.importSession')}</Link>}
    <button onClick={surface?.openMap}><LayoutDashboard size={16} aria-hidden />{translate('workbench.threadMap')}</button>
  </div>;
  return <div className="thread-ui-shell thread-ui-viewport-constrained desktop-workspaces-shell" data-theme-effective={shell?.effectiveTheme} data-theme-mode={shell?.themeMode}>
    <MatterWorkbench homeHref={appUrl('/')} title={translate('files.workspaces')} options={workbenchUrls({ ...navigation, ...desktop, workspaceControlsEnabled: false, emptyWorkspace: true, activeView: 'chat', terminalEnabled: false, onViewChange: () => {}, onSearch: () => {}, onNavigate: navigate,
      sidebarHeader: <div className="desktop-workspace-device"><DeviceStateMark state={device?.connected ? 'online' : 'offline'} showLabel={false} label={translate(device?.connected ? 'desktopDevices.online' : 'desktopDevices.offline')} /><Select aria-label={translate('desktopDevices.device')} value={deviceId ?? ''} options={index.devices.map(device => ({ value: device.id, label: device.name }))} onValueChange={id => navigate(workspacesHref(id))} /></div>,
      sidebarLeading: <>{actions}{desktop.sidebarLeading}</>,
      renderThreadMenu: (thread, context) => <RecentThreadMenu thread={thread} context={context} currentKey={navigation.currentKey} onFavorite={navigation.onToggleThreadFavorite} onRenamed={navigation.onThreadRenamed} onRemoved={navigation.onThreadRemoved} onNavigate={navigate} />,
    })} settings={relayModeActive() ? <RelayUserMenu rail /> : <button aria-label={translate('desktopShell.settings')} onClick={shell?.openSettings}><Settings data-icon="settings" /></button>} newThread={null} actions={null} threadMenu={null} connection={null} explorer={null} revealExplorer={0}>
      <div className="desktop-workspaces-scroll">{children}</div>
    </MatterWorkbench>
  </div>;
}
