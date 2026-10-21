import { OWNER_UNLOCKED_EVENT } from '../lib/ownerUnlockRecovery';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState } from 'react';
import type { RelayPortalSummaryDto } from '@pockymoe/shared';
import {
  isProtectedSharingDevice,
  sharingGrantDevices,
  sharingOwnerAccountKeyId,
  type PublicAccount,
} from '../lib/sharedDeviceTrust';
import { currentAccount } from '../lib/accountVault';
import { clearTransportKeyCache } from '../lib/relayTransportCrypto';
import { sharedInviteUrl } from '../lib/sharedInvite';
import {
  confirmRecipient,
  recipientVerified,
  safetyCode,
  recipientAccount,
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
  const [account, setAccount] = useState<PublicAccount>();
  const [code, setCode] = useState('');
  const [verified, setVerified] = useState(false);
  const [entry, setEntry] = useState<Reconciliation>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const [copied, setCopied] = useState(false);
  const [inviteOwnerKeyId, setInviteOwnerKeyId] = useState<string>();
  useEffect(() => {
    const refresh = () => setRevision((r) => r + 1);
    window.addEventListener(OWNER_UNLOCKED_EVENT, refresh);
    return () =>
      window.removeEventListener(OWNER_UNLOCKED_EVENT, refresh);
  }, []);
  useEffect(() => {
    let active = true;
    setError(undefined);
    setEntry(undefined);
    setAccount(undefined);
    setInviteOwnerKeyId(undefined);
    if (row.deviceConnected !== false) void sharingOwnerAccountKeyId(row.deviceId)
      .then((value) => { if (active) setInviteOwnerKeyId(value); }).catch(() => {});
    void (async () => {
      if (!(await isProtectedSharingDevice(row.deviceId))) return;
      const recipient = await recipientAccount(kind, row.id);
      const [nextCode, confirmed] = await Promise.all([
        safetyCode(recipient.accountKeyId),
        recipientVerified(recipient, row.targetUserId),
      ]);
      if (!active) return;
      setAccount(recipient);
      setCode(nextCode);
      setVerified(confirmed);
      if (row.deviceConnected === false) {
        setError(translate('sharing.protectedOnline'));
        return;
      }
      const grants = await deviceGrants(row.deviceId);
      if (active)
        setEntry(
          reconcileGrants([{ kind, row, account: recipient }], grants)[0],
        );
    })().catch((e: unknown) => {
      if (active) {
        setError(
          e instanceof Error ? e.message : translate('sharing.deviceCertificateInvalid'),
        );
      }
    });
    return () => {
      active = false;
    };
  }, [kind, row, revision]);
  function update() {
    setBusy(true);
    setError(undefined);
    clearTransportKeyCache(row.deviceId);
    void syncDeviceGrant(kind, row)
      .then(() => setRevision((r) => r + 1))
      .catch((error: unknown) => setError(error instanceof Error ? error.message : translate('sharing.deviceCertificateInvalid')))
      .finally(() => setBusy(false));
  }
  return (
    <div className="space-y-2 py-2 text-xs">
      {account && (
        <>
          <p>
            {translate('sharing.safetyCode')}: <code>{code}</code> ·{' '}
            {translate(verified ? 'sharing.verified' : 'sharing.unverified')}
          </p>
          <p className="text-[var(--theme-fg-muted)]">{translate('sharing.safetyHelp')}</p>
          {!verified && (
            <button className={button} disabled={busy} onClick={() => {
              setBusy(true);
              void confirmRecipient(account, row.targetUserId)
                .then(() => setVerified(true))
                .catch((error: unknown) => setError(error instanceof Error ? error.message : translate('sharing.accountKeyMismatch')))
                .finally(() => setBusy(false));
            }}>{translate('sharing.confirmSafety')}</button>
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
        <p role="status">{translate('sharing.recipientKeyChanged')}</p>
      )}
      {(entry?.state !== 'active' && account) && (
        <button
          className={button}
          disabled={
            busy ||
            !account ||
            row.deviceConnected === false
          }
          onClick={update}
        >
          {translate(
            entry?.state === 'changed' || entry?.state === 'different'
              ? 'sharing.updateGrant'
              : 'sharing.confirmGrant',
          )}
        </button>
      )}
      {inviteOwnerKeyId && <button
        className={button}
        disabled={busy || row.deviceConnected === false}
        onClick={() => {
          setBusy(true);
          setError(undefined);
          void sharingOwnerAccountKeyId(row.deviceId)
            .then((ownerAccountKeyId) =>
              navigator.clipboard.writeText(
                sharedInviteUrl(location.origin, {
                  deviceId: row.deviceId,
                  ownerAccountKeyId,
                  ...(row.threadId ? { threadId: row.threadId }
                    : 'scope' in row && row.scope === 'workspace' && row.workspaceId ? { workspaceId: row.workspaceId } : {}),
                }),
              ),
            )
            .then(() => setCopied(true))
            .catch((e: unknown) =>
              setError(
                e instanceof Error
                  ? e.message
                  : translate('sharing.deviceCertificateInvalid'),
              ),
            )
            .finally(() => setBusy(false));
        }}
      >
        {translate(copied ? 'sharing.inviteCopied' : 'sharing.copyInvite')}
      </button>}
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
    window.addEventListener(OWNER_UNLOCKED_EVENT, refresh);
    return () =>
      window.removeEventListener(OWNER_UNLOCKED_EVENT, refresh);
  }, []);
  useEffect(() => {
    let active = true;
    void (async () => {
      const found: { deviceId: string; grant: DeviceGrant }[] = [];
      for (const deviceId of await sharingGrantDevices()) {
        const grants = await deviceGrants(deviceId);
        for (const grant of grants.filter(
          (g) =>
            g.relayRef.kind === kind &&
            (g.expiresAt === null || g.expiresAt > Date.now()),
        )) {
          const rows =
            kind === 'share' ? portal.sharedByMe : (portal.grantsByMe ?? []);
          if (
            !rows.some(
              (r) => r.deviceId === deviceId && r.id === grant.relayRef.id,
            )
          )
            found.push({ deviceId, grant });
        }
      }
      if (active) {
        setOrphans(found);
        setError(undefined);
      }
    })().catch((e: unknown) => {
      if (active)
        setError(
          e instanceof Error ? e.message : translate('sharing.deviceCertificateInvalid'),
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
                      : translate('sharing.deviceCertificateInvalid'),
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
    void currentAccount()
      .then(async (account) => {
        const next = account ? await safetyCode(account.accountKeyId) : '';
        if (active) {
          setCode(next);
          setError(undefined);
        }
      })
      .catch((e: unknown) => {
        if (active)
          setError(
            e instanceof Error ? e.message : translate('sharing.deviceCertificateInvalid'),
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
        !error && <p>{translate('sharing.accountNotConfigured')}</p>
      )}
      {error && <p role="alert">{error}</p>}
    </div>
  );
}
