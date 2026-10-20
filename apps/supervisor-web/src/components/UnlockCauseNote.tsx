import { CopyButton } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState } from 'react';
import { fetchRelayPortal } from '../lib/api';
import type { UnlockCause } from '../lib/unlockCause';

const KIND = {
  'missing-browser-key': 'auth.unlockCause.missingBrowserKey',
  'device-refused': 'auth.unlockCause.deviceRefused',
  'browser-removed': 'auth.unlockCause.browserRemoved',
  'account-missing': 'auth.unlockCause.accountMissing',
  action: 'auth.unlockCause.action',
} as const;
const ACTION = {
  connect: 'auth.unlockAction.connect',
  'bind-device': 'auth.unlockAction.bindDevice',
  'certify-device': 'auth.unlockAction.certifyDevice',
  revoke: 'auth.unlockAction.revoke',
  'change-password': 'auth.unlockAction.changePassword',
  'recovery-code': 'auth.unlockAction.recoveryCode',
} as const;

/** Device names the account can see: its own and those shared with it. */
async function deviceNames() {
  const portal = await fetchRelayPortal();
  const names = new Map<string, string>(portal.devices.map(d => [d.id, d.name]));
  for (const entry of [...portal.sharedWithMe, ...(portal.sharedDevicesWithMe ?? []), ...(portal.sharedThreadsWithMe ?? [])])
    if (!names.has(entry.deviceId)) names.set(entry.deviceId, entry.deviceName);
  return names;
}

/** Why the unlock prompt opened: one plain line, the exact request below it, and a copy for support. */
export function UnlockCauseNote({ cause }: { cause: UnlockCause }) {
  useI18n();
  const [name, setName] = useState<string | null>(null);
  useEffect(() => {
    setName(null);
    if (!cause.deviceId) return;
    let active = true;
    void deviceNames().then(names => { if (active) setName(names.get(cause.deviceId!) ?? null); }).catch(() => {});
    return () => { active = false; };
  }, [cause.deviceId]);
  const device = cause.deviceId ? `${name ?? cause.deviceId.slice(0, 8)}` : '';
  const sentence = translate(KIND[cause.kind], {
    device,
    action: translate(ACTION[cause.action ?? 'connect']),
  });
  const time = new Date(cause.at).toLocaleTimeString();
  const details = [
    cause.deviceId ? `${device} (${cause.deviceId.slice(0, 8)})` : null,
    cause.method && cause.path ? `${cause.method} ${cause.path}` : null,
    cause.code ?? null,
    cause.source ?? null,
    time,
  ].filter(Boolean).join(' · ');
  const report = () => JSON.stringify({ ...cause, deviceName: name, page: window.location.pathname, userAgent: navigator.userAgent, at: new Date(cause.at).toISOString() }, null, 2);
  return (
    <div className="unlock-cause space-y-1 rounded-lg border border-[var(--theme-border)] px-3 py-2 text-xs leading-5" data-kind={cause.kind}>
      <p className="text-[var(--theme-fg)]">{sentence}</p>
      <div className="flex items-start gap-2">
        <p className="min-w-0 flex-1 break-all font-mono text-[11px] text-[var(--theme-fg-muted)]">{details}</p>
        <CopyButton value={report} label={translate('auth.unlockCause.copy')} />
      </div>
    </div>
  );
}
