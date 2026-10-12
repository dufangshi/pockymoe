import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useId, useState } from 'react';
import {
  ArrowUpRight,
  MonitorSmartphone,
  History,
  SlidersHorizontal,
  UserRound,
  UserRoundMinus,
} from 'lucide-react';
import { FormDialog } from './FormDialog';

type AccessEvent = {
  id: string;
  username: string;
  kind: string;
  accessedAt: string;
};
export function SharedAccessCard({
  title,
  subtitle,
  deviceName,
  deviceConnected,
  username,
  mode,
  permissions,
  events,
  lastAccessedAt,
  expanded,
  busy,
  onOpen,
  onEdit,
  onRevoke,
  onToggleAccess,
  details,
}: {
  details?: React.ReactNode;
  title: string;
  subtitle: string;
  deviceName: string;
  deviceConnected?: boolean | undefined;
  username: string;
  mode: 'incoming' | 'outgoing';
  permissions: string[];
  events: AccessEvent[];
  lastAccessedAt: string | null;
  expanded: boolean;
  busy: boolean;
  onOpen?: (() => void) | undefined;
  onEdit?: (() => void) | undefined;
  onRevoke?: (() => void) | undefined;
  onToggleAccess?: (() => void) | undefined;
}) {
  useI18n();
  const [profileOpen, setProfileOpen] = useState(false);
  const historyId = useId();
  const date = (value: string) =>
    new Date(value).toLocaleString(getLocale(), {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  return (
    <article className="shared-access-card">
      <div className="shared-access-card-heading">
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-sm font-semibold" title={title}>
            {title}
          </h3>
          <p
            className="mt-1 truncate text-xs text-[var(--theme-fg-muted)]"
            title={subtitle}
          >
            {subtitle}
          </p>
        </div>
        <button type="button" className="shared-access-open" onClick={onOpen} disabled={deviceConnected !== true || !onOpen} title={deviceConnected === true ? translate("sharing.open") : deviceConnected === false ? translate("sharing.supervisorOfflineReconnectTheDeviceToOpen") : translate("sharing.supervisorStatusUnavailable")}>
          {translate("sharing.open")} <ArrowUpRight size={15} aria-hidden="true" />
        </button>
      </div>
      <div className="shared-access-device">
        <span className="shared-access-device-name" title={deviceName}><MonitorSmartphone size={14} aria-hidden="true" /><span>{deviceName}</span></span>
        <span className="shared-supervisor-presence" data-online={deviceConnected === true} role="status">
          <span aria-hidden="true" />Supervisor {deviceConnected === true ? translate("sharing.online") : deviceConnected === false ? translate("sharing.offline") : translate("sharing.unknown")}
        </span>
      </div>
      <div className="shared-access-card-person">
        <button
          type="button"
          className="shared-profile-button"
          aria-label={translate("sharing.viewSProfile", { value1: username })}
          onClick={() => setProfileOpen(true)}
        >
          <span className="shared-profile-avatar" aria-hidden="true">
            {Array.from(username).slice(0, 2).join('').toUpperCase()}
          </span>
          <span className="min-w-0 text-left">
            <span className="block text-[10px] text-[var(--theme-fg-muted)]">
              {mode === 'incoming' ? translate("sharing.sharedBy") : translate("sharing.sharedWith")}
            </span>
            <span className="block truncate text-xs font-medium">
              {username}
            </span>
          </span>
        </button>
        {mode === 'outgoing' && (
          <div className="shared-access-actions">
            <button
              type="button"
              className="product-icon-button"
              aria-label={translate("sharing.permissions")}
              title={translate("sharing.editPermissions")}
              disabled={busy}
              onClick={onEdit}
            >
              <SlidersHorizontal size={16} />
            </button>
            <button
              type="button"
              className="product-icon-button"
              aria-label={translate("sharing.accessHistory")}
              title={translate("sharing.recentAccess")}
              aria-expanded={expanded}
              aria-controls={historyId}
              onClick={onToggleAccess}
            >
              <History size={16} />
            </button>
            <button
              type="button"
              className="product-icon-button shared-access-revoke"
              aria-label={translate("sharing.revoke")}
              title={translate("sharing.revokeAccess")}
              disabled={busy}
              onClick={onRevoke}
            >
              <UserRoundMinus size={16} />
            </button>
          </div>
        )}
      </div>
      <div className="shared-access-card-footer">
        <div className="flex min-w-0 flex-wrap gap-1.5">
          {permissions.map((permission) => (
            <span className="shared-access-permission" key={permission}>
              {permission}
            </span>
          ))}
        </div>
        {mode === 'outgoing' && (
          <span className="shared-access-date">
            {lastAccessedAt
              ? translate("sharing.visited", { value1: date(lastAccessedAt) })
              : translate("sharing.noVisitsYet")}
          </span>
        )}
      </div>
      {mode === 'outgoing' && expanded && (
        <section
          id={historyId}
          className="shared-access-history"
          aria-label={translate("sharing.recentAccess")}
        >
          <h4 className="mb-2 text-xs font-medium">{translate("sharing.recentAccess")}</h4>
          {events.length ? (
            <ol>
              {events.map((event) => (
                <li
                  key={event.id}
                  className="flex items-center justify-between gap-3 py-2 text-xs"
                >
                  <span className="min-w-0">
                    <span className="block truncate">{event.username}</span>
                    <span className="text-[var(--theme-fg-muted)]">
                      {event.kind.replaceAll('_', ' ')}
                    </span>
                  </span>
                  <time
                    className="shrink-0 text-[var(--theme-fg-muted)]"
                    dateTime={event.accessedAt}
                  >
                    {date(event.accessedAt)}
                  </time>
                </li>
              ))}
            </ol>
          ) : (
            <p className="text-xs text-[var(--theme-fg-muted)]">
              {translate("sharing.noVisitsRecordedYet")}</p>
          )}
        </section>
      )}
      {details}
      {profileOpen && (
        <FormDialog
          title={username}
          description={
            mode === 'incoming'
              ? translate("sharing.thisPersonSharedAccessWithYou")
              : translate("sharing.youSharedAccessWithThisPerson")
          }
          onClose={() => setProfileOpen(false)}
        >
          <div className="flex items-center gap-3 py-3">
            <span className="shared-profile-avatar !h-14 !w-14">
              <UserRound size={24} />
            </span>
            <div className="min-w-0">
              <p className="truncate font-semibold">{username}</p>
              <p className="text-xs text-[var(--theme-fg-muted)]">
                {translate("sharing.pockymoeAccount")}</p>
            </div>
          </div>
          <div className="rounded-xl border border-[var(--theme-border)] p-4">
            <p className="break-words text-sm font-medium">{title}</p>
            <p className="mt-1 break-words text-xs text-[var(--theme-fg-muted)]">
              {subtitle}
            </p>
            <p className="mt-3 text-xs">{permissions.join(' · ')}</p>
          </div>
          {mode === 'outgoing' && (
            <button
              type="button"
              className="relay-button-secondary mt-4"
              onClick={() => {
                setProfileOpen(false);
                onEdit?.();
              }}
            >
              {translate("sharing.editPermissions")}</button>
          )}
        </FormDialog>
      )}
    </article>
  );
}
