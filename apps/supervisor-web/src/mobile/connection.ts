import type { PockymoeHost } from '../desktop/host';
import { failHostRelayOrigin, setHostRelayOrigin } from '../lib/relayOrigin';
import { translate } from '@pockymoe/thread-ui/i18n';

const mobile = new WeakSet<PockymoeHost>();
export function connectedMobileHost() { const host = window.pockymoeHost; return host && mobile.has(host) ? host : undefined; }
const ready = new WeakMap<PockymoeHost, Promise<void>>();
/** The shell probes the person's one address and preserves its resolved Web /
 * Relay pair. Initialize before rendering so session, sign-in, push and device
 * requests all use that Relay rather than this deployed Web's build default. */
export function initializeMobileConnection(host = window.pockymoeHost): Promise<void> {
  if (!host) return Promise.resolve();
  let task = ready.get(host);
  if (!task) {
    task = (async () => {
      try {
        if ((await host.capabilities()).surface !== 'mobile') return;
        const settings = await host.settings.get();
        if (new URL(settings.webOrigin).origin !== location.origin) throw Error('Mismatched Web origin');
        setHostRelayOrigin(settings.relayOrigin); mobile.add(host);
      } catch {
        const error = new Error(translate('auth.unableToLoadTheRelayPortal'));
        failHostRelayOrigin(error); throw error;
      }
    })();
    ready.set(host, task);
  }
  return task;
}
