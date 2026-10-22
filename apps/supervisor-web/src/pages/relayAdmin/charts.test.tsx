import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { BarList, ChartCard, ColumnChart, HistoryNote, LineChart, niceTicks, ProportionBar, Sparkline } from './charts';

const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');

beforeAll(() => {
  setLocale('en', false);
  // jsdom lays nothing out; the charts measure their container.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 600 });
});
afterAll(() => {
  if (width) Object.defineProperty(HTMLElement.prototype, 'clientWidth', width);
});

describe('admin charts', () => {
  it('uses whole-number ticks for counts', () => {
    expect(niceTicks(2)).toEqual([0, 1, 2]);
    expect(niceTicks(87)).toEqual([0, 25, 50, 75, 100]);
    expect(niceTicks(0)).toEqual([0, 1]);
    expect(niceTicks(0.6, 4, false)).toEqual([0, 0.2, 0.4, 0.6]);
  });

  it('breaks a line at unknown points instead of drawing zeros', () => {
    const { container } = render(
      <LineChart
        formatAt={(at) => String(at)}
        label="Online devices"
        points={[
          { at: 0, value: 3 },
          { at: 1, value: 4 },
          { at: 2, value: null },
          { at: 3, value: 5 },
          { at: 4, value: 6 },
        ]}
      />,
    );
    const line = container.querySelector('path.admin-line');
    // Two runs: one "M" per run.
    expect(line?.getAttribute('d')?.match(/M/g)).toHaveLength(2);
    expect(screen.getByRole('img', { name: 'Online devices: 6 (4)' })).toBeInTheDocument();
  });

  it('shows the value under the pointer and keyboard focus', () => {
    const { container } = render(
      <ColumnChart
        data={[
          { key: 'a', label: 'Mon', values: { login: 3, challenge: 1 } },
          { key: 'b', label: 'Tue', values: { login: 5, challenge: 2 } },
        ]}
        label="Security events"
        series={[
          { key: 'login', label: 'Sign-in', slot: 1 },
          { key: 'challenge', label: 'Second-factor check', slot: 2 },
        ]}
      />,
    );
    expect(container.querySelectorAll('path.admin-bar')).toHaveLength(4);
    // The tallest column carries its total.
    expect(container.querySelector('.admin-value-label')?.textContent).toBe('7');
    const chart = container.querySelector('.admin-chart') as HTMLElement;
    act(() => chart.focus());
    fireEvent.keyDown(chart, { key: 'End' });
    const tooltip = container.querySelector('.admin-tooltip');
    expect(tooltip?.textContent).toContain('Tue');
    expect(tooltip?.textContent).toContain('5Sign-in');
    fireEvent.keyDown(chart, { key: 'Escape' });
    expect(container.querySelector('.admin-tooltip')).toBeNull();
  });

  it('switches every chart to a table view', () => {
    render(
      <ChartCard table={{ columns: ['Day', 'Count'], rows: [['Mon', 1200]] }} title="Sign-ups">
        <BarList items={[{ key: 'a', label: 'Within 1 hour', value: 18 }]} label="Activity" />
      </ChartCard>,
    );
    expect(screen.getByRole('list', { name: 'Activity' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Show as table' }));
    expect(screen.getByRole('table')).toHaveTextContent('1,200');
    expect(screen.getByRole('button', { name: 'Show as chart' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('names parts of a whole and tiny trends', () => {
    render(
      <>
        <ProportionBar items={[{ key: 'a', label: '0.13.38', value: 3, slot: 1 }, { key: 'b', label: '0.13.37', value: 1, slot: 2 }]} label="Versions" />
        <Sparkline label="Requests per day" values={[1, null, 4]} />
      </>,
    );
    expect(screen.getByRole('img', { name: 'Versions: 0.13.38 3, 0.13.37 1' })).toBeInTheDocument();
    expect(screen.getByRole('img', { name: 'Requests per day' })).toBeInTheDocument();
  });

  it('says where history starts instead of drawing zeros before it', () => {
    const { rerender, container } = render(<HistoryNote historyStart="2026-10-20T00:00:00Z" rangeStart={Date.parse('2026-10-15T00:00:00Z')} />);
    expect(container.textContent).toMatch(/^History starts .*unknown, not zero\.$/);
    rerender(<HistoryNote historyStart="2026-10-10T00:00:00Z" rangeStart={Date.parse('2026-10-15T00:00:00Z')} />);
    expect(container.textContent).toBe('');
  });
});
