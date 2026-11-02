import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { ExcludedDevices } from './Charts';
afterEach(() => setLocale('en', false));
it('uses English singular/plural and names omitted devices in an Info bubble', () => {
  setLocale('en', false);
  const view = render(<ExcludedDevices names={['Legacy laptop']} />);
  expect(screen.getByText('1 device is running an older version and is not included')).toBeVisible();
  fireEvent.click(screen.getByRole('button'));
  expect(screen.getByText('Legacy laptop', { selector: '.error-mark-bubble *' })).toBeVisible();
  view.rerender(<ExcludedDevices names={['Legacy laptop', 'Old server']} />);
  expect(screen.getByText('2 devices are running older versions and are not included')).toBeVisible();
  view.rerender(<ExcludedDevices names={[]} />);
  expect(document.querySelector('.pd-chart-excluded')).toBeNull();
});
