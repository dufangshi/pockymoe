import { appPathname } from './webBase';
const RELAY_DEVICE_PATH_RE = /^\/devices\/([^/]+)(?:\/|$)/;
const THREAD_PATH_RE =
  /^(?:\/devices\/[^/]+)?\/threads\/([^/?#]+)(?:[/?#]|$)/;

export function relayDeviceIdFromPath(pathname: string) {
  const match = RELAY_DEVICE_PATH_RE.exec(pathname);
  return match ? decodeURIComponent(match[1] ?? '') : null;
}

export function currentRelayDeviceIdFromPath() {
  if (typeof window === 'undefined') {
    return null;
  }
  return relayDeviceIdFromPath(appPathname(window.location.pathname));
}

export function threadIdFromPath(pathname: string) {
  const match = THREAD_PATH_RE.exec(pathname);
  return match ? decodeURIComponent(match[1] ?? '') : null;
}

export function currentThreadIdFromPath() {
  if (typeof window === 'undefined') {
    return null;
  }
  return threadIdFromPath(appPathname(window.location.pathname));
}

export function relayScopedPath(path: string, deviceId?: string | null) {
  if (!deviceId) {
    return path;
  }
  const normalizedPath = path.startsWith('/') ? path : `/${path}`;
  return `/devices/${encodeURIComponent(deviceId)}${normalizedPath}`;
}

export function currentRelayScopedPath(path: string) {
  return relayScopedPath(path, currentRelayDeviceIdFromPath());
}

export function threadHref(threadId: string, deviceId?: string | null) {
  return relayScopedPath(`/threads/${encodeURIComponent(threadId)}`, deviceId);
}

export function currentThreadHref(threadId: string) {
  return threadHref(threadId, currentRelayDeviceIdFromPath());
}

export function threadsHref(workspaceId?: string | null, deviceId?: string | null) {
  const query = workspaceId ? `?workspaceId=${encodeURIComponent(workspaceId)}` : '';
  return relayScopedPath(`/threads${query}`, deviceId);
}

export function currentThreadsHref(workspaceId?: string | null) {
  return threadsHref(workspaceId, currentRelayDeviceIdFromPath());
}

export function newThreadHref(workspaceId?: string | null, deviceId?: string | null) {
  const query = workspaceId ? `?workspaceId=${encodeURIComponent(workspaceId)}` : '';
  return relayScopedPath(`/threads/new${query}`, deviceId);
}

export function currentNewThreadHref(workspaceId?: string | null) {
  return newThreadHref(workspaceId, currentRelayDeviceIdFromPath());
}

export function workspacesHref(deviceId?: string | null) {
  return relayScopedPath('/workspaces', deviceId);
}

export function currentWorkspacesHref() {
  return workspacesHref(currentRelayDeviceIdFromPath());
}
