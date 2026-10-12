import { useEffect, useState } from 'react';
import { Link, useLocation, useParams } from 'react-router-dom';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { openDevicePortPreview } from '../lib/portPreview';

/** Exchange the browser's account session for a private preview-host cookie. */
export function PortPreviewPage() {
  useI18n();
  const { deviceId, mappingId } = useParams();
  const location = useLocation();
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    setError(null);
    const query = new URLSearchParams(location.search);
    const path = query.get('path') ?? '/';
    const leaseId = query.get('lease');
    const expiry = query.get('leaseExpiresAt');
    const expiresAt = expiry && /^\d+$/.test(expiry) && Number.isSafeInteger(Number(expiry)) ? Number(expiry) : undefined;
    openDevicePortPreview(deviceId ?? '', mappingId ?? '', path, controller.signal, leaseId ? { leaseId, ...(expiresAt === undefined ? {} : { expiresAt }) } : undefined).then(({ url }) => {
      if (!controller.signal.aborted) window.location.replace(url);
    }).catch((cause: unknown) => {
      if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
    });
    return () => controller.abort();
  }, [deviceId, mappingId, location.search]);
  return <main className="flex min-h-screen items-center justify-center bg-[var(--app-bg)] p-6 text-[var(--theme-fg)]" data-page="port-preview">
    <section className="max-w-lg space-y-4">
      <h1 className="text-xl font-semibold">{translate('devices.portMappings')}</h1>
      {error ? <p role="alert">{error}</p> : <p role="status">{translate('devices.opening')}</p>}
      {error && <Link className="underline" to="/">{translate('workbench.pockymoeHome')}</Link>}
    </section>
  </main>;
}
