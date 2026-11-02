import { initializeMobileConnection } from './mobile/connection';
import { requireTopLevelWeb } from './lib/staticWeb';
import { installStaleBuildReload } from './lib/staleBuildReload';
import { tryOpenPortPreview } from './lib/portPreviewBootstrap';
import { installPageZoomGuard } from './lib/pageZoom';
import { installTouchTapGuard } from './lib/touchTapGuard';

if (import.meta.env.VITE_POCKYMOE_STATIC_WEB === 'true') requireTopLevelWeb(window);
installStaleBuildReload();
installPageZoomGuard();
installTouchTapGuard();

// A CLI lease already carries device authorization. Redeem it before loading
// the workbench, its browser-key setup, editor assets and stored theme pack.
// Failed shell configuration blocks Relay requests; the existing sign-in error
// UI presents the failure instead of silently contacting the Web origin.
void initializeMobileConnection().catch(() => undefined).then(() => tryOpenPortPreview()).then(async opened => {
  if (opened) return;
  if (import.meta.env.DEV && new URLSearchParams(location.search).get('desktopMockHost') === '1') {
    const { installMockHost } = await import('./desktop/devMockHost'); installMockHost();
  }
  await import('./renderApp');
});
