import { getLocale, translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState } from 'react';
import { boundDevices, currentAccount } from '../lib/accountVault';
import {
  deviceProtectionState,
  listPendingEnrollments,
  prepareDeviceEnrollment,
} from '../lib/deviceEnrollment';
import { deviceBindCommand } from '../lib/deviceSetupCommands';
import { deviceTrustRequest, type DeviceTrust } from '../lib/deviceTrust';
import { protectionLabel } from '../lib/useDeviceProtection';

export function DeviceTrustPanel({ deviceId }: { deviceId: string }) {
  useI18n();
  const [trust, setTrust] = useState<DeviceTrust>();
  const [state, setState] =
    useState<Awaited<ReturnType<typeof deviceProtectionState>>>('unknown');
  const [error, setError] = useState<string>();
  const [command, setCommand] = useState<string>();
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState(false);
  const [generation, setGeneration] = useState(0);
  useEffect(() => {
    let active = true;
    const controller = new AbortController();
    setTrust(undefined);
    setError(undefined);
    setCommand(undefined);
    setCopied(false);
    void (async () => {
      const data = await deviceTrustRequest<DeviceTrust>(
        deviceId,
        '',
        'GET',
        undefined,
        controller.signal,
      );
      const account = await currentAccount();
      const [bound, pending] = await Promise.all([
        boundDevices(),
        listPendingEnrollments(account),
      ]);
      const next = await deviceProtectionState(
        deviceId,
        {
          identityKey: data.certificate?.identityKey ?? '',
          certificate: data.certificate,
          trust: data,
        },
        pending,
        bound,
        account,
      );
      if (active) {
        setTrust(data);
        setState(next);
      }
    })().catch((e: unknown) => {
      if (active)
        setError(
          e instanceof Error
            ? e.message
            : translate('devices.enrollmentFailed'),
        );
    });
    return () => {
      active = false;
      controller.abort();
    };
  }, [deviceId, generation]);
  async function protect() {
    setBusy(true);
    setError(undefined);
    try {
      setCommand(deviceBindCommand(await prepareDeviceEnrollment(deviceId)));
    } catch (e) {
      setError(
        e instanceof Error ? e.message : translate('devices.enrollmentFailed'),
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="owner-trust-panel space-y-4 text-sm">
      {error && <p role="alert">{error}</p>}
      {!trust && !error && (
        <p role="status">{translate('auth.ownerPreparing')}</p>
      )}
      {trust && (
        <>
          <p
            className="font-semibold"
            role={state === 'certificate-missing' ? 'alert' : undefined}
          >
            {translate(protectionLabel(state))}
          </p>
          {state === 'certificate-missing' && (
            <p>{translate('devices.certificateMissingHelp')}</p>
          )}
          {trust.strict && <p>{translate('auth.ownerStrict')}</p>}
          {trust.accountKeyId && (
            <p>
              {translate('devices.boundAccountFingerprint')}{' '}
              <code>{trust.accountKeyId.slice(0, 12)}</code>
            </p>
          )}
          {!trust.bound && (
            <>
              <p>{translate('devices.protectDeviceHelp')}</p>
              <button
                className="relay-button-secondary px-3 py-2"
                disabled={busy}
                onClick={() => void protect()}
              >
                {translate('devices.protectDevice')}
              </button>
            </>
          )}
          {command && (
            <>
              <pre className="whitespace-pre-wrap break-all text-xs">
                {command}
              </pre>
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
                  copied
                    ? 'devices.setupCommandCopied'
                    : 'devices.copyBindCommand',
                )}
              </button>
              <p>{translate('devices.waitingForConnection')}</p>
            </>
          )}
          {trust.bound && (
            <>
              <h3 className="font-semibold">
                {translate('devices.registeredBrowsers')}
              </h3>
              <ul className="space-y-3">
                {trust.browsers.map((browser) => (
                  <li key={browser.browserId} className="owner-trust-row">
                    <span className="min-w-0">
                      <code title={browser.browserId}>
                        {browser.browserId.slice(0, 12)}
                      </code>
                      {browser.current && (
                        <span className="owner-trust-browser-badge">
                          {translate('auth.ownerCurrent')}
                        </span>
                      )}
                      <span className="block">
                        {translate(
                          browser.role === 'owner'
                            ? 'devices.ownerBrowser'
                            : 'devices.sharedBrowser',
                        )}
                      </span>
                      {browser.userId && (
                        <span className="block">{browser.userId}</span>
                      )}
                      <span className="block text-xs">
                        {translate('devices.browserRegisteredAt', {
                          value1: new Date(browser.registeredAt).toLocaleString(
                            getLocale(),
                          ),
                        })}
                      </span>
                      <span className="block text-xs">
                        {translate('devices.browserLastSeenAt', {
                          value1: new Date(browser.lastSeenAt).toLocaleString(
                            getLocale(),
                          ),
                        })}
                      </span>
                    </span>
                  </li>
                ))}
              </ul>
            </>
          )}
        </>
      )}
      <button
        className="relay-button-secondary px-3 py-2"
        disabled={busy}
        onClick={() => setGeneration((g) => g + 1)}
      >
        {translate('auth.ownerRefresh')}
      </button>
    </div>
  );
}
