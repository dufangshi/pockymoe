import type { MatterWorkbenchOptions } from '@pockymoe/thread-ui';
import { appPathname, appUrl } from './webBase';

// Shared components render plain anchors. Supply browser hrefs at the host
// boundary, then turn callbacks back into logical React Router destinations.
export function workbenchUrls(options: MatterWorkbenchOptions): MatterWorkbenchOptions {
  return {
    ...options,
    threads: options.threads.map(thread => ({ ...thread, href: appUrl(thread.href) })),
    ...(options.workspaceThreads ? {
      workspaceThreads: options.workspaceThreads.map(thread => ({ ...thread, href: appUrl(thread.href) })),
    } : {}),
    ...(options.activeThreads ? {
      activeThreads: options.activeThreads.map(thread => ({ ...thread, href: appUrl(thread.href) })),
    } : {}),
    notifications: options.notifications.map(notification => ({ ...notification, href: appUrl(notification.href) })),
    onNavigate: href => options.onNavigate(appPathname(href)),
    ...(options.onPrefetchThread ? { onPrefetchThread: (href: string) => options.onPrefetchThread!(appPathname(href)) } : {}),
    ...(options.renderThreadMenu ? {
      renderThreadMenu: thread => options.renderThreadMenu!({ ...thread, href: appPathname(thread.href) }),
    } : {}),
  };
}
