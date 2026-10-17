import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Plus, Pencil } from 'lucide-react';
import { FormDialog } from './FormDialog';
import { useEffect, useState } from 'react';
import {
  fetchModelPricing,
  updateModelPricing,
  type ModelPriceRates,
} from '../lib/modelPricingApi';

const pricingFields = () => [
  ['inputUsdPerMillion', translate("settings.in")],
  ['cachedInputUsdPerMillion', translate("settings.cached")],
  ['outputUsdPerMillion', translate("settings.out_220e06")],
  ['cacheWriteInputUsdPerMillion', translate("settings.cacheWrite")],
  ['cacheWriteOneHourInputUsdPerMillion', translate("settings.cacheWriteOneHour")],
] as const;
const inputClass =
  'min-w-0 w-full rounded-md border border-[var(--theme-border)] bg-[var(--theme-surface-strong)] px-2 py-1.5 text-sm text-[var(--theme-fg)]';

export function ModelPricingSettings() {
  useI18n();
  const [models, setModels] = useState<Record<string, ModelPriceRates>>({});
  const [query, setQuery] = useState('');
  const [draft, setDraft] = useState<{
    id: string;
    rates: ModelPriceRates;
    aliases: string;
    isNew: boolean;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  useEffect(() => {
    let active = true;
    fetchModelPricing()
      .then((data) => {
        if (active) setModels(data.models);
      })
      .catch((e) => {
        if (active) setError(e.message);
      });
    return () => {
      active = false;
    };
  }, []);
  async function save(reset = false) {
    if (!draft || busy) return;
    setBusy(true);
    setError('');
    setMessage('');
    try {
      const data = await updateModelPricing({
        id: draft.id.trim(),
        reset,
        rates: {
          ...draft.rates,
          aliases: draft.aliases
            .split(',')
            .map((s) => s.trim())
            .filter(Boolean),
        },
      });
      setModels(data.models);
      setDraft(null);
      window.dispatchEvent(new Event('model-pricing-updated'));
      setMessage(reset ? translate("settings.defaultRestored") : translate("settings.modelPricesSaved"));
    } catch (e) {
      setError(e instanceof Error ? e.message : translate("settings.unableToSavePrices"));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="py-5" aria-label={translate("settings.modelPricing")}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-sm font-semibold">{translate("settings.modelPricing")}</h3>
        <button
          aria-label={translate("settings.addModel")}
          title={translate("settings.addModel")}
          className="host-icon-button inline-flex h-9 w-9 items-center justify-center rounded-md"
          onClick={() => {
            setError('');
            setDraft({
              id: '',
              rates: {
                inputUsdPerMillion: 0,
                cachedInputUsdPerMillion: 0,
                outputUsdPerMillion: 0,
              },
              aliases: '',
              isNew: true,
            });
          }}
        >
          <Plus size={18} />
        </button>
      </div>
      <p className="mt-1 text-xs text-[var(--theme-fg-muted)]">
        {translate("settings.uSDPer1MTokensInExcludesCache")}</p>
      <input
        aria-label={translate("settings.searchModelPrices")}
        className={`${inputClass} mt-3`}
        placeholder={translate("settings.searchModelIDOrDisplayName")}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      <div className="mt-2 max-h-72 overflow-auto rounded-md border border-[var(--theme-border)]">
        <table className="w-full text-left text-xs">
          <thead className="sticky top-0 bg-[var(--theme-surface-strong)]">
            <tr>
              <th className="p-2">{translate("settings.model")}</th>
              {pricingFields().map(([key, label]) => (
                <th className="p-2 text-right" key={key}>
                  {label}
                </th>
              ))}
              <th>
                <span className="sr-only">{translate("settings.edit")}</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {Object.entries(models)
              .filter(([id, r]) =>
                [id, ...(r.aliases ?? [])].some((s) =>
                  s.toLowerCase().includes(query.toLowerCase()),
                ),
              )
              .map(([id, rates]) => (
                <tr className="border-t border-[var(--theme-border)]" key={id}>
                  <td className="p-2">
                    <span>{id}</span>
                    {rates.custom && (
                      <span className="ml-1 text-[var(--theme-fg-muted)]">
                        {translate("settings.custom")}</span>
                    )}
                  </td>
                  {pricingFields().map(([key]) => (
                    <td className="p-2 text-right tabular-nums" key={key}>
                      {typeof rates[key] === 'number' ? `$${rates[key]}` : '—'}
                    </td>
                  ))}
                  <td className="p-2">
                    <button
                      aria-label={translate("settings.edit_c10442", { value1: id })}
                      className="host-secondary-button rounded border px-2 py-1"
                      onClick={() => {
                        setError('');
                        setDraft({
                          id,
                          rates: { ...rates },
                          aliases: (rates.aliases ?? []).join(', '),
                          isNew: false,
                        });
                      }}
                    >
                      <Pencil size={14} />
                    </button>
                  </td>
                </tr>
              ))}
          </tbody>
        </table>
      </div>
      {draft && (
        <FormDialog
          title={draft.isNew ? translate("settings.addModel") : translate("settings.editModelPrices")}
          description={translate("settings.uSDPer1MTokens")}
          busy={busy}
          onClose={() => setDraft(null)}
        >
          <form
            className="space-y-3"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <label className="block text-xs">
              {translate("settings.modelID")}<input
                required
                aria-label={translate("settings.pricingModelID")}
                className={`${inputClass} mt-1`}
                disabled={!draft.isNew || busy}
                value={draft.id}
                onChange={(e) => setDraft({ ...draft, id: e.target.value })}
              />
            </label>
            <label className="mt-2 block text-xs">
              {translate("settings.displayNamesAliasesCommaSeparated")}<input
                aria-label={translate("settings.modelAliases")}
                className={`${inputClass} mt-1`}
                value={draft.aliases}
                disabled={busy}
                onChange={(e) =>
                  setDraft({ ...draft, aliases: e.target.value })
                }
              />
            </label>
            <div className="mt-2 grid grid-cols-2 gap-2 sm:grid-cols-4">
              {pricingFields().map(([key, label]) => (
                <label className="text-xs" key={key}>
                  {label} $/1M
                  <input
                    aria-label={translate("settings.pricePerMillion", { value1: label })}
                    className={`${inputClass} mt-1`}
                    type="number"
                    min="0"
                    max="1000000"
                    step="any"
                    required={!['cacheWriteInputUsdPerMillion','cacheWriteOneHourInputUsdPerMillion'].includes(key)}
                    disabled={busy}
                    value={draft.rates[key] ?? ''}
                    onChange={(e) =>
                      setDraft({
                        ...draft,
                        rates: {
                          ...draft.rates,
                          [key]:
                            e.target.value === ''
                              ? undefined
                              : Number(e.target.value),
                        },
                      })
                    }
                  />
                </label>
              ))}
            </div>
            {draft.rates.notes && (
              <p className="mt-2 text-xs text-[var(--theme-fg-muted)]">
                {draft.rates.notes}
              </p>
            )}
            {draft.rates.longContextThresholdTokens != null && (
              <p className="mt-2 text-xs text-[var(--theme-fg-muted)]">
                {translate("settings.longContextAbove")}{' '}
                {Number(
                  draft.rates.longContextThresholdTokens,
                ).toLocaleString(getLocale())}{' '}
                {translate("settings.inputTokInCache")}{draft.rates.longContextInputMultiplier} {translate("settings.out")}{draft.rates.longContextOutputMultiplier}
              </p>
            )}
            {draft.rates.sourceUrl && (
              <a
                className="mt-2 block text-xs underline"
                href={draft.rates.sourceUrl}
                target="_blank"
                rel="noreferrer"
              >
                {translate("settings.officialPricingChecked")} {draft.rates.verifiedAt}
              </a>
            )}
            <div className="mt-3 flex flex-wrap gap-2">
              <button
                disabled={busy}
                type="submit"
                className="relay-button-primary min-h-10 rounded-md px-4 text-xs font-semibold"
              >
                {busy ? translate("settings.saving_56a228") : translate("settings.savePrices")}
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => setDraft(null)}
                className="host-secondary-button min-h-10 rounded-md border px-3 text-xs"
              >
                {translate("settings.cancel")}</button>
              {!draft.isNew && draft.rates.custom && (
                <button
                  type="button"
                  disabled={busy}
                  className="host-secondary-button rounded border px-3 py-2 text-xs"
                  onClick={() => void save(true)}
                >
                  {translate("settings.resetRemoveCustom")}</button>
              )}
            </div>
            {error && (
              <p role="alert" className="host-error mt-2 text-xs">
                {error}
              </p>
            )}
          </form>
        </FormDialog>
      )}
      {error && !draft && (
        <p role="alert" className="host-error mt-2 rounded border p-2 text-xs">
          {error}
        </p>
      )}
      {message && (
        <p role="status" className="mt-2 text-xs">
          {message}
        </p>
      )}
    </section>
  );
}
