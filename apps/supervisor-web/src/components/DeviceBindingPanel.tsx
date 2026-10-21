import { HostFeatureGate } from '../lib/hostCapabilities';
import { useEffect, useState } from 'react';
import { ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { prepareDeviceEnrollment } from '../lib/deviceEnrollment';
import type { RelayDeviceDto } from '@pockymoe/shared';
import {
  deviceProtectionProgress,
  REMOTE_PROTECTION_CHANGED_EVENT,
} from '../lib/remoteDeviceProtection';
import { ENROLLMENT_CHANGED_EVENT } from '../lib/pendingEnrollmentStore';
import { ProtectDevicesButton } from './RemoteProtectionControls';
import { RemoteProtectionProgress } from './DeviceProtectionStatus';
import {
  deviceBindCommand,
  type SupervisorPlatform,
} from '../lib/deviceSetupCommands';

/** Binding is a local command and must also be available while the device is offline. */
export function DeviceBindingPanel(props: { deviceId: string; repair?: boolean; device?: RelayDeviceDto }) {
  return <HostFeatureGate feature="accountBinding"><DeviceBindingContent {...props} /></HostFeatureGate>;
}

function DeviceBindingContent({
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
  // A failed read of the protection progress only matters before anything
  // was shown; later refreshes keep the last state silently.
  const [progressError, setProgressError] = useState<string>();
  const [prepareError, setPrepareError] = useState<string>();
  const [copyError, setCopyError] = useState<string>();
  const [attempt, setAttempt] = useState(0);
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
          setProgressError(undefined);
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
            setProgressError(translate('devices.enrollmentFailed'));
        });
    };
    refresh();
    const events = [ENROLLMENT_CHANGED_EVENT, REMOTE_PROTECTION_CHANGED_EVENT];
    for (const event of events) window.addEventListener(event, refresh);
    return () => {
      active = false;
      for (const event of events) window.removeEventListener(event, refresh);
    };
  }, [deviceId, !!device, repair, localRequested, attempt]);
  useEffect(() => {
    let active = true;
    setCommands(undefined);
    setCopied(false);
    setCopyError(undefined);
    setPrepareError(undefined);
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
          setPrepareError(
            e instanceof Error
              ? e.message
              : translate('devices.enrollmentFailed'),
          );
      });
    return () => {
      active = false;
    };
  }, [deviceId, repair, mode, attempt]);
  const failure = mode === 'loading' ? progressError
    : mode === 'local' && !command ? prepareError : undefined;
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
      {device && <RemoteProtectionProgress device={device} />}
      {failure && (
        <p className="flex items-center gap-1.5 text-[var(--theme-fg-muted)]">
          {translate('devices.unavailable')}
          <ErrorMark message={failure} onRetry={() => setAttempt((value) => value + 1)} />
        </p>
      )}
      {(mode === 'loading' || mode === 'local') && !command && !failure && (
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
                  setCopyError(undefined);
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
          <div className="flex items-center gap-2">
            <button
              className="relay-button-secondary px-3 py-2"
              onClick={() => {
                setCopyError(undefined);
                void navigator.clipboard
                  .writeText(command)
                  .then(() => setCopied(true))
                  .catch(() =>
                    setCopyError(
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
            {copyError && <ErrorMark message={copyError} />}
          </div>
          <p>{translate('devices.waitingForConnection')}</p>
        </>
      )}
    </div>
  );
}
