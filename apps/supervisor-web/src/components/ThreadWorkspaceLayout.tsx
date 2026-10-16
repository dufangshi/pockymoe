import { useHostScope } from '../lib/hostCapabilities';
import {
  ThreadCards,
  usePlugins,
  ThreadWorkspaceLayout as SharedThreadWorkspaceLayout,
} from '@pockymoe/thread-ui';
import type { ComponentProps } from 'react';
import { Link, useNavigate, useLocation } from 'react-router-dom';
import { DeviceMonitor } from './DeviceMonitor';
import { TourLauncherButton } from './onboarding/TourProvider';
import { relayDeviceIdFromPath } from '../lib/relayRoutes';

import { useAppShellNav } from './AppShellNavContext';
import { appSettingsSections } from './AppShellSettingsDialog';
import {
  currentNewThreadHref,
  currentThreadHref,
  currentWorkspacesHref,
} from '../lib/relayRoutes';
import { ThreadCreateForm } from '../pages/thread-create/ThreadCreateForm';
import { appUrl } from '../lib/webBase';
import { workbenchUrls } from '../lib/workbenchUrls';

type ThreadWorkspaceLayoutProps = ComponentProps<
  typeof SharedThreadWorkspaceLayout
>;

function buildNewThreadHref(workspaceId?: string | null) {
  return currentNewThreadHref(workspaceId);
}

export { ThreadCards };

export function ThreadWorkspaceLayout({
  getThreadHref,
  onOpenThread,
  getNewThreadHref,
  renderThreadLink,
  onCloseAppNavigation,
  ...props
}: ThreadWorkspaceLayoutProps) {
  const navigate = useNavigate();
  const plugins = usePlugins();
  const hostScope = useHostScope();
  const location = useLocation();
  const deviceId = relayDeviceIdFromPath(location.pathname) ?? 'local';
  const shellNav = useAppShellNav();
  const closeAppNavigation = onCloseAppNavigation ?? shellNav?.closeNav;
  const effectiveTheme = props.effectiveTheme ?? shellNav?.effectiveTheme;
  const themeMode = props.themeMode ?? shellNav?.themeMode;
  const onThemeModeChange = props.onThemeModeChange ?? shellNav?.setThemeMode;

  return (
    <SharedThreadWorkspaceLayout
      {...props}
      {...(props.workbench ? { workbench: workbenchUrls(props.workbench) } : {})}
      restrictedNavigation={Boolean(hostScope)}
      deviceMonitor={hostScope ? null : <><DeviceMonitor key={deviceId} /><TourLauncherButton /></>}
      workspaceReturnHref={appUrl(props.workspaceReturnHref ?? currentWorkspacesHref())}
      settingsSections={props.settingsSections ?? appSettingsSections(plugins.getSettingsSections())}
      {...(effectiveTheme ? { effectiveTheme } : {})}
      {...(themeMode ? { themeMode } : {})}
      {...(onThemeModeChange ? { onThemeModeChange } : {})}
      getThreadHref={(threadId) => appUrl(getThreadHref ? getThreadHref(threadId) : currentThreadHref(threadId))}
      onOpenThread={
        onOpenThread ?? ((threadId) => navigate(currentThreadHref(threadId)))
      }
      getNewThreadHref={(workspaceId) => appUrl((getNewThreadHref ?? buildNewThreadHref)(workspaceId))}
      renderNewThreadDialogContent={
        props.renderNewThreadDialogContent ??
        (({ close, closeNavigation, currentWorkspaceId }) => (
          <ThreadCreateForm
            variant="dialog"
            initialWorkspaceId={currentWorkspaceId}
            onCancel={close}
            onCreated={(thread) => {
              close();
              closeNavigation();
              navigate(currentThreadHref(thread.id));
            }}
          />
        ))
      }
      renderThreadLink={
        renderThreadLink ??
        (({ thread, children, className, onClick }) => (
          <Link
            to={currentThreadHref(thread.id)}
            className={className}
            onClick={onClick}
          >
            {children}
          </Link>
        ))
      }
      {...(closeAppNavigation
        ? { onCloseAppNavigation: closeAppNavigation }
        : {})}
    />
  );
}
