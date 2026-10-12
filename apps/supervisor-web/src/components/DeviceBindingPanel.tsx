import { useEffect, useState } from 'react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { prepareDeviceEnrollment } from '../lib/deviceEnrollment';
import { deviceBindCommand } from '../lib/deviceSetupCommands';

/** Binding is a local command and must also be available while the device is offline. */
export function DeviceBindingPanel({ deviceId }: { deviceId: string }) {
  useI18n();
  const [command, setCommand] = useState<string>();
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    let active = true;
    setCommand(undefined);
    setCopied(false);
    setError(undefined);
    void prepareDeviceEnrollment(deviceId)
      .then((binding) => {
        if (active) setCommand(deviceBindCommand(binding));
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
  }, [deviceId]);
  return (
    <div className="space-y-3 text-sm">
      <p>{translate('devices.protectDeviceHelp')}</p>
      {error && <p role="alert">{error}</p>}
      {!command && !error && (
        <p role="status">{translate('auth.ownerPreparing')}</p>
      )}
      {command && (
        <>
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
