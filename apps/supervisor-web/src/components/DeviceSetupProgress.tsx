import { useEffect, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { LoaderCircle, ShieldAlert, ShieldCheck } from 'lucide-react';
import { ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { currentAccount } from '../lib/accountVault';
import { request } from '../lib/api';
import { getTransportStatus } from '../lib/relayTransport';
import { ENROLLMENT_CHANGED_EVENT, listPendingEnrollments } from '../lib/pendingEnrollmentStore';
import { finishPendingProtection } from '../lib/finishPendingProtection';

const NONE: ReadonlySet<string> = new Set();

/** The device proved, on this page, that the account already certified it. */
function verified(deviceId: string, accountKeyId: string) {
  const status = getTransportStatus(deviceId);
  return status?.state === 'encrypted' && status.certificateVerified === true && status.role === 'owner' && status.accountKeyId === accountKeyId;
}
/** The device answered as bound to an account, without a certificate this
 *  account accepts: it refuses work until its protection is finished. */
function blocked(deviceId: string) {
  const status = getTransportStatus(deviceId);
  return status?.bound === true && status.certificateVerified !== true;
}

function usePending(listed: (deviceId: string, accountKeyId: string) => boolean): ReadonlySet<string> {
  const [ids, setIds] = useState(NONE);
  const filter = useRef(listed);
  filter.current = listed;
  useEffect(() => {
    let alive = true;
    const read = async () => {
      try {
        const account = await currentAccount();
        const pending = account ? await listPendingEnrollments(account) : [];
        // Remote protection reports through the device's own status chip.
        const next = new Set(pending.filter((enrollment) => !enrollment.remoteProtection && filter.current(enrollment.deviceId, account!.accountKeyId)).map((enrollment) => enrollment.deviceId));
        if (alive) setIds((current) => (current.size === next.size && [...next].every((id) => current.has(id)) ? current : next));
      } catch {
        // No account key in this browser: nothing is pending here.
      }
    };
    void read();
    const changed = () => { void read(); };
    const events = [ENROLLMENT_CHANGED_EVENT, 'remote-codex-transport'];
    for (const event of events) window.addEventListener(event, changed);
    return () => { alive = false; for (const event of events) window.removeEventListener(event, changed); };
  }, []);
  return ids;
}

/** Devices that are set up and connected but refuse work until the reader
 *  finishes their protection. A pending command that was not run yet (the
 *  device still trusts the relay), or one copied again for a device that
 *  already shows its certificate, is not listed: neither blocks anything. */
export function usePendingProtection(): ReadonlySet<string> {
  return usePending((deviceId, accountKeyId) => blocked(deviceId) && !verified(deviceId, accountKeyId));
}
/** Every device with a setup or bind command from this browser still open. */
function useOpenEnrollments(): ReadonlySet<string> {
  return usePending((deviceId, accountKeyId) => !verified(deviceId, accountKeyId));
}

/** Finishes a connected device's protection when the reader asks; this may ask
 *  for the password once. `compact` is the mark on a device card. */
export function FinishProtectionButton({ deviceId, compact = false, onFinished }: {
  deviceId: string;
  compact?: boolean;
  onFinished?: () => void;
}) {
  useI18n();
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const finish = () => {
    setBusy(true);
    setFailed(false);
    void finishPendingProtection(deviceId)
      .then((done) => { if (done) onFinished?.(); else setFailed(true); })
      .catch(() => setFailed(true))
      .finally(() => setBusy(false));
  };
  return (
    <>
      <button
        className={compact ? 'pd-protect-badge' : 'relay-button-primary inline-flex min-h-11 items-center gap-2 px-3 py-2'}
        disabled={busy}
        onClick={finish}
        type="button"
      >
        {busy ? <LoaderCircle aria-hidden className="animate-spin" size={compact ? 10 : 14} /> : <ShieldAlert aria-hidden size={compact ? 10 : 14} />}
        {translate('devices.finishProtection')}
      </button>
      {failed ? <ErrorMark message={translate('devices.finishProtectionFailed')} /> : null}
    </>
  );
}

function useConnected(deviceId: string, initial: boolean) {
  const [connected, setConnected] = useState(initial);
  useEffect(() => {
    let alive = true;
    const read = () => {
      if (document.visibilityState !== 'visible') return;
      void request<{ connected?: boolean }>(`relay/devices/${encodeURIComponent(deviceId)}/presence`, { cache: 'no-store' })
        .then((presence) => { if (alive) setConnected(Boolean(presence.connected)); })
        .catch(() => undefined);
    };
    read();
    const timer = window.setInterval(read, 3000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [deviceId]);
  return connected;
}

/** Under a new device's setup command: what happens after it runs, so the
 *  reader sees the device arrive and become usable without leaving the window. */
export function DeviceSetupProgress({ deviceId, onOpen }: { deviceId: string; onOpen?: () => void }) {
  useI18n();
  const connected = useConnected(deviceId, false);
  const pending = useOpenEnrollments().has(deviceId);
  // The quiet finisher needs a moment after the device connects; only then is
  // a still-pending protection the reader's to finish.
  const [waited, setWaited] = useState(false);
  useEffect(() => {
    setWaited(false);
    if (!connected || !pending) return;
    const timer = window.setTimeout(() => setWaited(true), 12_000);
    return () => window.clearTimeout(timer);
  }, [connected, pending]);
  const row = 'mt-3 flex min-h-11 flex-wrap items-center gap-2 text-sm';
  if (!connected) {
    return <p className={`${row} text-[var(--theme-fg-muted)]`} role="status"><LoaderCircle aria-hidden className="animate-spin" size={14} />{translate('devices.setupWaiting')}</p>;
  }
  if (pending && !waited) {
    return <p className={`${row} text-[var(--theme-fg-muted)]`} role="status"><LoaderCircle aria-hidden className="animate-spin" size={14} />{translate('devices.setupFinishing')}</p>;
  }
  if (pending) {
    return <div className={`${row} text-[var(--theme-fg)]`} role="status"><span>{translate('devices.setupNeedsFinish')}</span><FinishProtectionButton deviceId={deviceId} /></div>;
  }
  return (
    <div className={`${row} text-[var(--theme-fg)]`} role="status">
      <ShieldCheck aria-hidden className="text-[var(--status-success-fg)]" size={16} />
      <span>{translate('devices.setupReady')}</span>
      <Link className="relay-button-primary inline-flex min-h-11 items-center px-3 py-2" onClick={onOpen} to={`/devices/${encodeURIComponent(deviceId)}/workspaces`}>{translate('devices.setupOpen')}</Link>
    </div>
  );
}

/** On a device page that cannot load because its protection is unfinished:
 *  the step that unblocks it, instead of an error with no way forward. */
export function DeviceProtectionPrompt({ deviceId, missingCertificate, onFinished }: {
  deviceId: string;
  /** The device is bound to the account but showed no certificate. */
  missingCertificate: boolean;
  onFinished: () => void;
}) {
  useI18n();
  const pending = usePendingProtection().has(deviceId);
  // Finished elsewhere (the quiet finisher, or another control): load again.
  const was = useRef(pending);
  useEffect(() => {
    if (was.current && !pending) onFinished();
    was.current = pending;
  }, [pending, onFinished]);
  if (pending) {
    return (
      <div className="flex min-h-11 flex-wrap items-center gap-3 text-sm" role="status">
        <span className="text-[var(--theme-fg)]">{translate('devices.finishProtectionHint')}</span>
        <FinishProtectionButton deviceId={deviceId} onFinished={onFinished} />
      </div>
    );
  }
  if (!missingCertificate) return null;
  return (
    <p className="flex min-h-11 flex-wrap items-center gap-3 text-sm text-[var(--theme-fg)]" role="status">
      <span>{translate('devices.protectionRepairHint')}</span>
      <Link className="relay-button-secondary inline-flex min-h-11 items-center px-3 py-2" to={`/desktop/devices/${encodeURIComponent(deviceId)}`}>{translate('desktopDevices.deviceDetails')}</Link>
    </p>
  );
}
