import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState } from 'react';
import type { RelayPortalSummaryDto } from '@pockymoe/shared';
import { hasDeviceCertificate } from '../lib/ownerDeviceTrust';
import { prepareOwnerUnlock, fetchOwnerCredentials } from '../lib/ownerSession';
import type { OwnerCredential } from '../lib/ownerTrustCrypto';
import {
  sharingFingerprint,
  clearTransportKeyCache,
} from '../lib/relayTransportCrypto';
import { sharedInviteUrl } from '../lib/sharedInvite';
import {
  confirmRecipient,
  recipientVerified,
  safetyCode,
  recipientPasskeys,
  deviceGrants,
  reconcileGrants,
  syncDeviceGrant,
  deleteDeviceGrant,
  type ShareKind,
  type ShareRow,
  type Reconciliation,
  type DeviceGrant,
} from '../lib/signedSharing';

const button = 'relay-button-secondary min-h-10 px-3 py-2 text-xs';
export function SignedShareDetails({
  kind,
  row,
}: {
  kind: ShareKind;
  row: ShareRow;
}) {
  useI18n();
  const [credentials, setCredentials] = useState<OwnerCredential[]>();
  const [code, setCode] = useState('');
  const [verified, setVerified] = useState(false);
  const [entry, setEntry] = useState<Reconciliation>();
  const [unlock, setUnlock] =
    useState<Awaited<ReturnType<typeof prepareOwnerUnlock>>>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const [copied, setCopied] = useState(false);
  const [needsUnlock, setNeedsUnlock] = useState(false);
  useEffect(() => {
    const refresh = () => setRevision((r) => r + 1);
    window.addEventListener('remote-codex-owner-unlocked', refresh);
    return () =>
      window.removeEventListener('remote-codex-owner-unlocked', refresh);
  }, []);
  useEffect(() => {
    let active = true;
    setError(undefined);
    setEntry(undefined);
    setUnlock(undefined);
    setCredentials(undefined);
    setNeedsUnlock(false);
    void (async () => {
      if (!(await hasDeviceCertificate(row.deviceId))) return;
      const records = await recipientPasskeys(kind, row.id);
      const [nextCode, confirmed] = await Promise.all([
        safetyCode(records),
        recipientVerified(records),
      ]);
      if (!active) return;
      setCredentials(records);
      setCode(nextCode);
      setVerified(confirmed);
      if (row.deviceConnected === false) {
        setError(translate('sharing.pairedOnline'));
        return;
      }
      const prepare = await prepareOwnerUnlock();
      if (active) setUnlock(() => prepare);
      const grants = await deviceGrants(row.deviceId);
      if (active)
        setEntry(
          reconcileGrants([{ kind, row, credentials: records }], grants)[0],
        );
    })().catch((e: unknown) => {
      if (active) {
        setNeedsUnlock(
          e instanceof Error &&
            'code' in e &&
            e.code === 'owner_session_required',
        );
        setError(
          e instanceof Error ? e.message : translate('auth.ownerTrustFailed'),
        );
      }
    });
    return () => {
      active = false;
    };
  }, [kind, row, revision]);
  function update() {
    if (!unlock) return;
    const proof = unlock(); // WebAuthn starts directly in this user gesture.
    setBusy(true);
    setError(undefined);
    void proof
      .then(async () => {
        clearTransportKeyCache(row.deviceId);
        await syncDeviceGrant(kind, row);
        setRevision((r) => r + 1);
      })
      .catch((e: unknown) =>
        setError(
          e instanceof Error ? e.message : translate('auth.ownerTrustFailed'),
        ),
      )
      .finally(() => setBusy(false));
  }
  return (
    <div className="space-y-2 py-2 text-xs">
      {credentials && (
        <>
          {credentials.length ? (
            <>
              <p>
                {translate('sharing.safetyCode')}: <code>{code}</code> ·{' '}
                {translate(
                  verified ? 'sharing.verified' : 'sharing.unverified',
                )}
              </p>
              <p className="text-[var(--theme-fg-muted)]">
                {translate('sharing.safetyHelp')}
              </p>
              {!verified && (
                <button
                  className={button}
                  disabled={busy}
                  onClick={() => {
                    setBusy(true);
                    void confirmRecipient(credentials)
                      .then(() => setVerified(true))
                      .catch((e: unknown) =>
                        setError(
                          e instanceof Error
                            ? e.message
                            : translate('auth.ownerTrustFailed'),
                        ),
                      )
                      .finally(() => setBusy(false));
                  }}
                >
                  {translate('sharing.confirmSafety')}
                </button>
              )}
            </>
          ) : (
            <p>{translate('sharing.recipientNeedsPasskey')}</p>
          )}
        </>
      )}
      {entry?.state === 'missing' && (
        <p role="status">{translate('sharing.notActive')}</p>
      )}
      {entry?.state === 'different' && (
        <p role="status">{translate('sharing.permissionsDiffer')}</p>
      )}
      {entry?.state === 'changed' && (
        <p role="status">{translate('sharing.newPasskeys')}</p>
      )}
      {(needsUnlock || (entry && entry.state !== 'active')) && (
        <button
          className={button}
          disabled={
            busy ||
            !unlock ||
            !credentials?.length ||
            row.deviceConnected === false
          }
          onClick={update}
        >
          {translate(
            entry?.state === 'changed' || entry?.state === 'different'
              ? 'sharing.updateGrant'
              : 'sharing.confirmPasskey',
          )}
        </button>
      )}
      <button
        className={button}
        disabled={busy || row.deviceConnected === false}
        onClick={() => {
          setBusy(true);
          setError(undefined);
          void sharingFingerprint(row.deviceId, row.threadId ?? undefined)
            .then((fingerprint) =>
              navigator.clipboard.writeText(
                sharedInviteUrl(location.origin, {
                  deviceId: row.deviceId,
                  identity: fingerprint,
                  ...(row.threadId ? { threadId: row.threadId } : {}),
                }),
              ),
            )
            .then(() => setCopied(true))
            .catch((e: unknown) =>
              setError(
                e instanceof Error
                  ? e.message
                  : translate('auth.ownerTrustFailed'),
              ),
            )
            .finally(() => setBusy(false));
        }}
      >
        {translate(copied ? 'sharing.inviteCopied' : 'sharing.copyInvite')}
      </button>
      {error && <p role="alert">{error}</p>}
    </div>
  );
}

export function OrphanDeviceGrants({
  portal,
  kind,
}: {
  portal: RelayPortalSummaryDto;
  kind: ShareKind;
}) {
  useI18n();
  const [orphans, setOrphans] = useState<
    { deviceId: string; grant: DeviceGrant }[]
  >([]);
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    const refresh = () => setRevision((r) => r + 1);
    window.addEventListener('remote-codex-owner-unlocked', refresh);
    return () =>
      window.removeEventListener('remote-codex-owner-unlocked', refresh);
  }, []);
  useEffect(() => {
    let active = true;
    void (async () => {
      const found: { deviceId: string; grant: DeviceGrant }[] = [];
      for (const device of portal.devices.filter((d) => d.connected)) {
        if (!(await hasDeviceCertificate(device.id))) continue;
        const grants = await deviceGrants(device.id);
        for (const grant of grants.filter(
          (g) =>
            g.relayRef.kind === kind &&
            (g.expiresAt === null || g.expiresAt > Date.now()),
        )) {
          const rows =
            kind === 'share' ? portal.sharedByMe : (portal.grantsByMe ?? []);
          if (
            !rows.some(
              (r) => r.deviceId === device.id && r.id === grant.relayRef.id,
            )
          )
            found.push({ deviceId: device.id, grant });
        }
      }
      if (active) {
        setOrphans(found);
        setError(undefined);
      }
    })().catch((e: unknown) => {
      if (active)
        setError(
          e instanceof Error ? e.message : translate('auth.ownerTrustFailed'),
        );
    });
    return () => {
      active = false;
    };
  }, [portal, kind, revision]);
  return (
    <div className="space-y-3">
      {orphans.map(({ deviceId, grant }) => (
        <div key={grant.grantId} className="rounded-lg border p-3 text-sm">
          <p>
            {grant.grantee.label} · {translate('sharing.stillActive')}
          </p>
          <button
            className={button}
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void deleteDeviceGrant(deviceId, grant.grantId)
                .then(() => setRevision((r) => r + 1))
                .catch((e: unknown) =>
                  setError(
                    e instanceof Error
                      ? e.message
                      : translate('auth.ownerTrustFailed'),
                  ),
                )
                .finally(() => setBusy(false));
            }}
          >
            {translate('sharing.revoke')}
          </button>
        </div>
      ))}
      {error && (
        <p className="text-sm" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
export function AccountSafetyCode({ revision }: { revision?: unknown }) {
  useI18n();
  const [code, setCode] = useState('');
  const [error, setError] = useState<string>();
  useEffect(() => {
    let active = true;
    void fetchOwnerCredentials()
      .then(async (records) => {
        const next = records.length ? await safetyCode(records) : '';
        if (active) {
          setCode(next);
          setError(undefined);
        }
      })
      .catch((e: unknown) => {
        if (active)
          setError(
            e instanceof Error ? e.message : translate('auth.ownerTrustFailed'),
          );
      });
    return () => {
      active = false;
    };
  }, [revision]);
  return (
    <div className="space-y-2 text-sm">
      <h3 className="font-medium">{translate('sharing.safetyCode')}</h3>
      {code ? (
        <>
          <code className="text-base">{code}</code>
          <p>{translate('sharing.safetyRecipient')}</p>
        </>
      ) : (
        !error && <p>{translate('sharing.recipientNeedsPasskey')}</p>
      )}
      {error && <p role="alert">{error}</p>}
    </div>
  );
}
