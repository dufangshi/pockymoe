import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useLayoutEffect, useState, useRef } from 'react';
import { createPortal } from 'react-dom';
import {
  LockKeyhole,
  ShieldAlert,
  LockKeyholeOpen,
  Wifi,
  WifiOff,
  LoaderCircle,
} from 'lucide-react';
import {
  getTransportStatus,
  probeDeviceEncryption,
  trustDeviceIdentity,
} from '../lib/relayTransport';
import { useDeviceProtection, protectionLabel } from '../lib/useDeviceProtection';
import { type TransportStatus } from '../lib/relayTransportCrypto';
import { DeviceTrustPanel } from './DeviceTrustPanel';
import { FormDialog } from './FormDialog';
import { ConfirmDialog } from './ConfirmDialog';
import { popoverPosition } from '../lib/popoverPosition';
import { OWNER_UNLOCKED_EVENT, ownerUnlockedDevice } from '../lib/ownerUnlockRecovery';
export function DeviceEncryptionStatus({
  deviceId,
  online,
  connection,
  hideHealthy = false,
}: {
  deviceId?: string | undefined;
  online?: boolean;
  hideHealthy?: boolean;
  connection?: {
    loaded: boolean;
    busy: boolean;
    state: string;
    label: string;
    onConnect: () => void;
  };
}) {
  useI18n();
  const [status, setStatus] = useState<TransportStatus | undefined>(() =>
    deviceId ? getTransportStatus(deviceId) : undefined,
  );
  const protection = useDeviceProtection(deviceId, status);
  const [manage, setManage] = useState(false);
  const [open, setOpen] = useState(false),
    [confirm, setConfirm] = useState<TransportStatus | null>(null);
  const [trustError, setTrustError] = useState<string | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const popover = useRef<HTMLSpanElement>(null);
  const [position, setPosition] = useState({ top: 0, left: 12, maxWidth: 360, maxHeight: 600 });
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const rect = trigger.current?.getBoundingClientRect();
      const panel = popover.current?.getBoundingClientRect();
      const viewport = window.visualViewport;
      if (rect && panel) setPosition(popoverPosition(rect, {
        width: 360,
        height: Math.max(panel.height, (popover.current?.scrollHeight ?? 0) +
          (popover.current ? popover.current.offsetHeight - popover.current.clientHeight : 0)),
      }, {
        width: viewport?.width ?? window.innerWidth,
        height: viewport?.height ?? window.innerHeight,
        left: viewport?.offsetLeft ?? 0,
        top: viewport?.offsetTop ?? 0,
      }));
    };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') { setOpen(false); trigger.current?.focus(); } };
    place();
    window.addEventListener('resize', place);
    window.addEventListener('scroll', place, true);
    window.addEventListener('keydown', escape);
    window.visualViewport?.addEventListener('resize', place);
    window.visualViewport?.addEventListener('scroll', place);
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(place);
    if (popover.current) observer?.observe(popover.current);
    return () => {
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', place, true);
      window.removeEventListener('keydown', escape);
      window.visualViewport?.removeEventListener('resize', place);
      window.visualViewport?.removeEventListener('scroll', place);
      observer?.disconnect();
    };
  }, [open]);
  useEffect(() => {
    setStatus(deviceId ? getTransportStatus(deviceId) : undefined);
    const update = (event: Event) => {
      const next = (event as CustomEvent<TransportStatus>).detail;
      if (next.deviceId === deviceId) setStatus(next);
    };
    window.addEventListener('remote-codex-transport', update);
    return () => window.removeEventListener('remote-codex-transport', update);
  }, [deviceId]);
  useEffect(() => {
    if (deviceId && online) void probeDeviceEncryption(deviceId);
  }, [deviceId, online]);
  useEffect(() => {
    const unlocked = (event: Event) => {
      if (deviceId && ownerUnlockedDevice(event, deviceId) && connection && !connection.loaded && !connection.busy)
        connection.onConnect();
    };
    window.addEventListener(OWNER_UNLOCKED_EVENT, unlocked);
    return () => window.removeEventListener(OWNER_UNLOCKED_EVENT, unlocked);
  }, [connection, deviceId]);
  if (!deviceId && !connection) return null;
  const locked = status?.state === 'unlock-required';
  const encrypted = status?.state === 'encrypted',
    changed = status?.state === 'identity-changed';
  if (hideHealthy && protection !== 'certificate-missing' && protection !== 'pending' && status?.role !== 'grantee' && !changed && !locked && connection?.loaded && connection.state === 'connected') return null;
  const label = !deviceId ? translate("auth.localDeviceConnection") : locked ? translate("auth.ownerUnlock") : encrypted
    ? translate(status?.role === 'grantee' ? status.identityTrust === 'invite' ? 'sharing.granteeConnectionPinned' : 'sharing.granteeConnectionTofu' : 'auth.deviceConnectionEncrypted')
    : changed
      ? translate("auth.deviceIdentityChanged")
      : status?.state === 'legacy'
        ? translate("auth.connectionIsNotEndToEndEncrypted")
        : translate("auth.deviceEncryptionNotVerified");
  return (
    <span className="relative inline-flex">
      <button
        ref={trigger}
        aria-expanded={open}
        type="button"
        aria-label={
          connection
            ? !connection.loaded && !changed && !locked
              ? connection.busy
                ? translate("auth.connectingThread")
                : translate("auth.connectThread")
              : `${connection.label} · ${label}`
            : label
        }
        title={connection ? `${connection.label} · ${label}` : label}
        disabled={connection?.busy}
        data-connection={
          connection
            ? !connection.loaded
              ? 'detached'
              : connection.state
            : undefined
        }
        data-encryption={
          encrypted
            ? 'encrypted'
            : changed
              ? 'identity-changed'
              : (status?.state ?? 'unknown')
        }
        className={
          connection
            ? 'device-connection-button'
            : 'inline-flex h-8 w-8 items-center justify-center rounded-full text-[var(--theme-fg-muted)] hover:bg-[var(--theme-hover)]'
        }
        onClick={() =>
          connection && !connection.loaded && !changed && !locked
            ? connection.onConnect()
            : setOpen(!open)
        }
      >
        {connection ? (
          <>
            {connection.busy || connection.state === 'reconnecting' ? (
              <LoaderCircle size={19} className="animate-spin" />
            ) : connection.state === 'offline' ? (
              <WifiOff size={19} />
            ) : encrypted && connection.loaded && connection.state === 'connected' ? (
              <LockKeyhole size={19} />
            ) : (
              <Wifi size={19} />
            )}
            {deviceId && !(encrypted && connection.loaded && connection.state === 'connected') && (
              <span className="device-connection-security" aria-hidden="true">
                {encrypted ? (
                  <LockKeyhole size={10} />
                ) : changed ? (
                  <ShieldAlert size={11} />
                ) : (
                  <LockKeyholeOpen size={10} />
                )}
              </span>
            )}
          </>
        ) : encrypted ? (
          <LockKeyhole size={14} />
        ) : (
          <ShieldAlert size={16} />
        )}
      </button>
      {open && createPortal(
        <>
          <button
            aria-label={translate("auth.closeConnectionInformation")}
            className="fixed inset-0 z-[99] cursor-default"
            onClick={() => setOpen(false)}
          />
          <span
            ref={popover}
            role="status"
            className="device-connection-popover fixed z-[100] block space-y-3 rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-4 text-left text-sm shadow-xl"
            style={{ ...position, width: 'min(360px, calc(100vw - 24px))', overflowY: 'auto' }}
          >
            <strong className="block">{label}</strong>
            {connection && <span className="block device-connection-state">{connection.label}</span>}
            <span className="block text-[var(--theme-fg-muted)]">
              {encrypted
                ? translate("auth.privateContentIsEncryptedBetweenThisBrowser")
                : changed
                  ? translate("auth.thisBrowserStoppedTheConnectionRunRemote")
                  : !deviceId ? connection?.label
                    : status?.state === 'legacy' ? translate("auth.thisDeviceUsesLegacyRelayTransportUpdate")
                    : translate("auth.deviceEncryptionHasNotBeenVerifiedReconnect")}
            </span>
            {status?.fingerprint && (
              <code
                className="block break-all text-xs"
                aria-label={translate("auth.deviceFingerprint")}
              >
                SHA-256 {status.fingerprint}
              </code>
            )}
            {deviceId && <span className="block space-y-2">
              <span className="block" role={protection === 'certificate-missing' ? 'alert' : undefined}>{translate(protectionLabel(protection))}</span>
              {protection === 'certificate-missing' && <span className="block">{translate('devices.certificateMissingHelp')}</span>}
              {locked && <button className="relay-button-primary px-3 py-2" onClick={() => { setOpen(false); window.dispatchEvent(new CustomEvent('remote-codex-owner-unlock-request', { detail: status })); }}>{translate('auth.ownerUnlock')}</button>}
              {status?.role === 'grantee' && <span className="block">{translate(status.identityTrust === 'invite' ? 'sharing.invitePinned' : 'sharing.recipientTofu')}</span>}
              {status?.strict && <span className="block">{translate('auth.ownerStrict')}</span>}
              {protection === 'not-protected' && <span className="block">{translate('devices.protectDeviceHelp')}</span>}
              {status?.role !== 'grantee' && <button className="relay-button-secondary px-3 py-2" onClick={() => { setOpen(false); setManage(true); }}>{translate('auth.ownerManage')}</button>}
            </span>}
            {trustError && <span role="alert">{trustError}</span>}
            {changed && protection === 'not-protected' && status?.identityKey && (
              <button
                className="relay-button-secondary px-3 py-2"
                onClick={() => {
                  setOpen(false);
                  setTrustError(null);
                  setConfirm(status);
                }}
              >
                {translate("auth.trustReplacementIdentity")}</button>
            )}
          </span>
        </>, document.body
      )}
      {manage && deviceId && <FormDialog title={translate("auth.ownerManage")} onClose={() => setManage(false)}><DeviceTrustPanel deviceId={deviceId} /></FormDialog>}
      <ConfirmDialog
        open={Boolean(confirm)}
        title={translate("auth.trustTheReplacementDeviceIdentity")}
        description={translate("auth.compareSHA256WithPockymoeRelay", { value1: confirm?.fingerprint ?? "" })}
        confirmLabel={translate("auth.iVerifiedTheFingerprint")}
        onCancel={() => setConfirm(null)}
        onConfirm={async () => {
          try {
            if (!deviceId || !confirm?.identityKey || !confirm.fingerprint) return;
            await trustDeviceIdentity(deviceId, confirm.identityKey, confirm.fingerprint);
            location.reload();
          } catch (error) {
            setTrustError(error instanceof Error ? error.message : translate("auth.unableToSaveDeviceIdentity"));
            setConfirm(null);
            setOpen(true);
          }
        }}
      />
    </span>
  );
}
