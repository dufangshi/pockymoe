import { useEffect, useState } from 'react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { RelayDeviceDto } from '@pockymoe/shared';
import {
  deviceProtectionProgress,
  protectDevices,
  updateAndProtectDevice,
  REMOTE_PROTECTION_CHANGED_EVENT,
  type ProtectionProgress,
} from '../lib/remoteDeviceProtection';
import { ENROLLMENT_CHANGED_EVENT } from '../lib/pendingEnrollmentStore';
import { FormDialog } from './FormDialog';

export function ProtectDevicesButton({
  devices,
  all = false,
}: {
  devices: readonly RelayDeviceDto[];
  all?: boolean;
}) {
  useI18n();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const title = translate(
    all ? 'devices.protectAllDevices' : 'devices.protectRemotely',
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
              void protectDevices(devices)
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

const phaseLabel = (phase: ProtectionProgress['phase']) =>
  (
    ({
      queued: 'devices.remoteProtectionQueued',
      'update-required': 'devices.remoteProtectionUpdateRequired',
      updating: 'devices.remoteProtectionUpdating',
      binding: 'devices.remoteProtectionBinding',
      certifying: 'devices.remoteProtectionCertifying',
      complete: 'devices.remoteProtectionComplete',
      'already-bound': 'devices.remoteProtectionAlreadyBound',
      error: 'devices.remoteProtectionFailed',
    }) as const
  )[phase];

export function RemoteProtectionStatus({ device }: { device: RelayDeviceDto }) {
  useI18n();
  const [state, setState] = useState<ProtectionProgress>();
  const [busy, setBusy] = useState(false);
  const [actionFailed, setActionFailed] = useState(false);
  useEffect(() => {
    let active = true,
      generation = 0;
    const update = () => {
      const current = ++generation;
      void deviceProtectionProgress(device.id)
        .then((next) => {
          if (active && current === generation) setState(next);
        })
        .catch(() => {
          if (active && current === generation) setState(undefined);
        });
    };
    const events = [
      ENROLLMENT_CHANGED_EVENT,
      REMOTE_PROTECTION_CHANGED_EVENT,
      'pockymoe-account-logout',
    ];
    update();
    for (const event of events) window.addEventListener(event, update);
    return () => {
      active = false;
      for (const event of events) window.removeEventListener(event, update);
    };
  }, [device.id]);
  if (!state) return null;
  const failed = state.phase === 'error';
  const error = failed ? state.error : undefined;
  const label =
    error === 'account-mismatch'
      ? 'devices.remoteProtectionAccountMismatch'
      : error === 'update-failed'
        ? 'devices.remoteProtectionUpdateFailed'
        : phaseLabel(state.phase);
  const updateRequired =
    state.phase === 'update-required' || error === 'update-failed';
  return (
    <div className="mt-2 space-y-2 text-sm">
      <p role={failed ? 'alert' : 'status'}>{translate(label)}</p>
      {actionFailed && (
        <p role="alert">{translate('devices.remoteProtectionFailed')}</p>
      )}
      {(updateRequired || failed) && (
        <button
          type="button"
          className="relay-button-secondary px-3 py-2"
          disabled={busy || !device.connected}
          onClick={() => {
            setBusy(true);
            setActionFailed(false);
            void (
              updateRequired
                ? updateAndProtectDevice(device.id)
                : protectDevices([device])
            )
              .catch(() => setActionFailed(true))
              .finally(() => setBusy(false));
          }}
        >
          {translate(
            busy
              ? 'devices.starting'
              : updateRequired
                ? 'devices.updateAndProtect'
                : 'devices.retryEnrollment',
          )}
        </button>
      )}
    </div>
  );
}
