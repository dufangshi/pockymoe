import { useEffect, useState } from 'react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { RelayDeviceDto } from '@pockymoe/shared';
import {
  deviceProtectionProgress,
  protectDevices,
  canSwitchLocalEnrollmentToRemote,
  switchLocalEnrollmentToRemote,
  updateAndProtectDevice,
  REMOTE_PROTECTION_CHANGED_EVENT,
  type ProtectionProgress,
} from '../lib/remoteDeviceProtection';
import { ENROLLMENT_CHANGED_EVENT } from '../lib/pendingEnrollmentStore';
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
      'identity-changed': 'auth.accountDeviceIdentityChanged',
      'certificate-missing': 'devices.certificateMissingHelp',
      'local-command-pending': 'devices.localCommandPending',
      error: 'devices.remoteProtectionFailed',
    }) as const
  )[phase];

export function RemoteProtectionStatus({ device }: { device: RelayDeviceDto }) {
  useI18n();
  const [state, setState] = useState<ProtectionProgress>();
  const [busy, setBusy] = useState(false);
  const [actionFailed, setActionFailed] = useState(false);
  const [canSwitch, setCanSwitch] = useState(false);
  useEffect(() => {
    let active = true,
      generation = 0;
    const update = () => {
      const current = ++generation;
      void deviceProtectionProgress(device.id)
        .then(async (next) => {
          if (!active || current !== generation) return;
          setState(next);
          setCanSwitch(false);
          if (next?.phase === 'local-command-pending' && device.connected) {
            const eligible = await canSwitchLocalEnrollmentToRemote(device);
            if (active && current === generation) setCanSwitch(eligible);
          }
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
  }, [device.id, device.connected, device.ownerUserId]);
  if (!state) return null;
  const pinnedError =
    state.phase === 'identity-changed' || state.phase === 'certificate-missing';
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
      <p role={failed || pinnedError ? 'alert' : 'status'}>
        {translate(label)}
      </p>
      {actionFailed && (
        <p role="alert">{translate('devices.remoteProtectionFailed')}</p>
      )}
      {state.phase === 'local-command-pending' &&
        device.connected &&
        canSwitch && <ProtectDevicesButton devices={[device]} replaceLocal />}
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
