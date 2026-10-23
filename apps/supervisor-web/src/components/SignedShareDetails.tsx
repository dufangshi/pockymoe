import { OWNER_UNLOCKED_EVENT } from '../lib/ownerUnlockRecovery';
import { CopyButton, ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Hourglass, ShieldAlert, ShieldCheck, ShieldQuestion } from 'lucide-react';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
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
import { announceSharingChanged, normalizeSafetyCode } from '../lib/sharePending';
import { useRequireSafetyCode } from '../lib/sharingPreferences';
import {
  approveRelayShare,
  confirmRecipient,
  recipientVerified,
  safetyCode,
  recipientAccount,
  RecipientAccountMissingError,
  deviceGrants,
  reconcileGrants,
  syncDeviceGrant,
  deleteDeviceGrant,
  type ShareKind,
  type ShareRow,
  type Reconciliation,
  type DeviceGrant,
} from '../lib/signedSharing';
import './signed-share.css';

const button = 'relay-button-secondary min-h-10 px-3 py-2 text-xs';

/** What the owner's browser found out about one share: whether its device is
    protected, the recipient's account key (null without one), the code to
    expect, whether this browser checked it, and the device's grant. */
interface ShareCheck {
  protectedDevice: boolean;
  account: PublicAccount | null;
  code: string;
  verifiedHere: boolean;
  entry: Reconciliation | undefined;
}

/** One state, one sentence and one action (docs/account-key-trust.md#safety-code-check). */
export type ShareState =
  | 'pending' | 'missing' | 'different' | 'changed' | 'noAccount' | 'unverified' | 'verified';

/** The fields a check depends on. A portal refresh builds new row objects every
    few seconds; equal content must neither clear nor reload the details. */
export function shareSignature(kind: ShareKind, row: ShareRow) {
  const grant = row as Partial<import('@pockymoe/shared').RelayAccessGrantDto>;
  return JSON.stringify([
    kind, row.id, row.deviceId, row.deviceConnected === false, row.targetUserId, row.threadId ?? null,
    row.workspaceId ?? null, row.threadAccess, row.workspaceAccess, row.expiresAt ?? null,
    grant.scope ?? 'thread', grant.workspaceScope ?? null, grant.workspaceIds ?? null,
    grant.canCreateThreads ?? false, row.pending === true, row.verifiedAt ?? null,
  ]);
}

async function loadCheck(kind: ShareKind, row: ShareRow): Promise<ShareCheck> {
  const protectedDevice = await isProtectedSharingDevice(row.deviceId);
  let account: PublicAccount | null = null;
  try {
    account = await recipientAccount(kind, row.id);
  } catch (error) {
    if (!(error instanceof RecipientAccountMissingError)) throw error;
  }
  const [code, verifiedHere] = account
    ? await Promise.all([safetyCode(account.accountKeyId), recipientVerified(account, row.targetUserId)])
    : ['', false];
  const entry = protectedDevice && account && row.deviceConnected !== false
    ? reconcileGrants([{ kind, row, account }], await deviceGrants(row.deviceId))[0]
    : undefined;
  return { protectedDevice, account, code, verifiedHere, entry };
}

export function shareState(row: ShareRow, check: ShareCheck | undefined): ShareState | null {
  if (row.pending === true) return 'pending';
  if (!check) return null;
  if (check.protectedDevice && !check.account) return 'noAccount';
  const state = check.entry?.state;
  if (state === 'missing' || state === 'different' || state === 'changed') return state;
  if (!check.account) return null;
  return check.verifiedHere || row.verifiedAt ? 'verified' : 'unverified';
}

const STATE_ICON = { pending: Hourglass, missing: ShieldAlert, different: ShieldAlert, changed: ShieldAlert, noAccount: ShieldAlert, unverified: ShieldQuestion, verified: ShieldCheck };

/** The owner's view of one share's trust: a single status with what it means
    for the recipient and the one action that fixes it. */
export function SignedShareDetails({
  kind,
  row,
}: {
  kind: ShareKind;
  row: ShareRow;
}) {
  useI18n();
  const requireCode = useRequireSafetyCode();
  const signature = useMemo(() => shareSignature(kind, row), [kind, row]);
  const rowRef = useRef(row);
  rowRef.current = row;
  // The last result stays on screen while a refresh checks again.
  const [check, setCheck] = useState<ShareCheck>();
  const [loadError, setLoadError] = useState<string>();
  const [actionError, setActionError] = useState<string>();
  const [inviteError, setInviteError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const [copied, setCopied] = useState(false);
  const [inviteOwnerKeyId, setInviteOwnerKeyId] = useState<string>();
  // Approved here; the portal says so on its next refresh.
  const [approved, setApproved] = useState(false);
  useEffect(() => {
    const refresh = () => setRevision((r) => r + 1);
    window.addEventListener(OWNER_UNLOCKED_EVENT, refresh);
    return () =>
      window.removeEventListener(OWNER_UNLOCKED_EVENT, refresh);
  }, []);
  useEffect(() => {
    let active = true;
    const current = rowRef.current;
    if (current.deviceConnected !== false) void sharingOwnerAccountKeyId(current.deviceId)
      .then((value) => { if (active) setInviteOwnerKeyId(value); }).catch(() => {});
    void loadCheck(kind, current)
      .then((next) => {
        if (!active) return;
        setCheck(next);
        setLoadError(undefined);
      })
      .catch((error: unknown) => {
        if (active) setLoadError(error instanceof Error ? error.message : translate('sharing.deviceCertificateInvalid'));
      });
    return () => {
      active = false;
    };
  }, [kind, signature, revision]);
  useEffect(() => { if (row.pending !== true) setApproved(false); }, [row.pending]);

  const shown = approved && row.pending === true ? { ...row, pending: false } : row;
  const state = shareState(shown, check);
  const name = row.targetUsername;
  const device = row.deviceName;
  const offline = row.deviceConnected === false;
  const needsDevice = Boolean(check?.protectedDevice) && (state === 'pending' || state === 'missing' || state === 'different' || state === 'changed');
  const unavailable = needsDevice && offline ? translate('sharing.protectedOnline') : undefined;

  function run(action: () => Promise<unknown>) {
    setBusy(true);
    setActionError(undefined);
    clearTransportKeyCache(row.deviceId);
    void action()
      .then(() => {
        announceSharingChanged();
        setRevision((r) => r + 1);
      })
      .catch((error: unknown) => setActionError(error instanceof Error ? error.message : translate('sharing.approveFailed')))
      .finally(() => setBusy(false));
  }
  /** The code matched: remember the recipient's key, sign the device grant when
      the device needs one, then let the relay start (or record) the share. */
  function approveWithCode(typed: string) {
    const account = check?.account;
    if (!account) return;
    const signs = Boolean(check?.protectedDevice) && state !== 'unverified';
    run(async () => {
      await confirmRecipient(account, row.targetUserId);
      if (signs) await syncDeviceGrant(kind, row);
      await approveRelayShare(kind, row.id, typed);
      if (row.pending === true) setApproved(true);
    });
  }
  const sign = () => run(() => syncDeviceGrant(kind, row));
  const approveWithoutCode = () => run(async () => {
    await approveRelayShare(kind, row.id, null);
    setApproved(true);
  });

  // Until the check loads, the field is there but disabled, so nothing moves.
  const codeAction = (label: string) => check?.account || !check
    ? <SafetyCodeEntry name={name} expected={check?.code ?? ''} label={label} busy={busy || !check} unavailable={unavailable} error={actionError} onApprove={approveWithCode} />
    : null;
  const buttonAction = (label: string, onClick: () => void) => (
    <div className="signed-share-actions">
      <button className={button} disabled={busy || Boolean(unavailable)} onClick={onClick} type="button">{label}</button>
      {unavailable ? <ErrorMark kind="unavailable" message={unavailable} /> : actionError ? <ErrorMark message={actionError} /> : null}
    </div>
  );
  // Without this browser's own check, signing a missing or replaced grant asks
  // for the code when the owner keeps the check on.
  const signAction = (label: string) => check?.verifiedHere || !requireCode
    ? buttonAction(label, sign)
    : codeAction(label);

  let body: { title: string; text?: string; action?: React.ReactNode; tone: 'warn' | 'ok' | 'muted' } | null = null;
  switch (state) {
    case 'pending':
      body = check?.account || !check
        ? {
          tone: 'warn',
          title: translate('sharing.statePendingTitle', { name }),
          text: translate('sharing.statePendingText', { name }),
          action: codeAction(translate('sharing.approveWithCode')),
        }
        : {
          tone: 'warn',
          title: translate('sharing.statePendingTitle', { name }),
          text: translate('sharing.statePendingNoAccount', { name }),
          action: check.protectedDevice ? null : buttonAction(translate('sharing.approveWithoutCode'), approveWithoutCode),
        };
      break;
    case 'missing':
      body = { tone: 'warn', title: translate('sharing.stateMissingTitle', { name }), text: translate('sharing.stateMissingText', { device }), action: signAction(translate('sharing.signOntoDevice')) };
      break;
    case 'changed':
      body = { tone: 'warn', title: translate('sharing.stateChangedTitle', { name }), text: translate('sharing.stateChangedText', { name, device }), action: requireCode ? codeAction(translate('sharing.signOntoDevice')) : buttonAction(translate('sharing.signOntoDevice'), sign) };
      break;
    case 'different':
      body = { tone: 'warn', title: translate('sharing.stateDifferentTitle', { device }), text: translate('sharing.stateDifferentText', { name }), action: buttonAction(translate('sharing.signCurrentPermissions'), sign) };
      break;
    case 'noAccount':
      body = { tone: 'warn', title: translate('sharing.stateMissingTitle', { name }), text: translate('sharing.stateNoAccount', { name }) };
      break;
    case 'unverified':
      body = { tone: 'muted', title: translate('sharing.stateUnverifiedTitle'), text: translate('sharing.stateUnverifiedText', { name }), action: codeAction(translate('sharing.checkCode')) };
      break;
    case 'verified':
      body = { tone: 'ok', title: translate('sharing.stateVerified') };
      break;
  }
  const Icon = state ? STATE_ICON[state] : null;
  return (
    <div className="signed-share">
      {body && Icon ? (
        <section className="signed-share-state" data-tone={body.tone} data-state={state} aria-label={body.title}>
          <p className="signed-share-title">
            <Icon size={14} aria-hidden="true" />
            <span>{body.title}</span>
            {state === 'verified' && check?.code ? <code className="signed-share-code">{check.code}</code> : null}
          </p>
          {body.text ? <p className="signed-share-text">{body.text}</p> : null}
          {body.action}
          {state === 'pending' && requireCode ? <p className="signed-share-hint">{translate('sharing.checkOffHint')}</p> : null}
        </section>
      ) : null}
      {!check && loadError ? (
        <p className="flex items-center gap-1.5 text-[var(--theme-fg-muted)]">
          {translate('devices.unavailable')}
          <ErrorMark message={loadError} onRetry={() => setRevision((r) => r + 1)} />
        </p>
      ) : null}
      {inviteOwnerKeyId && <div className="signed-share-actions"><button
        className={button}
        disabled={busy || offline}
        type="button"
        onClick={() => {
          setBusy(true);
          setInviteError(undefined);
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
            .catch((e: unknown) => setInviteError(e instanceof Error ? e.message : translate('sharing.deviceCertificateInvalid')))
            .finally(() => setBusy(false));
        }}
      >
        {translate(copied ? 'sharing.inviteCopied' : 'sharing.copyInvite')}
      </button>{inviteError ? <ErrorMark message={inviteError} /> : null}</div>}
    </div>
  );
}

/** The owner types the code the recipient sent. A match runs the approval; a
    mismatch says what it may mean. The expected code is never shown: typing
    it from the recipient's message is the check. */
export function SafetyCodeEntry({
  name,
  expected,
  label,
  busy,
  unavailable,
  error,
  onApprove,
}: {
  name: string;
  expected: string;
  label: string;
  busy: boolean;
  unavailable?: string | undefined;
  error?: string | undefined;
  onApprove: (typed: string) => void;
}) {
  useI18n();
  const id = useId();
  const [value, setValue] = useState('');
  const [problem, setProblem] = useState<'incomplete' | 'mismatch' | null>(null);
  // Not a <form>: the share window that shows these details is one already.
  function submit() {
    const typed = normalizeSafetyCode(value);
    if (typed.length !== 12) return setProblem('incomplete');
    if (typed !== normalizeSafetyCode(expected)) return setProblem('mismatch');
    setProblem(null);
    onApprove(value.trim());
  }
  return (
    <div className="signed-share-entry">
      <label htmlFor={id} className="signed-share-entry-label">{translate('sharing.codeInputLabel', { name })}</label>
      <div className="signed-share-actions">
        <input
          id={id}
          className="signed-share-input"
          value={value}
          onChange={(event) => { setValue(event.target.value); setProblem(null); }}
          onKeyDown={(event) => {
            if (event.key !== 'Enter' || event.nativeEvent.isComposing) return;
            event.preventDefault();
            if (!busy && !unavailable && value.trim()) submit();
          }}
          placeholder={translate('sharing.codePlaceholder')}
          autoComplete="off"
          autoCapitalize="characters"
          spellCheck={false}
          maxLength={24}
          aria-invalid={problem ? true : undefined}
          aria-describedby={problem ? `${id}-problem` : undefined}
          disabled={busy}
        />
        <button className="relay-button-primary min-h-10 px-3 py-2 text-xs" type="button" onClick={submit} disabled={busy || Boolean(unavailable) || !value.trim()}>{label}</button>
        {unavailable ? <ErrorMark kind="unavailable" message={unavailable} /> : error ? <ErrorMark message={error} /> : null}
      </div>
      {/* A check of what is being typed is one line under the field (docs/ui-principles.md §11). */}
      {problem ? (
        <p id={`${id}-problem`} role="alert" className="signed-share-problem" data-problem={problem}>
          {problem === 'mismatch' ? translate('sharing.codeMismatch', { name }) : translate('sharing.codeIncomplete')}
        </p>
      ) : null}
    </div>
  );
}

/** Device grants without a relay row: listed for revocation. Reloads only when
    my shares or their devices' presence change, not on every portal refresh. */
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
  // A refresh that fails keeps the last inventory; only one that never
  // loaded says why. A failed revoke is a mark after its button.
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string>();
  const [revokeError, setRevokeError] = useState<{ grantId: string; message: string }>();
  const [revision, setRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  const rows = kind === 'share' ? portal.sharedByMe : (portal.grantsByMe ?? []);
  const rowsKey = rows.map((r) => `${r.deviceId}:${r.id}`).sort().join('|');
  const presenceKey = portal.devices.map((d) => `${d.id}:${d.connected}`).sort().join('|');
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
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
          if (
            !rowsRef.current.some(
              (r) => r.deviceId === deviceId && r.id === grant.relayRef.id,
            )
          )
            found.push({ deviceId, grant });
        }
      }
      if (active) {
        setOrphans(found);
        setLoaded(true);
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
  }, [rowsKey, presenceKey, kind, revision]);
  return (
    <div className="space-y-3">
      {orphans.map(({ deviceId, grant }) => (
        <section key={grant.grantId} className="signed-share-state" data-tone="warn" aria-label={translate('sharing.stillActive')}>
          <p className="signed-share-title">
            <ShieldAlert size={14} aria-hidden="true" />
            <span>{translate('sharing.orphanGrant', { name: grant.grantee.label, device: portal.devices.find((d) => d.id === deviceId)?.name ?? deviceId.slice(0, 8) })}</span>
          </p>
          <div className="signed-share-actions">
            <button
              className={button}
              disabled={busy}
              onClick={() => {
                setBusy(true);
                setRevokeError(undefined);
                void deleteDeviceGrant(deviceId, grant.grantId)
                  .then(() => setRevision((r) => r + 1))
                  .catch((e: unknown) =>
                    setRevokeError({
                      grantId: grant.grantId,
                      message: e instanceof Error
                        ? e.message
                        : translate('sharing.deviceCertificateInvalid'),
                    }),
                  )
                  .finally(() => setBusy(false));
              }}
            >
              {translate('sharing.revoke')}
            </button>
            {revokeError?.grantId === grant.grantId && <ErrorMark message={revokeError.message} />}
          </div>
        </section>
      ))}
      {error && !loaded && (
        <p className="flex items-center gap-1.5 text-sm text-[var(--theme-fg-muted)]">
          {translate('devices.unavailable')}
          <ErrorMark message={error} onRetry={() => setRevision((r) => r + 1)} />
        </p>
      )}
    </div>
  );
}

/** This account's own safety code, the one people who share with it ask for. */
export function AccountSafetyCode({ revision }: { revision?: unknown }) {
  useI18n();
  const [code, setCode] = useState('');
  const [error, setError] = useState<string>();
  const [attempt, setAttempt] = useState(0);
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
  }, [revision, attempt]);
  return (
    <div className="space-y-2 text-sm">
      <h3 className="flex items-center gap-1.5 font-medium">
        {translate('sharing.pendingYourCode')}
        {error && <ErrorMark message={error} onRetry={() => setAttempt((value) => value + 1)} />}
      </h3>
      {code ? (
        <>
          <p className="flex items-center gap-2">
            <code className="signed-share-own-code">{code}</code>
            <CopyButton value={code} label={translate('sharing.copyCode')} />
          </p>
          <p className="text-xs text-[var(--theme-fg-muted)]">{translate('sharing.yourSafetyCodeHint')}</p>
        </>
      ) : (
        !error && <p>{translate('sharing.accountNotConfigured')}</p>
      )}
    </div>
  );
}
