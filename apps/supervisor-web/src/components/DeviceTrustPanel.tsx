import { getLocale, translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useRef, useState } from 'react';
import {
  assertOwner,
  clearOwnerSession,
  currentOwnerSession,
  fetchOwnerCredentials,
  OwnerTrustError,
} from '../lib/ownerSession';
import {
  addCredentialChallenge,
  certificateChallenge,
  type DeviceCertificate,
  type OwnerCredential,
} from '../lib/ownerTrustCrypto';
import { appendDeviceCertificate } from '../lib/ownerDeviceTrust';
import { deviceTrustRequest, type DeviceTrust } from '../lib/deviceTrust';
import { securityError } from '../lib/relaySecurity';
import { OwnerUnlockButton } from './OwnerUnlockPrompt';
import { notifyOwnerUnlocked } from '../lib/ownerUnlockRecovery';

export function DeviceTrustPanel({ deviceId }: { deviceId: string }) {
  useI18n();
  const deviceRef = useRef(deviceId);
  deviceRef.current = deviceId;
  const revision = useRef(0);
  const [trust, setTrust] = useState<DeviceTrust>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [available, setAvailable] = useState<OwnerCredential[]>([]);
  const [accountCredentials, setAccountCredentials] = useState<OwnerCredential[]>([]);
  const [selected, setSelected] = useState('');
  const [add, setAdd] = useState<() => ReturnType<typeof assertOwner>>();
  const [certify, setCertify] = useState<() => Promise<DeviceCertificate>>();
  const [certificateToUpload, setCertificateToUpload] =
    useState<DeviceCertificate>();
  const [generation, setGeneration] = useState(0);
  async function load() {
    const ticket = ++revision.current;
    const data = await deviceTrustRequest<DeviceTrust>(deviceId);
    const records = data.paired ? await fetchOwnerCredentials() : [];
    if (ticket !== revision.current || deviceRef.current !== deviceId) return;
    setTrust(data);
    setAccountCredentials(records);
    setError(undefined);
    setAvailable(
      records.filter(
        (r) => !data.credentials.some((c) => c.credentialId === r.credentialId),
      ),
    );
  }
  useEffect(() => {
    let active = true;
    setTrust(undefined);
    void load().catch((e: unknown) => {
      if (active) setError(securityError(e));
    });
    return () => {
      active = false;
      revision.current++;
    };
  }, [deviceId, generation]);
  useEffect(() => {
    let active = true;
    setAdd(undefined);
    const record = available.find((r) => r.credentialId === selected);
    if (record)
      void addCredentialChallenge(deviceId, record)
        .then((challenge) => {
          if (active) setAdd(() => () => assertOwner([record], challenge));
        })
        .catch((e: unknown) => {
          if (active) setError(securityError(e));
        });
    return () => {
      active = false;
    };
  }, [deviceId, selected, available]);
  async function mutate(path: string, method: string, body?: unknown) {
    setBusy(true);
    setError(undefined);
    try {
      const owner = await currentOwnerSession();
      await deviceTrustRequest(deviceId, path, method, body);
      if (
        (path === `sessions/${owner?.sessionId}` ||
          path ===
            `credentials/${owner?.registration.assertion.credentialId}`) &&
        method === 'DELETE'
      )
        await clearOwnerSession();
      await load();
    } catch (e) {
      setError(securityError(e));
    } finally {
      setBusy(false);
    }
  }
  function credentialLabel(record: OwnerCredential) {
    // Match the public record to the authenticated device's credential before
    // using its current account name (the device may hold an older name).
    const account = accountCredentials.find(candidate =>
      candidate.credentialId === record.credentialId &&
      candidate.publicKey === record.publicKey && candidate.algorithm === record.algorithm &&
      candidate.rpId === record.rpId && candidate.origin === record.origin);
    return account?.name?.trim() || record.name?.trim() || shortId(record.credentialId);
  }
  return (
    <div className="owner-trust-panel space-y-4 text-sm">
      {error && <p role="alert">{error}</p>}
      {!trust && !error && (
        <p role="status">{translate('auth.ownerPreparing')}</p>
      )}
      {trust && (
        <>
          <p className="font-semibold">
            {translate(
              trust.paired ? 'auth.ownerPaired' : 'auth.ownerRelayTrusted',
            )}
          </p>
          {trust.strict && <div className="space-y-1">
            <p>{translate('auth.ownerStrict')}</p>
            <p className="text-xs text-[var(--theme-fg-muted)]">{translate('auth.ownerPortVisibility')}</p>
          </div>}
          {!trust.paired ? (
            <p>{translate('auth.ownerPairInstructions')}</p>
          ) : (
            <>
              <h3 className="font-semibold">
                {translate('auth.ownerCredentials')}
              </h3>
              <ul className="space-y-3">
                {trust.credentials.map((record) => (
                  <li
                    key={record.credentialId}
                    className="owner-trust-row"
                  >
                    <span className="min-w-0 break-words" title={record.credentialId}>
                      {credentialLabel(record)}
                    </span>
                    <button
                      className="relay-button-secondary px-3 py-2"
                      disabled={busy || trust.credentials.length <= 1}
                      onClick={() =>
                        void mutate(
                          `credentials/${encodeURIComponent(record.credentialId)}`,
                          'DELETE',
                        )
                      }
                    >
                      {translate('auth.ownerRemove')}
                    </button>
                  </li>
                ))}
              </ul>
              {trust.credentials.length === 1 && (
                <p className="text-xs text-[var(--theme-fg-muted)]">
                  {translate('auth.ownerLastCredential')}
                </p>
              )}
              <h3 className="font-semibold">
                {translate('auth.ownerSessions')}
              </h3>
              <ul className="space-y-3">
                {trust.sessions.map((session) => (
                  <li
                    key={session.sessionId}
                    className="owner-trust-row"
                  >
                    <span className="min-w-0">
                      <span className="flex flex-wrap items-center gap-2">
                        <code className="text-xs" title={session.sessionId}>
                          {shortId(session.sessionId)}
                        </code>
                        {session.current && (
                          <span className="owner-trust-browser-badge">
                            {translate('auth.ownerCurrent')}
                          </span>
                        )}
                      </span>
                      <time className="mt-1 block text-xs text-[var(--theme-fg-muted)]" dateTime={new Date(session.expiresAt).toISOString()}>
                        {translate('auth.ownerExpires', { value1: new Date(session.expiresAt).toLocaleString(getLocale()) })}
                      </time>
                    </span>
                    <button
                      className="relay-button-secondary px-3 py-2"
                      disabled={busy}
                      onClick={() =>
                        void mutate(
                          `sessions/${encodeURIComponent(session.sessionId)}`,
                          'DELETE',
                        )
                      }
                    >
                      {translate('auth.ownerRevoke')}
                    </button>
                  </li>
                ))}
              </ul>
              <h3 className="font-semibold">{translate('auth.ownerAdd')}</h3>
              <p className="text-[var(--theme-fg-muted)]">{translate('auth.ownerRegisterFirst')}</p>
              <a className="owner-trust-account-link" href="/relay-account">
                {translate('auth.ownerAccountSecurity')}
              </a>
              {available.length ? (
                <>
                  <select
                    className="w-full rounded border border-[var(--theme-border)] bg-[var(--theme-panel)] p-2"
                    aria-label={translate('auth.ownerSelect')}
                    value={selected}
                    onChange={(e) => setSelected(e.target.value)}
                  >
                    <option value="">{translate('auth.ownerSelect')}</option>
                    {available.map((r) => (
                      <option key={r.credentialId} value={r.credentialId}>
                        {r.name || shortId(r.credentialId)}
                      </option>
                    ))}
                  </select>
                  <button
                    className="relay-button-secondary px-3 py-2"
                    disabled={!add || busy}
                    onClick={() => {
                      if (!add) return;
                      const proof = add();
                      setBusy(true);
                      void proof
                        .then(async (value) => {
                          await deviceTrustRequest(
                            deviceId,
                            'credentials',
                            'POST',
                            value,
                          );
                          const base = trust.certificate;
                          if (!base)
                            throw new OwnerTrustError(
                              'ownerCertificateUnavailable',
                            );
                          const challenge = await certificateChallenge(
                            deviceId,
                            base.identityKey,
                            base.pairingId,
                          );
                          setCertify(() => async () => {
                            const { assertion } = await assertOwner(
                              [value.credential],
                              challenge,
                            );
                            return {
                              version: 1 as const,
                              deviceId,
                              identityKey: base.identityKey,
                              pairingId: base.pairingId,
                              assertion,
                            };
                          });
                          await load();
                        })
                        .catch((e: unknown) => setError(securityError(e)))
                        .finally(() => setBusy(false));
                    }}
                  >
                    {translate('auth.ownerAdd')}
                  </button>
                </>
              ) : null}
              {certify && (
                <div className="space-y-2">
                  <p>{translate('auth.ownerCertifyHelp')}</p>
                  <button
                    className="relay-button-primary px-3 py-2"
                    disabled={busy}
                    onClick={() => {
                      const proof = certify();
                      setBusy(true);
                      setError(undefined);
                      void proof
                        .then(async (certificate) => {
                          setCertificateToUpload(certificate);
                          await appendDeviceCertificate(certificate);
                          setCertify(undefined);
                          setCertificateToUpload(undefined);
                        })
                        .catch((e: unknown) => setError(securityError(e)))
                        .finally(() => setBusy(false));
                    }}
                  >
                    {translate('auth.ownerCertify')}
                  </button>
                </div>
              )}
              {certificateToUpload && (
                <button
                  className="relay-button-secondary px-3 py-2"
                  disabled={busy}
                  onClick={() => {
                    setBusy(true);
                    setError(undefined);
                    void appendDeviceCertificate(certificateToUpload)
                      .then(() => {
                        setCertificateToUpload(undefined);
                        setCertify(undefined);
                      })
                      .catch((e: unknown) => setError(securityError(e)))
                      .finally(() => setBusy(false));
                  }}
                >
                  {translate('auth.ownerSaveCertificate')}
                </button>
              )}
            </>
          )}
        </>
      )}
      {(trust?.paired || error) && (
        <OwnerUnlockButton
          renew
          onUnlocked={() => {
            setGeneration((g) => g + 1);
            notifyOwnerUnlocked([deviceId]);
          }}
        />
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
function shortId(id: string) {
  return id.length > 8 ? `${id.slice(0, 8)}…` : id;
}
