// Display formatting for the admin console, in the selected locale.
import { getLocale, translate } from '@pockymoe/thread-ui/i18n';

export function formatCount(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value)) return '—';
  return new Intl.NumberFormat(getLocale()).format(value);
}

/** 1,284 / 12.9K / 4.2M for figures that need to stay short. */
export function formatCompact(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value)) return '—';
  return new Intl.NumberFormat(getLocale(), { notation: value >= 10_000 ? 'compact' : 'standard', maximumFractionDigits: 1 }).format(value);
}

export function formatPercent(part: number, whole: number) {
  if (!whole) return '—';
  return new Intl.NumberFormat(getLocale(), { style: 'percent', maximumFractionDigits: 0 }).format(part / whole);
}

export function formatRatio(value: number | null | undefined, digits = 2) {
  if (value === null || value === undefined || !Number.isFinite(value)) return '—';
  return new Intl.NumberFormat(getLocale(), { style: 'percent', maximumFractionDigits: digits }).format(value);
}

export function formatBytes(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value)) return '—';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${new Intl.NumberFormat(getLocale(), { maximumFractionDigits: size >= 10 || unit === 0 ? 0 : 1 }).format(size)} ${units[unit]}`;
}

export function parseTime(value: string | number | null | undefined) {
  if (value === null || value === undefined || value === '') return null;
  const time = typeof value === 'number' ? value : Date.parse(value);
  return Number.isFinite(time) ? time : null;
}

/** "5 minutes ago", "in 6 days"; a missing time reads "never". */
export function formatRelative(value: string | number | null | undefined, now = Date.now()) {
  const time = parseTime(value);
  if (time === null) return translate('admin.never');
  const seconds = Math.round((time - now) / 1000);
  const abs = Math.abs(seconds);
  const format = new Intl.RelativeTimeFormat(getLocale(), { numeric: 'auto' });
  if (abs < 45) return translate('admin.justNow');
  if (abs < 3600) return format.format(Math.round(seconds / 60), 'minute');
  if (abs < 86_400) return format.format(Math.round(seconds / 3600), 'hour');
  if (abs < 86_400 * 30) return format.format(Math.round(seconds / 86_400), 'day');
  return formatDate(time);
}

export function formatDate(value: string | number | null | undefined) {
  const time = parseTime(value);
  if (time === null) return '—';
  return new Intl.DateTimeFormat(getLocale(), { dateStyle: 'medium' }).format(time);
}

export function formatDateTime(value: string | number | null | undefined) {
  const time = parseTime(value);
  if (time === null) return '—';
  return new Intl.DateTimeFormat(getLocale(), { dateStyle: 'medium', timeStyle: 'short' }).format(time);
}

export function formatTime(value: string | number | null | undefined) {
  const time = parseTime(value);
  if (time === null) return '—';
  return new Intl.DateTimeFormat(getLocale(), { hour: '2-digit', minute: '2-digit' }).format(time);
}

/** A short axis label for a day (`10/21` style in the locale). */
export function formatDay(value: string | number) {
  const time = typeof value === 'number' ? value : Date.parse(value.length === 10 ? `${value}T00:00:00Z` : value);
  return new Intl.DateTimeFormat(getLocale(), { month: 'numeric', day: 'numeric', timeZone: typeof value === 'string' && value.length === 10 ? 'UTC' : undefined }).format(time);
}

/** "3 d 4 h", "6 h", "4 min": the largest two units. */
export function formatDuration(seconds: number | null | undefined) {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 0) return '—';
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days) return hours ? translate('admin.durationDaysHours', { days, hours }) : translate('admin.durationDays', { days });
  if (hours) return minutes ? translate('admin.durationHoursMinutes', { hours, minutes }) : translate('admin.durationHours', { hours });
  if (minutes) return translate('admin.durationMinutes', { minutes });
  return translate('admin.durationSeconds', { seconds: Math.max(0, Math.round(seconds)) });
}

export function since(value: string | null | undefined, now = Date.now()) {
  const time = parseTime(value);
  return time === null ? null : Math.max(0, (now - time) / 1000);
}

/** Two letters for an avatar. */
export function initials(name: string | null | undefined) {
  const trimmed = (name ?? '').trim();
  if (!trimmed) return '??';
  return Array.from(trimmed.replace(/[^\p{L}\p{N}]/gu, '') || trimmed).slice(0, 2).join('').toUpperCase();
}
