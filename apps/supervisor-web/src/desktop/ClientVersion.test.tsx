import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { ClientVersion } from './ClientVersion';
import type { PockymoeHost } from './host';
afterEach(() => { delete window.pockymoeHost; });
it.each(['browser', 'mobile', 'desktop'])('only desktop shows its host version: %s', async surface => {
  setLocale('zh-CN', false);
  const install = vi.fn();
  if (surface !== 'browser') window.pockymoeHost = {
    capabilities: async () => ({ surface, shellVersion: '0.2.9', features: { updates: true } }),
    updates: { state: async () => ({ status: 'ready', version: '0.3.0' }), onChange: () => () => {}, install },
  } as unknown as PockymoeHost;
  const view = render(<ClientVersion />);
  await act(async () => {});
  if (surface !== 'desktop') { expect(view.container).toBeEmptyDOMElement(); return; }
  expect(screen.getByText(/Pockymoe 0.2.9/)).toBeVisible();
  fireEvent.click(screen.getByRole('menuitem', { name: '可更新到 0.3.0' }));
  expect(install).toHaveBeenCalledExactlyOnceWith();
});
