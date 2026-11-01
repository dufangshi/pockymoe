import type { DesktopSettings, HostCommand, PockymoeHost } from './host';
/** Dev-only entry, removed entirely from the production import graph. */
export function installMockHost() {
  if (window.pockymoeHost) return;
  window.__POCKYMOE_BOOTSTRAP__ = { mode: 'relay' };
  let settings: DesktopSettings = { webOrigin: location.origin, relayOrigin: location.origin, alertMode: 'petFirst', petVisible: false, petId: 'default', petSize: 'M', petMini: false, petSound: false, launchAtLogin: false, quickWindowShortcut: 'Alt+Space', snoozeUntil: null };
  const listeners = new Set<(command: HostCommand) => void>();
  window.pockymoeHost = {
    capabilities: async () => ({ surface: 'desktop', bridgeVersion: 2, platform: new URLSearchParams(location.search).get('desktopMockPlatform') === 'win32' ? 'win32' : 'darwin', shellVersion: 'development', notifications: 'local', windowChrome: { kind: 'hiddenInset', trafficLightsWidth: 78, toolbarHeight: 38, controlsSide: 'left' }, features: { pet: true, quickWindow: true, multiWindow: true, nativeSave: true, badge: true } }),
    activity: { sync: snapshot => window.dispatchEvent(new CustomEvent('desktopMock:snapshot', { detail: snapshot })) },
    appearance: { sync: appearance => window.dispatchEvent(new CustomEvent('desktopMock:appearance', { detail: appearance })) },
    onCommand: listener => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    commandResult: (id, result) => window.dispatchEvent(new CustomEvent('desktopMock:commandResult', { detail: { id, ...result } })),
    window: { openThread: path => { history.pushState(null, '', path); window.dispatchEvent(new PopStateEvent('popstate')); }, setTitle: title => { document.title = title; }, openPreferences: section => window.dispatchEvent(new CustomEvent('desktopMock:preferences', { detail: section })) },
    shell: { openExternal: url => { window.open(url, '_blank', 'noopener,noreferrer'); } },
    files: { saveAs: async (name, data) => { window.dispatchEvent(new CustomEvent('desktopMock:save', { detail: { name, bytes: data.byteLength } })); return { saved: false }; } },
    pet: { setVisible: visible => { settings = { ...settings, petVisible: visible }; }, isVisible: async () => settings.petVisible },
    settings: { get: async () => settings, set: async patch => { settings = { ...settings, ...patch }; return settings; } },
    badge: { set: (count, style) => window.dispatchEvent(new CustomEvent('desktopMock:badge', { detail: { count, style } })) },
  } satisfies PockymoeHost;
  window.addEventListener('desktopMock:command', event => listeners.forEach(listener => listener((event as CustomEvent<HostCommand>).detail)));
}
