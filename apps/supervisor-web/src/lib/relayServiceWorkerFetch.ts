import { translate } from '@pockymoe/thread-ui/i18n';
import {
  deviceRoute,
  exchange,
  scopeFromPage,
  TransportError,
} from './relayTransportCrypto';
import { AccountTrustError } from './accountVaultState';
// The worker uses the same exchange/signing path and reads CryptoKeys from IDB.
export async function relayWorkerFetch(request: Request, clientUrl?: string) {
  try {
    return (
      await exchange(
        new Request(request, { credentials: 'include' }),
        clientUrl
          ? scopeFromPage(
              clientUrl,
              deviceRoute(new URL(request.url))?.deviceId,
            )
          : undefined,
      )
    ).response;
  } catch (error) {
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
