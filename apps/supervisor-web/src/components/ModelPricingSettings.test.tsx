import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { request } from '../lib/api';
import { ModelPricingSettings } from './ModelPricingSettings';
import type { ModelPriceRates } from '../lib/modelPricingApi';
vi.mock('../lib/api', () => ({ request: vi.fn(), readSelectedRelayDeviceId: () => 'device-a' }));
const reference = { inputUsdPerMillion: 2, cachedInputUsdPerMillion: 0.2, outputUsdPerMillion: 8 };
let models: Record<string, ModelPriceRates>;
beforeEach(() => {
  setLocale('en', false);
  models = { 'future-model': { unpriced: true } as unknown as ModelPriceRates, 'official-model': { ...reference } };
  vi.mocked(request).mockReset().mockImplementation(async (path, init) => {
    if (path === 'api/config/model-pricing') return { models: {}, localOverrides: { 'old-device-model': reference } };
    if (path !== 'relay/account/model-pricing') throw new Error(`Unexpected ${path}`);
    if (init?.method === 'PATCH') {
      const input = JSON.parse(String(init.body));
      if (input.import) models['old-device-model'] = { ...reference, custom: true };
      else if (input.reset) models[input.id] = { ...reference };
      else models[input.id] = { ...input.rates, custom: true };
    }
    return { models: structuredClone(models), syncedAt: '2026-11-03T12:00:00Z' };
  });
});
it('edits account prices for an unknown model, restores defaults and imports local prices', async () => {
  const refreshed = vi.fn(); window.addEventListener('model-pricing-updated', refreshed);
  render(<ModelPricingSettings scope="account" />);
  const pending = (await screen.findByText('future-model')).closest('tr')!;
  expect(within(pending).getByText('Awaiting price')).toBeVisible();
  expect(within(pending).getAllByText('—')).toHaveLength(5);
  fireEvent.click(within(pending).getByRole('button', { name: /Edit/ }));
  const dialog=screen.getByRole('dialog');
  const prices=within(dialog).getAllByRole('spinbutton');
  for (const [i,value] of ['2','0.2','8'].entries()) fireEvent.change(prices[i]!,{ target:{ value } });
  fireEvent.click(within(dialog).getByRole('button',{name:'Save prices'}));
  await waitFor(()=>expect(screen.queryByRole('dialog')).toBeNull());
  expect(models['future-model']!.inputUsdPerMillion).toBe(2);
  expect(refreshed).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole('button',{name:'Edit future-model'}));
  fireEvent.click(screen.getByRole('button',{name:'Reset / remove custom'}));
  await waitFor(()=>expect(models['future-model']!.custom).toBeUndefined());
  fireEvent.click(screen.getByRole('button',{name:/Import this device/}));
  await screen.findByText('old-device-model');
  expect(screen.queryByRole('button',{name:/Import this device/})).toBeNull();
  expect(vi.mocked(request).mock.calls.filter(([,init])=>init?.method==='PATCH').every(([path])=>path==='relay/account/model-pricing')).toBe(true);
  window.removeEventListener('model-pricing-updated', refreshed);
});

it('retains a changed price draft until the user confirms discarding it', async () => {
  render(<ModelPricingSettings scope="account" />);
  fireEvent.click(await screen.findByRole('button',{name:'Edit official-model'}));
  const edit=screen.getByRole('dialog');
  fireEvent.change(within(edit).getAllByRole('spinbutton')[0]!,{target:{value:'7'}});
  fireEvent.click(within(edit).getByRole('button',{name:'Cancel'}));
  const confirmation=screen.getByRole('dialog',{name:'Discard unsaved changes?'});
  fireEvent.click(within(confirmation).getByRole('button',{name:'Cancel'}));
  expect(within(edit).getAllByRole('spinbutton')[0]).toHaveValue(7);
  fireEvent.click(within(edit).getByRole('button',{name:'Cancel'}));
  fireEvent.click(screen.getByRole('button',{name:'Discard changes'}));
  await waitFor(()=>expect(screen.queryByRole('dialog')).toBeNull());
  expect(models['official-model']!.inputUsdPerMillion).toBe(2);
});
