import { requireTopLevelWeb } from './lib/staticWeb';
import { installStaleBuildReload } from './lib/staleBuildReload';
import { tryOpenPortPreview } from './lib/portPreviewBootstrap';
import { installPageZoomGuard } from './lib/pageZoom';

if (import.meta.env.VITE_POCKYMOE_STATIC_WEB === 'true') requireTopLevelWeb(window);
installStaleBuildReload();
installPageZoomGuard();

// A CLI lease already carries device authorization. Redeem it before loading
// the workbench, its browser-key setup, editor assets and stored theme pack.
void tryOpenPortPreview().then(opened => {
  if (!opened) void import('./renderApp');
});
