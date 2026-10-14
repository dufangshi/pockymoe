import type { ReactNode } from 'react';
import { Navigate, useLocation } from 'react-router-dom';
import { useHostScope } from '../lib/hostCapabilities';

/** The server remains authoritative for membership of individual thread IDs. */
export function HostScopeRouteGate({ children }: { children: ReactNode }) {
  const scope = useHostScope();
  const { pathname } = useLocation();
  if (scope && (!/^\/threads(?:\/[^/]+)?$/.test(pathname) || pathname === '/threads/import')) {
    return <Navigate replace to={`/threads/${encodeURIComponent(scope.boundThreadId)}`} />;
  }
  return children;
}
