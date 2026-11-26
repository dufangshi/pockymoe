import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import './device-state.css';

export type DeviceState = 'online' | 'offline' | 'unstable';

/**
 * Device connection state, recognizable by shape alone (docs/ui-principles.md
 * "Device connection state"): solid dot online, hollow circle offline, diamond
 * reconnecting often. Always with its words or an accessible name. The only
 * online mark of a device: cards, rows, the device switcher and the settings window.
 */
export function DeviceStateMark({ state, label, showLabel = true }: { state: DeviceState; label?: string | undefined; showLabel?: boolean }) {
  useI18n();
  const text = label ?? deviceStateLabel(state);
  return (
    <span className={`admin-device-state is-${state}`}>
      <span aria-hidden className="admin-device-state-mark" />
      {showLabel ? <span>{text}</span> : <span className="sr-only">{text}</span>}
    </span>
  );
}

export function deviceStateLabel(state: DeviceState) {
  return state === 'online' ? translate('admin.online') : state === 'unstable' ? translate('admin.unstable') : translate('admin.offline');
}
