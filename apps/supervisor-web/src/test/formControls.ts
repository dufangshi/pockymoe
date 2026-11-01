import { fireEvent, within } from '@testing-library/react';

export function openSelect(control: HTMLElement): HTMLElement {
  if (control.getAttribute('role') !== 'combobox') control = within(control).getByRole('combobox');
  if (control.getAttribute('aria-expanded') !== 'true') fireEvent.click(control);
  const popup = document.getElementById(control.getAttribute('aria-controls') ?? '');
  if (!popup) throw new Error('The select popup did not open.');
  return popup;
}
export function selectOptions(control: HTMLElement): HTMLButtonElement[] {
  const options = within(openSelect(control)).getAllByRole('option') as HTMLButtonElement[];
  fireEvent.keyDown(options[0]!, { key: 'Escape' });
  return options;
}
export function selectOption(control: HTMLElement, query: { name?: string | RegExp }): HTMLButtonElement {
  const option = within(openSelect(control)).getByRole('option', query) as HTMLButtonElement;
  fireEvent.keyDown(option, { key: 'Escape' }); return option;
}
/** Exercise the popup for selects; text/number inputs keep their normal events. */
export function changeControl(control: HTMLElement, event: { target: { value: string | number } }) {
  if (control.getAttribute('role') !== 'combobox' || control.tagName !== 'BUTTON') { fireEvent.change(control, event); return; }
  const option = within(openSelect(control)).getAllByRole('option').find(option => option.getAttribute('data-value') === String(event.target.value));
  if (!option) throw new Error('The requested select value was not available.');
  fireEvent.click(option);
}
