import { useEffect, useState } from 'react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { prepareDeviceEnrollment } from '../lib/deviceEnrollment';
import type { RelayDeviceDto } from '@pockymoe/shared';
import {
  deviceProtectionProgress,
  REMOTE_PROTECTION_CHANGED_EVENT,
} from '../lib/remoteDeviceProtection';
import { ENROLLMENT_CHANGED_EVENT } from '../lib/pendingEnrollmentStore';
import {
  ProtectDevicesButton,
  RemoteProtectionStatus,
} from './RemoteProtectionControls';
import {
  deviceBindCommand,
  type SupervisorPlatform,
} from '../lib/deviceSetupCommands';

/** Binding is a local command and must also be available while the device is offline. */
export function DeviceBindingPanel({
  deviceId,
  repair = false,
  device,
}: {
  deviceId: string;
  repair?: boolean;
  device?: RelayDeviceDto;
}) {
  useI18n();
  const [commands, setCommands] =
    useState<Record<SupervisorPlatform, string>>();
  const [platform, setPlatform] = useState<SupervisorPlatform>('unix');
  const command = commands?.[platform];
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string>();
  const [mode, setMode] = useState<'loading' | 'choice' | 'local' | 'remote'>(
    'loading',
  );
  const [localRequested, setLocalRequested] = useState(false);
  useEffect(() => {
    let active = true,
      generation = 0;
    const refresh = () => {
      const current = ++generation;
      if (!device && !repair) {
        setMode('local');
        return;
      }
      void deviceProtectionProgress(deviceId)
        .then((progress) => {
          if (!active || current !== generation) return;
          setMode(
            progress?.phase === 'local-command-pending'
              ? 'local'
              : progress &&
                  ![
                    'complete',
                    'already-bound',
                    'identity-changed',
                    'certificate-missing',
                  ].includes(progress.phase)
                ? 'remote'
                : repair || localRequested
                  ? 'local'
                  : 'choice',
          );
        })
        .catch(() => {
          if (active && current === generation)
            setError(translate('devices.enrollmentFailed'));
        });
    };
    refresh();
    const events = [ENROLLMENT_CHANGED_EVENT, REMOTE_PROTECTION_CHANGED_EVENT];
    for (const event of events) window.addEventListener(event, refresh);
    return () => {
      active = false;
      for (const event of events) window.removeEventListener(event, refresh);
    };
  }, [deviceId, !!device, repair, localRequested]);
  useEffect(() => {
    let active = true;
    setCommands(undefined);
    setCopied(false);
    setError(undefined);
    if (mode !== 'local') return;
    const preparing = repair
      ? prepareDeviceEnrollment(deviceId, { fresh: true })
      : prepareDeviceEnrollment(deviceId);
    void preparing
      .then((binding) => {
        if (active)
          setCommands({
            unix: deviceBindCommand(binding),
            windows: deviceBindCommand(binding, location.origin, 'windows'),
          });
      })
      .catch((e: unknown) => {
        if (active)
          setError(
            e instanceof Error
              ? e.message
              : translate('devices.enrollmentFailed'),
          );
      });
    return () => {
      active = false;
    };
  }, [deviceId, repair, mode]);
  return (
    <div className="space-y-3 text-sm">
      <p>{translate('devices.protectDeviceHelp')}</p>
      {device && mode === 'choice' && !repair && (
        <>
          <ProtectDevicesButton devices={[device]} />
          <button
            type="button"
            className="relay-button-secondary px-3 py-2"
            onClick={() => setLocalRequested(true)}
          >
            {translate('devices.showLocalBindCommand')}
          </button>
        </>
      )}
      {device && <RemoteProtectionStatus device={device} />}
      {error && <p role="alert">{error}</p>}
      {(mode === 'loading' || mode === 'local') && !command && !error && (
        <p role="status">{translate('auth.ownerPreparing')}</p>
      )}
      {command && (
        <>
          <div
            role="group"
            aria-label={translate('devices.supervisorCommandPlatform')}
          >
            {(['unix', 'windows'] as const).map((value) => (
              <button
                key={value}
                type="button"
                aria-pressed={platform === value}
                className="relay-button-secondary px-3 py-2"
                onClick={() => {
                  setPlatform(value);
                  setCopied(false);
                }}
              >
                {translate(
                  value === 'unix'
                    ? 'devices.platformUnix'
                    : 'devices.platformWindows',
                )}
              </button>
            ))}
          </div>
          <pre className="whitespace-pre-wrap break-all text-xs">{command}</pre>
          <button
            className="relay-button-secondary px-3 py-2"
            onClick={() => {
              void navigator.clipboard
                .writeText(command)
                .then(() => setCopied(true))
                .catch(() =>
                  setError(
                    translate(
                      'devices.clipboardAccessIsUnavailableCheckBrowserPermissions',
                    ),
                  ),
                );
            }}
          >
            {translate(
              copied ? 'devices.setupCommandCopied' : 'devices.copyBindCommand',
            )}
          </button>
          <p>{translate('devices.waitingForConnection')}</p>
        </>
      )}
    </div>
  );
}
