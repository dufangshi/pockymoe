import { appPathname } from '../webBase';

/**
 * The guided tour's sample device. Its pages (`/devices/pockymoe-tour/...`) are
 * answered in the browser by a small in-memory Supervisor, so the tour has the
 * same workspace, conversations, terminal and files for every account, with or
 * without devices of its own, and never touches real ones.
 */
export const TOUR_DEVICE_ID = 'pockymoe-tour';
/** The sample conversations the tour opens: the main one and the helper agent it started. */
export const TOUR_MAIN_THREAD = 'c27d9e41-5b8a-4f3c-8e61-2d4b7a9c1e01';
export const TOUR_HELPER_THREAD = 'c27d9e41-5b8a-4f3c-8e61-2d4b7a9c1e02';
const TOUR_PREFIX = `relay/devices/${TOUR_DEVICE_ID}/`;
const TOUR_PATH = new RegExp(`^/devices/${TOUR_DEVICE_ID}(?:/|$)`);
// Relay calls a tour page makes about its device: answered for the sample device.
const TOUR_RELAY = /^relay\/(?:portal|access|thread-links|shares|grants|public-links|presence)(?:[/?]|$)/;

export function isTourDevice(deviceId: string | null | undefined) {
  return deviceId === TOUR_DEVICE_ID;
}

/** The page shows the sample device. */
export function onTourRoute() {
  return typeof window !== 'undefined' && TOUR_PATH.test(appPathname(window.location.pathname));
}

/** A request path (as `apiPath` resolves it) the sample device answers. */
export function isTourRequest(path: string) {
  const relative = path.replace(/^\//, '');
  if (relative.startsWith(TOUR_PREFIX)) return true;
  if (!onTourRoute()) return false;
  return TOUR_RELAY.test(relative) || relative.includes(`deviceId=${TOUR_DEVICE_ID}`);
}

const server = () => import('./server');

export async function tourFetch(path: string, init: RequestInit = {}): Promise<Response> {
  return (await server()).handleTourRequest(path.replace(/^\//, ''), init);
}

type Listener = (event: Event) => void;

/**
 * A WebSocket-shaped connection to the sample device: thread events and
 * terminals, like the Supervisor's `/ws`.
 */
export class TourSocket extends EventTarget {
  readyState: number = WebSocket.CONNECTING;
  readonly url = `tour://${TOUR_DEVICE_ID}/ws`;
  private detach: (() => void) | null = null;
  private send_: ((data: string) => void) | null = null;
  private queued: string[] = [];
  onmessage: Listener | null = null;
  onopen: Listener | null = null;
  onclose: Listener | null = null;
  onerror: Listener | null = null;

  constructor() {
    super();
    void server().then(({ connectTourSocket }) => {
      if (this.readyState !== WebSocket.CONNECTING) return;
      const connection = connectTourSocket((data) => this.emit(new MessageEvent('message', { data })));
      this.detach = connection.close;
      this.send_ = connection.send;
      this.readyState = WebSocket.OPEN;
      this.emit(new Event('open'));
      connection.ready();
      for (const data of this.queued.splice(0)) connection.send(data);
    });
  }

  private emit(event: Event) {
    if (this.readyState === WebSocket.CLOSED && event.type !== 'close') return;
    this.dispatchEvent(event);
    const handler = (this as unknown as Record<string, Listener | null>)[`on${event.type}`];
    handler?.call(this, event);
  }

  send(data: string) {
    if (this.send_) this.send_(data);
    else this.queued.push(data);
  }

  close() {
    if (this.readyState === WebSocket.CLOSED) return;
    this.readyState = WebSocket.CLOSED;
    this.detach?.();
    this.emit(new CloseEvent('close', { code: 1000 }));
  }
}

/**
 * Starts the sample over for a new tour: its conversations, files and the
 * tabs and recents kept for it in this browser.
 */
export async function resetTourSandbox() {
  try {
    for (const key of Object.keys(localStorage)) if (key.endsWith(`:${TOUR_DEVICE_ID}`)) localStorage.removeItem(key);
  } catch { /* Storage may be unavailable; the sample still starts over. */ }
  (await server()).resetTourSandbox();
}

/** Browser state the tour keeps apart from the reader's own (tabs, recents). */
export function tourStateKey(key: string) {
  return `${key}:${TOUR_DEVICE_ID}`;
}
