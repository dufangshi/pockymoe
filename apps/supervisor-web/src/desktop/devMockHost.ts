import type { DesktopSettings, HostCommand, RemoteCodexHost } from './host';
/** Dynamically imported only in development, never included in the production graph. */
export function installMockHost() {
  if (window.remoteCodexHost) return;
  const params = new URLSearchParams(location.search);
  const windows = params.get('desktopMockPlatform') === 'win32';
  const opaque = params.get('desktopMockOpaque') === '1';
  const backdrop = document.createElement('style');
  backdrop.textContent = `html:has(.rc-desktop)[data-theme-effective=dark]{background:${opaque ? '#191a1d' : 'linear-gradient(150deg,#1c2027,#151719 55%,#24201c)'}}html:has(.rc-desktop)[data-theme-effective=light]{background:${opaque ? '#eeefec' : 'linear-gradient(150deg,#e5e9ed,#f1f1ec 55%,#ede9e3)'}}`;
  document.head.append(backdrop);
  window.__REMOTE_CODEX_BOOTSTRAP__ = { mode: 'relay' };
  let petVisible = false;
  let settings: DesktopSettings = { relayOrigin: location.origin, alertMode: 'petFirst', petVisible, petId: 'default', petSize: 'M', petMini: false, petSound: false, launchAtLogin: false, quickWindowShortcut: 'Alt+Space', snoozeUntil: null };
  const listeners = new Set<(command: HostCommand) => void>();
  window.remoteCodexHost = {
    capabilities: async () => ({ surface: 'desktop', bridgeVersion: 1, platform: windows ? 'win32' : 'darwin', shellVersion: 'development', notifications: 'local', windowChrome: { kind: windows ? 'titleBarOverlay' : 'hiddenInset', trafficLightsWidth: windows ? 138 : 78, toolbarHeight: 38, controlsSide: windows ? 'right' : 'left' }, features: { pet: true, quickWindow: true, multiWindow: true, nativeSave: false, badge: true } }),
    activity: { sync: snapshot => { window.dispatchEvent(new CustomEvent('desktopMock:snapshot', { detail: snapshot })); } },
    onCommand: listener => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    commandResult: (id, result) => { window.dispatchEvent(new CustomEvent('desktopMock:commandResult', { detail: { id, ...result } })); },
    window: { openThread: path => { window.history.pushState(null, '', path); window.dispatchEvent(new PopStateEvent('popstate')); }, setTitle: title => { document.title = title; } },
    shell: { openExternal: url => { window.open(url, '_blank', 'noopener,noreferrer'); } },
    files: { saveAs: async () => ({ saved: false }) },
    pet: { setVisible: visible => { petVisible = visible; }, isVisible: async () => petVisible },
    settings: { get: async () => settings, set: async patch => { settings = { ...settings, ...patch }; return settings; } },
    badge: { set: () => undefined },
  } satisfies RemoteCodexHost;
  window.addEventListener('desktopMock:command', event => { const command = (event as CustomEvent<HostCommand>).detail; listeners.forEach(listener => listener(command)); });
}
