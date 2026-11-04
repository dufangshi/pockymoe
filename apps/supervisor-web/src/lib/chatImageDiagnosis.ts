import { translate } from '@pockymoe/thread-ui/i18n';
import { relayOrigin } from './relayOrigin';

/** Inspect the failed request itself, including the worker's decrypted status.
 * Never emit URLs, addresses, tokens, response bodies or exception messages. */
export async function diagnoseChatImage(src: string): Promise<string | null> {
  const url = new URL(src, window.location.href);
  if (!['http:', 'https:'].includes(url.protocol) ||
      ![window.location.origin, relayOrigin()].includes(url.origin) ||
      !/\/api\/(?:threads|workspaces)\//.test(url.pathname)) return null;
  const path = `${url.pathname}?path=${url.searchParams.get('path') ?? ''}`;
  const hash = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(path));
  const pathHash = Array.from(new Uint8Array(hash)).map(byte => byte.toString(16).padStart(2, '0')).join('');
  let status = 0, contentType = '', code = '';
  let missing = false;
  try {
    const response = await fetch(url, { credentials: 'include', cache: 'no-store', signal: AbortSignal.timeout(10_000) });
    status = response.status;
    // Only the MIME essence, never arbitrary header parameters, reaches logs.
    const type = response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase() ?? '';
    contentType = /^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/.test(type) ? type : '';
    if (contentType === 'application/json' && !response.ok) {
      const body: unknown = await response.json().catch(() => null);
      if (body && typeof body === 'object' && 'code' in body && typeof body.code === 'string') code = body.code;
      // 0.13.45 reports ENOENT as 400/bad_request. Classify it without
      // displaying or logging the response's private filesystem path.
      missing = status === 400 && !!body && typeof body === 'object' &&
        'message' in body && typeof body.message === 'string' &&
        /no such file or directory|os error 2\b/i.test(body.message);
    } else await response.body?.cancel();
  } catch { /* Status zero records a network/worker failure without leaking it. */ }
  console.info('chat-image-failure', { status, contentType, pathHash });
  if (status === 401) return translate('chat.imageSessionExpired');
  if (status === 404 || missing) return translate('chat.imageFileMissing');
  if (['device_offline', 'device_unresponsive'].includes(code)) return translate('chat.imageDeviceOffline');
  if (status === 403) return translate('chat.imageAccessDenied');
  if (status >= 200 && status < 300 && contentType.startsWith('image/')) return translate('chat.imageCannotDisplay');
  return null;
}
