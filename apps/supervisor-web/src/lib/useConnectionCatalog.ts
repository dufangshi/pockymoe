import { translate } from '@pockymoe/thread-ui/i18n';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  connectionErrorMessage,
  connectionsApi,
  syncConnection,
  type ConnectionCatalog,
  type ConnectionSource,
  type ConnectionSummary,
} from './connections';

/** Loads the account catalog for one device and keeps per-row sync state. */
export function useConnectionCatalog(apiRoot: string) {
  const api = useMemo(() => connectionsApi(apiRoot), [apiRoot]);
  const [catalog, setCatalog] = useState<ConnectionCatalog | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [syncing, setSyncing] = useState<string | null>(null);
  const [syncErrors, setSyncErrors] = useState<Record<string, string>>({});
  const request = useRef(0);

  const reload = useCallback(async () => {
    const id = ++request.current;
    setLoading(true);
    setError('');
    try {
      const value = await api.list();
      if (id !== request.current) return null;
      setCatalog(value);
      return value;
    } catch (e) {
      if (id === request.current)
        setError(
          connectionErrorMessage(e, translate('settings.connLoadFailed')),
        );
      return null;
    } finally {
      if (id === request.current) setLoading(false);
    }
  }, [api]);

  useEffect(() => {
    void reload();
    return () => {
      ++request.current;
    };
  }, [reload]);

  /** Credentials land on this device only; the caller decides what to do next. */
  const sync = useCallback(
    async (connection: ConnectionSummary, source: ConnectionSource) => {
      const deviceId = catalog?.deviceId;
      if (!deviceId) return null;
      setSyncing(connection.id);
      setSyncErrors(({ [connection.id]: _, ...rest }) => rest);
      try {
        const result = await syncConnection(
          apiRoot,
          connection,
          source,
          deviceId,
        );
        await reload();
        return result;
      } catch (e) {
        setSyncErrors((current) => ({
          ...current,
          [connection.id]: connectionErrorMessage(
            e,
            translate('settings.connSyncFailed'),
          ),
        }));
        return null;
      } finally {
        setSyncing(null);
      }
    },
    [apiRoot, catalog?.deviceId, reload],
  );

  return {
    api,
    catalog,
    loading,
    error,
    reload,
    sync,
    syncing,
    syncErrors,
    /** Updates one row immediately after a save, before the reload lands. */
    upsert: (connection: ConnectionSummary) =>
      setCatalog((current) =>
        current
          ? {
              ...current,
              connections: current.connections.some(
                (c) => c.id === connection.id,
              )
                ? current.connections.map((c) =>
                    c.id === connection.id ? { ...c, ...connection } : c,
                  )
                : [...current.connections, connection],
            }
          : current,
      ),
  };
}

/** One device's catalog, shared by the Connections list and harness usage. */
export type ConnectionCatalogStore = ReturnType<typeof useConnectionCatalog>;
