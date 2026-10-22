// Lists of security events and admin actions, shared by the overview, the
// user details and the sessions and security page.
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import {
  Ban,
  Copy,
  Fingerprint,
  KeyRound,
  LogIn,
  LogOut,
  MonitorX,
  RotateCcw,
  Settings,
  ShieldAlert,
  ShieldCheck,
  Share2,
  Smartphone,
  UserCheck,
  UserPlus,
  UserX,
} from 'lucide-react';
import { formatDateTime, formatRelative } from './format';
import { actionLabel, eventLabel } from './labels';
import type { AdminAction, AdminSecurityEvent } from './types';

const icon = { 'aria-hidden': true, className: 'h-4 w-4' } as const;

export function EventIcon({ event, category }: { event: string; category: string }) {
  const glyph = (() => {
    if (event.startsWith('login.failed')) return <ShieldAlert {...icon} />;
    if (event.startsWith('login.')) return <LogIn {...icon} />;
    if (event === 'security.reauthenticated') return <ShieldCheck {...icon} />;
    if (event === 'session.revoked' || event === 'session.ended' || event === 'user.sessions_ended') return <LogOut {...icon} />;
    if (event === 'browser.revoked') return <MonitorX {...icon} />;
    if (event === 'device.setup_copied') return <Copy {...icon} />;
    if (event === 'device.token_rotated') return <RotateCcw {...icon} />;
    if (event.startsWith('device.')) return <Smartphone {...icon} />;
    if (event.startsWith('passkey.') || event.startsWith('authenticator.')) return <Fingerprint {...icon} />;
    if (event.startsWith('share.') || event.startsWith('grant.') || event === 'shared_access_left') return <Share2 {...icon} />;
    if (event === 'registration.approved' || event === 'registration.submitted') return <UserPlus {...icon} />;
    if (event === 'registration.rejected' || event === 'user.deleted') return <UserX {...icon} />;
    if (event === 'user.disabled') return <Ban {...icon} />;
    if (event === 'user.enabled') return <UserCheck {...icon} />;
    if (event === 'user.password_reset') return <KeyRound {...icon} />;
    if (event.startsWith('settings.')) return <Settings {...icon} />;
    return <ShieldCheck {...icon} />;
  })();
  return <span className={`admin-event-icon is-${category}`}>{glyph}</span>;
}

export function EventList({ events, compact = false }: { events: AdminSecurityEvent[]; compact?: boolean }) {
  useI18n();
  if (!events.length) return <p className="admin-empty">{translate('admin.noEvents')}</p>;
  return (
    <ul className={`admin-event-list${compact ? ' is-compact' : ''}`}>
      {events.map((event) => (
        <li key={event.id}>
          <EventIcon category={event.category} event={event.event} />
          <span className="admin-event-text">
            <b>{event.username ? `${event.username} · ${eventLabel(event.event)}` : eventLabel(event.event)}</b>
            {event.resourceLabel ? <span>{event.resourceLabel}</span> : null}
          </span>
          <time dateTime={event.at} title={formatDateTime(event.at)}>{formatRelative(event.at)}</time>
        </li>
      ))}
    </ul>
  );
}

/** Actions that take something away read in the failure tone, the rest neutral. */
export function actionCategory(action: string) {
  return /disabled|deleted|rejected|revoked|disconnected|ended/.test(action) ? 'failure' : 'activity';
}

export function ActionList({ actions }: { actions: AdminAction[] }) {
  useI18n();
  if (!actions.length) return <p className="admin-empty">{translate('admin.noAdminActions')}</p>;
  return (
    <ul className="admin-event-list">
      {actions.map((action) => (
        <li key={action.id}>
          <EventIcon category={actionCategory(action.action)} event={action.action} />
          <span className="admin-event-text">
            <b>{action.targetLabel ? `${actionLabel(action.action)} · ${action.targetLabel}` : actionLabel(action.action)}</b>
            <span>{action.adminUsername}</span>
          </span>
          <time dateTime={action.at} title={formatDateTime(action.at)}>{formatRelative(action.at)}</time>
        </li>
      ))}
    </ul>
  );
}
