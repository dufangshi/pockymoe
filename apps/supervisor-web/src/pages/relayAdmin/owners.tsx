// A device always goes with its owner in the console: names such as "mac" or
// "wsl" exist for many people. The device name opens the device's details,
// the owner's name opens the person's; the owner's email shows on hover and
// keyboard focus (title) and is in the person's details, where touch reaches
// it (docs/ui-principles.md §8).
import { MiddleName } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { ReactNode } from 'react';

export interface DeviceOwner {
  deviceId: string;
  name: string;
  ownerUserId?: string | null | undefined;
  ownerUsername?: string | null | undefined;
  ownerEmail?: string | null | undefined;
}

/** "mac · alice" for places that hold text only (phone rows, summaries). */
export function deviceOwnerText(device: Pick<DeviceOwner, 'name' | 'ownerUsername'>) {
  return device.ownerUsername ? `${device.name} · ${device.ownerUsername}` : device.name;
}

/** The owner as a link to the person, with the email as its title. */
export function OwnerLink({ userId, username, email, onOpenUser }: {
  userId?: string | null | undefined;
  username?: string | null | undefined;
  email?: string | null | undefined;
  onOpenUser?: ((id: string) => void) | undefined;
}) {
  useI18n();
  if (!username) return <span className="admin-muted">—</span>;
  const label = email ? `${username} (${email})` : username;
  if (!userId || !onOpenUser) return <span className="admin-owner" title={email ?? undefined}>{username}</span>;
  return (
    <button
      aria-label={translate('admin.openOwnerOf', { owner: label })}
      className="admin-text-link admin-owner"
      onClick={() => onOpenUser(userId)}
      title={email ?? undefined}
      type="button"
    >
      {username}
    </button>
  );
}

/**
 * A device cell: the device name (middle-truncated, opens the device) over
 * its owner (opens the person). `detail` adds text after the owner
 * ("alice · paper-figs").
 */
export function DeviceOwnerCell({ device, onOpenDevice, onOpenUser, detail }: {
  device: DeviceOwner;
  onOpenDevice?: ((id: string) => void) | undefined;
  onOpenUser?: ((id: string) => void) | undefined;
  detail?: ReactNode;
}) {
  useI18n();
  return (
    <span className="admin-two-line admin-device-owner">
      {onOpenDevice ? (
        <button
          aria-label={translate('admin.openDeviceOf', { name: device.name, owner: device.ownerUsername ?? '—' })}
          className="admin-row-name admin-row-opener admin-device-name"
          onClick={() => onOpenDevice(device.deviceId)}
          title={device.name}
          type="button"
        >
          <b><MiddleName name={device.name} /></b>
        </button>
      ) : (
        <b className="admin-device-name" title={device.name}><MiddleName name={device.name} /></b>
      )}
      <span className="admin-device-owner-line">
        <OwnerLink email={device.ownerEmail} onOpenUser={onOpenUser} userId={device.ownerUserId} username={device.ownerUsername} />
        {detail ? <span className="admin-muted"> · {detail}</span> : null}
      </span>
    </span>
  );
}
