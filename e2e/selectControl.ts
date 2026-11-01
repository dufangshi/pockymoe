import { type Locator, type Page } from '@playwright/test';

/** Choose through the UI for shared Select controls. Native selectors remain
 * supported for isolated server fixtures that deliberately render plain HTML. */
export async function selectControl(page: Page, control: Locator, choice: string | { label: string }) {
  if (await control.evaluate(element => element.tagName === 'SELECT')) { await control.selectOption(choice); return; }
  await control.click();
  if (await control.getAttribute('aria-haspopup') === 'menu' && typeof choice === 'string') {
    const index = ['thread', 'workspace', 'device'].indexOf(choice);
    if (index < 0) throw new Error('Unknown search scope.');
    await page.getByRole('menuitemradio').nth(index).click(); return;
  }
  const id = await control.getAttribute('aria-controls');
  const popup = id ? page.locator(`[id=${JSON.stringify(id)}]`) : page.getByRole('listbox');
  const option = typeof choice === 'string'
    ? popup.locator(`[role=option][data-value=${JSON.stringify(choice)}]`)
    : popup.getByRole('option', { name: choice.label, exact: true });
  await option.click();
}
