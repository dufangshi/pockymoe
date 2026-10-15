import { useState } from 'react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { RelayDeviceDto } from '@pockymoe/shared';
import {
  protectDevices,
  switchLocalEnrollmentToRemote,
} from '../lib/remoteDeviceProtection';
import { FormDialog } from './FormDialog';

export function ProtectDevicesButton({
  devices,
  all = false,
  replaceLocal = false,
}: {
  devices: readonly RelayDeviceDto[];
  all?: boolean;
  replaceLocal?: boolean;
}) {
  useI18n();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const title = translate(
    replaceLocal
      ? 'devices.useOneClickProtectionInstead'
      : all
        ? 'devices.protectAllDevices'
        : 'devices.protectRemotely',
  );
  return (
    <>
      <button
        type="button"
        className="relay-button-secondary min-h-11 px-3 py-2"
        disabled={!devices.length}
        onClick={() => {
          setError(false);
          setOpen(true);
        }}
      >
        {title}
      </button>
      {open && (
        <FormDialog title={title} busy={busy} onClose={() => setOpen(false)}>
          <p className="text-sm">
            {translate('devices.remoteProtectionTradeoff')}
          </p>
          <p className="text-sm">
            {translate('devices.remoteProtectionLocalOption')}
          </p>
          <p className="text-sm">
            {translate('devices.remoteProtectionUpdatesHelp')}
          </p>
          {replaceLocal && (
            <p className="text-sm">
              {translate('devices.replaceLocalEnrollmentHelp')}
            </p>
          )}
          {error && (
            <p role="alert">{translate('devices.remoteProtectionFailed')}</p>
          )}
          <button
            type="button"
            className="relay-button-primary min-h-11 px-3 py-2"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              setError(false);
              void (
                replaceLocal
                  ? switchLocalEnrollmentToRemote(devices[0]!)
                  : protectDevices(devices)
              )
                .then(() => setOpen(false))
                .catch(() => setError(true))
                .finally(() => setBusy(false));
            }}
          >
            {translate(
              busy ? 'devices.starting' : 'devices.confirmRemoteProtection',
            )}
          </button>
        </FormDialog>
      )}
    </>
  );
}
