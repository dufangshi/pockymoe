import { Navigate, useLocation } from 'react-router-dom';
import { devicesHref } from '../lib/devicesRoute';

/**
 * `/relay-devices` was the phone's device page. Every surface now uses the device
 * center; shipped links (the native shells' included) are replaced there with
 * their query, so `?tab=shared&dir=out`, `?shareDevice=` and `?action=` still work.
 */
export function DevicesEntryRoute() {
  const location = useLocation();
  return <Navigate replace to={devicesHref(location.search) + location.hash} />;
}
