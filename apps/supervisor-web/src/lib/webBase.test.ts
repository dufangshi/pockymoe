import { afterEach, describe, expect, it, vi } from 'vitest';
import { appPathname, appUrl, resolveWebBase, supervisorPath } from './webBase';

afterEach(() => {
  document.querySelector('base')?.remove();
  vi.unstubAllEnvs();
  vi.resetModules();
});

describe('runtime Web base', () => {
  const prefix = '/api/workspaces/W/agents/A/interface/ui/';
  it.each([
    ['', './'], ['index.html', './'], ['workspaces', './'],
    ['threads/t', '../'], ['threads/t/', '../../'],
    ['devices/d/threads/t', '../../../'],
  ])('resolves the stripped route %s under any prefix', (route, relative) => {
    const page = `https://proxy.test${prefix}${route}`;
    const baseURI = new URL(relative, page).href;
    expect(resolveWebBase(baseURI, page)).toBe(prefix);
    expect(resolveWebBase(new URL(relative, `https://proxy.test/${route}`).href, `https://proxy.test/${route}`)).toBe('/');
  });
  it('preserves encoding, queries, fragments and external assets', () => {
    expect(appUrl('api/files?path=a%20b#part', prefix)).toBe(`${prefix}api/files?path=a%20b#part`);
    expect(appUrl('/relay/devices/a%2Fb/ws', prefix)).toBe(`${prefix}relay/devices/a%2Fb/ws`);
    expect(appUrl('./vendor/3Dmol-min.js', prefix)).toBe(`${prefix}vendor/3Dmol-min.js`);
    for (const url of ['https://other.test/a', '//other.test/a', 'blob:https://proxy.test/id', 'data:image/png;base64,a'])
      expect(appUrl(url, prefix)).toBe(url);
    expect(appUrl('api/version', '/')).toBe('/api/version');
    expect(appUrl('/api/version', '/')).toBe('/api/version');
    expect(supervisorPath('api/version')).toBe('/api/version');
  });
  it('strips only the complete mount when reading browser routes', () => {
    expect(appPathname(`${prefix}devices/d/threads/t`, prefix)).toBe('/devices/d/threads/t');
    expect(appPathname('/another/threads/t', prefix)).toBe('/another/threads/t');
    expect(appPathname('/threads/t', '/')).toBe('/threads/t');
    expect(resolveWebBase('https://proxy.test/encoded%20mount/', 'https://proxy.test/encoded%20mount/threads/t')).toBe('/encoded%20mount/');
    expect(() => resolveWebBase('https://other.test/', 'https://proxy.test/')).toThrow('page origin');
  });
  it('captures the document base once and composes local and separate-origin relay transports', async () => {
    const base = document.createElement('base');
    base.href = prefix;
    document.head.append(base);
    vi.resetModules();
    const { webBase } = await import('./webBase');
    const { relayUrl, relaySocketUrl, relayFetchInit } = await import('./relayOrigin');
    const { deviceRoute } = await import('./relayTransportCrypto');
    const { relayWorkerRegistration } = await import('./staticWeb');
    const { workbenchUrls } = await import('./workbenchUrls');
    const { currentRelayDeviceIdFromPath, currentThreadIdFromPath } = await import('./relayRoutes');
    const origin = window.location.origin;
    base.href = '/changed/';
    expect(webBase).toBe(prefix);
    const navigate = vi.fn();
    const menu = vi.fn();
    const source = { threads: [{ href: '/threads/t' }], workspaceThreads: [{ href: '/threads/t' }], openTabs: [{ href: '/threads/o' }], notifications: [{ href: '/devices/d/threads/t' }], onNavigate: navigate, renderThreadMenu: menu };
    const options = workbenchUrls(source as never);
    expect(options.openTabs?.[0]?.href).toBe(`${prefix}threads/o`);
    // A right-click's place reaches the host's thread menu with the logical href.
    const context = { x: 1, y: 2, onClose: () => {} };
    options.renderThreadMenu!({ href: `${prefix}threads/t` } as never, context);
    expect(menu).toHaveBeenCalledWith({ href: '/threads/t' }, context);
    expect(options.threads[0]?.href).toBe(`${prefix}threads/t`);
    expect(options.workspaceThreads?.[0]?.href).toBe(`${prefix}threads/t`);
    expect(options.notifications[0]?.href).toBe(`${prefix}devices/d/threads/t`);
    options.onNavigate(`${prefix}threads/t?x=1`);
    expect(navigate).toHaveBeenCalledWith('/threads/t?x=1');
    expect(source.threads[0]?.href).toBe('/threads/t');
    expect(relayUrl('api/version', origin, '')).toBe(`${prefix}api/version`);
    expect(relayUrl('relay/devices/d/api/version', origin, '')).toBe(`${prefix}relay/devices/d/api/version`);
    expect(relayUrl(`${prefix}relay/devices/d/api/version`, origin, '')).toBe(`${prefix}relay/devices/d/api/version`);
    expect(relaySocketUrl('relay/devices/d/ws', origin, '')).toBe(`${origin.replace('http:', 'ws:')}${prefix}relay/devices/d/ws`);
    expect(deviceRoute(new URL(`${origin}${prefix}relay/devices/d/api/version?q=1`))).toEqual({ deviceId: 'd', path: '/api/version?q=1' });
    expect(relayWorkerRegistration('/bundled.js', true)).toEqual({ url: `${prefix}relayServiceWorker.js`, options: { scope: prefix, type: 'module' } });
    window.history.replaceState(null, '', `${prefix}devices/d/threads/t`);
    expect(currentRelayDeviceIdFromPath()).toBe('d');
    expect(currentThreadIdFromPath()).toBe('t');
    window.history.replaceState(null, '', '/');
    vi.stubEnv('VITE_POCKYMOE_RELAY_ORIGIN', 'https://relay.test');
    expect(relayUrl('relay/devices/d/api/version')).toBe('https://relay.test/relay/devices/d/api/version');
    expect(relayFetchInit('relay/portal', { method: 'GET' })).toEqual({ method: 'GET', credentials: 'include' });
    expect(relaySocketUrl('relay/devices/d/ws')).toBe('wss://relay.test/relay/devices/d/ws');
  });
});
