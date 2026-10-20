import { translate } from '@pockymoe/thread-ui/i18n';
import {
  deviceRoute,
  exchange,
  scopeFromPage,
  TransportError,
} from './relayTransportCrypto';
import { AccountTrustError } from './accountVaultState';
/** Sends a request through the page that asked for it, and returns its answer. */
export type PageFetch = (request: Request) => Promise<Response>;

// The worker uses the same exchange/signing path and reads CryptoKeys from IDB.
// Safari's worker cannot always read them (WebKit bugs 183167, 312279): a
// browser key the page holds reads as missing there. Then the page that asked
// (which reads it) makes the request instead, and only its own answer counts.
export async function relayWorkerFetch(request: Request, clientUrl?: string, viaPage?: PageFetch) {
  const retry = viaPage && ['GET', 'HEAD'].includes(request.method) ? request.clone() : null;
  try {
    const { response } = await exchange(
      new Request(request, { credentials: 'include' }),
      clientUrl
        ? scopeFromPage(
            clientUrl,
            deviceRoute(new URL(request.url))?.deviceId,
          )
        : undefined,
    );
    if (response.status === 401 && retry && viaPage) return await viaPage(retry);
    return response;
  } catch (error) {
    if (retry && viaPage && error instanceof AccountTrustError && error.code === 'accountUnlockRequired') {
      try { return await viaPage(retry); } catch { /* The worker's own failure below. */ }
    }
    const code =
      error instanceof TransportError || error instanceof AccountTrustError
        ? error.code
        : 'transport_unavailable';
    return new Response(
      JSON.stringify({
        code,
        message:
          error instanceof Error
            ? error.message
            : translate('workbench.encryptedConnectionFailed'),
      }),
      {
        status:
          ['accountUnlockRequired', 'browser_registration_required', 'browser_revoked'].includes(code)
            ? 401
            : 502,
        headers: {
          'content-type': 'application/json',
          'cache-control': 'no-store',
        },
      },
    );
  }
}
