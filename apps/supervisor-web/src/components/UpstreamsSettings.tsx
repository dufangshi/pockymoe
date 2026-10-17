import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState } from 'react';
import { useLocation } from 'react-router-dom';
import { relayModeActive, request } from '../lib/api';
import { relayDeviceIdFromPath } from '../lib/relayRoutes';
import { useConnectionCatalog } from '../lib/useConnectionCatalog';
import { ConnectionCatalog } from './ConnectionCatalog';
import {
  HarnessConnections,
  type InventoryHarness,
} from './HarnessConnections';

export function UpstreamsSettings() {
  useI18n();
  const { pathname } = useLocation();
  const deviceId = relayDeviceIdFromPath(pathname);
  if (relayModeActive() && !deviceId) {
    return (
      <p className="py-5 text-sm text-[var(--theme-fg-muted)]">
        {translate('settings.upstreamsChooseDevice')}
      </p>
    );
  }
  const apiRoot = deviceId
    ? `relay/devices/${encodeURIComponent(deviceId)}/api`
    : 'api';
  return <DeviceUpstreams key={apiRoot} apiRoot={apiRoot} />;
}

/**
 * Connections first: they are saved, synced and edited without any harness. Using
 * them in a harness is a separate section below that reads the same catalog.
 */
function DeviceUpstreams({ apiRoot }: { apiRoot: string }) {
  const store = useConnectionCatalog(apiRoot);
  const [harnesses, setHarnesses] = useState<InventoryHarness[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let alive = true;
    setLoading(true);
    setError('');
    void request<InventoryHarness[]>(`${apiRoot}/management/harnesses`, {
      cache: 'no-store',
    })
      .then((inventory) => {
        if (alive) setHarnesses(Array.isArray(inventory) ? inventory : []);
      })
      .catch(() => {
        if (alive) setError(translate('settings.upstreamsInventoryError'));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [apiRoot, revision]);
  return (
    <section
      className="min-w-0 space-y-6 py-1"
      aria-label={translate('settings.upstreamsTab')}
    >
      <ConnectionCatalog
        apiRoot={apiRoot}
        store={store}
        harnessNames={Object.fromEntries(harnesses.map((h) => [h.id, h.name]))}
        onRefresh={() => {
          void store.reload();
          setRevision((v) => v + 1);
        }}
      />
      <HarnessConnections
        apiRoot={apiRoot}
        store={store}
        harnesses={harnesses}
        loading={loading}
        error={error}
        onRetry={() => setRevision((v) => v + 1)}
      />
    </section>
  );
}
