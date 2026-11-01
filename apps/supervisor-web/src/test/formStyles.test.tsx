import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { Select } from '@pockymoe/thread-ui';
it('does not hide ordinary labels when shared form and navigation styles are loaded', () => {
  render(<><button><strong>Workspace</strong><small>/code/workspace</small></button><Select options={[{ value: 'a', label: 'Alpha' }]} /></>);
  expect(screen.getByText('/code/workspace')).toBeVisible();
  expect(screen.getByRole('button', { name: /Workspace.*code/ })).toBeVisible();
});
