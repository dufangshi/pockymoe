// The page's time range as a chart request: a preset, or a custom window
// from the first day's local midnight to the end of the last day.
import type { WindowParams } from './adminApi';
import { formatDay } from './format';
import { rangeLabel } from './labels';
import type { AdminRange } from './types';

const DAY = 86_400_000;

export function localMidnight(day: string) {
  const [year, month, date] = day.split('-').map(Number);
  return new Date(year!, (month ?? 1) - 1, date ?? 1).getTime();
}

export function pageWindow(props: { range: AdminRange; customRange: { from: string; to: string } | null }): WindowParams {
  return props.customRange
    ? { from: localMidnight(props.customRange.from), to: localMidnight(props.customRange.to) + DAY }
    : { range: props.range };
}

/** "7 days", or "10/1 – 10/3" for a custom window. */
export function windowLabel(data: { range: string; window?: { start: string; end: string } | undefined }) {
  if (data.range !== 'custom' || !data.window) return rangeLabel(data.range as AdminRange);
  return `${formatDay(data.window.start)} – ${formatDay(Date.parse(data.window.end) - 1)}`;
}
