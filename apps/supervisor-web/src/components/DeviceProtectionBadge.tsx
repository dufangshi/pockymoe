import { useEffect, useRef, useState } from 'react';
import { LoaderCircle, ShieldAlert, ShieldCheck, ShieldOff, ShieldQuestion } from 'lucide-react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { RelayDeviceDto } from '@pockymoe/shared';
import { currentAccount } from '../lib/accountVault';
import type { DeviceProtectionState } from '../lib/deviceEnrollment';
import { protectDevices, updateAndProtectDevice } from '../lib/remoteDeviceProtection';
import { appUrl } from '../lib/webBase';
import { AnchoredPopover } from './AnchoredPopover';
import { DeviceBindingPanel } from './DeviceBindingPanel';
import { protectionView, useRemoteProtection, type Tone } from './DeviceProtectionStatus';
import { FormDialog } from './FormDialog';
import { ProtectDevicesButton } from './RemoteProtectionControls';

const icons: Record<Tone, typeof ShieldCheck> = {
  ok: ShieldCheck, neutral: ShieldOff, info: ShieldQuestion, warn: ShieldAlert, error: ShieldAlert,
};

export function DeviceProtectionBadge({ device, protection }: {
  device: RelayDeviceDto;
  protection: DeviceProtectionState;
}) {
  useI18n();
  const { state, canSwitch } = useRemoteProtection(device);
  const [open, setOpen] = useState(false);
  const [binding, setBinding] = useState(false);
  const [busy, setBusy] = useState(false);
  const [actionFailed, setActionFailed] = useState(false);
  const [accountId, setAccountId] = useState<string>();
  const trigger = useRef<HTMLButtonElement>(null);
  const status = protectionView(state, protection);
  useEffect(() => {
    if (open && status?.key === 'otherAccount')
      void currentAccount().then((account) => setAccountId(account?.accountKeyId.slice(0, 12))).catch(() => undefined);
  }, [open, status?.key]);
  if (!status) return null;
  const label = translate(`devices.state.${status.key}` as const);
  const Icon = status.key === 'updating' || status.key === 'protecting' ? LoaderCircle : icons[status.tone];
  const remote = (work: () => Promise<unknown>) => {
    setBusy(true);
    setActionFailed(false);
    void work()
      .catch(() => setActionFailed(true))
      .finally(() => setBusy(false));
  };
  const openBinding = () => { setOpen(false); setBinding(true); };
  const actions = {
    notProtected: <button className="relay-button-primary px-3 py-2" onClick={openBinding} type="button">{translate('devices.protectDevice')}</button>,
    waitingBind: <button className="relay-button-secondary px-3 py-2" onClick={openBinding} type="button">{translate('devices.showLocalBindCommand')}</button>,
    needsRepair: <button className="relay-button-secondary px-3 py-2" onClick={openBinding} type="button">{translate('devices.repairDevice')}</button>,
    updateNeeded: <button className="relay-button-primary px-3 py-2" disabled={busy || !device.connected} onClick={() => remote(() => updateAndProtectDevice(device.id))} type="button">{translate(busy ? 'devices.starting' : 'devices.updateAndProtect')}</button>,
    retry: <button className="relay-button-primary px-3 py-2" disabled={busy || !device.connected} onClick={() => remote(() => protectDevices([device]))} type="button">{translate(busy ? 'devices.starting' : 'devices.retryProtection')}</button>,
  };
  const action = status.key === 'notProtected' ? actions.notProtected
    : status.key === 'waitingBind' ? actions.waitingBind
      : status.key === 'needsRepair' ? actions.needsRepair
        : status.key === 'updateNeeded' || status.key === 'updateFailed' ? actions.updateNeeded
          : status.key === 'otherAccount' || status.key === 'failed' ? actions.retry
            : null;
  const switchable = status.key === 'waitingBind' && state?.phase === 'local-command-pending' && device.connected && canSwitch;
  const fix = status.tone === 'error' || status.tone === 'warn';
  return (
    <>
      <button
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label={status.key === 'protected' ? label : undefined}
        className="device-status-chip"
        data-tone={status.tone}
        onClick={() => setOpen(!open)}
        ref={trigger}
        type="button"
      >
        <Icon aria-hidden="true" className={Icon === LoaderCircle ? 'animate-spin' : undefined} size={14} />
        {status.key === 'protected' ? null : <span>{label}</span>}
      </button>
      <AnchoredPopover anchor={trigger} label={label} onClose={() => setOpen(false)} open={open}>
        <p className="font-semibold text-[var(--theme-fg)]">{label}</p>
        <p className="text-[var(--theme-fg-muted)]">{translate(`devices.detail.${status.key}` as const)}</p>
        {status.key === 'otherAccount' && accountId ? (
          <p className="text-[var(--theme-fg-muted)]">{translate('devices.yourAccountId', { value1: accountId })}</p>
        ) : null}
        {actionFailed && <p role="alert">{translate('devices.remoteProtectionFailed')}</p>}
        {action || switchable ? (
          <div className="flex flex-wrap items-center gap-2">
            {action}
            {switchable && <ProtectDevicesButton devices={[device]} replaceLocal />}
          </div>
        ) : null}
        <a
          className="inline-flex text-sm font-medium text-[var(--theme-accent-strong)] hover:underline"
          href={appUrl(`help/devices#${status.anchor}`)}
          rel="noopener"
          target="_blank"
        >
          {translate(fix ? 'devices.howToFix' : 'devices.learnMore')} →
        </a>
      </AnchoredPopover>
      {binding && (
        <FormDialog
          onClose={() => setBinding(false)}
          title={translate(status.key === 'needsRepair' ? 'devices.repairDevice' : 'devices.protectDevice')}
        >
          <DeviceBindingPanel device={device} deviceId={device.id} repair={status.key === 'needsRepair'} />
        </FormDialog>
      )}
    </>
  );
}
