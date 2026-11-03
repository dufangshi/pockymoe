import { request } from './api';
export interface ModelPriceRates {
  inputUsdPerMillion?: number;
  cachedInputUsdPerMillion?: number;
  outputUsdPerMillion?: number;
  cacheWriteInputUsdPerMillion?: number;
  cacheWriteOneHourInputUsdPerMillion?: number;
  aliases?: string[];
  sourceUrl?: string;
  verifiedAt?: string;
  notes?: string;
  custom?: boolean;
  unpriced?: boolean;
  longContextThresholdTokens?: number;
  longContextInputMultiplier?: number;
  longContextOutputMultiplier?: number;
  [key: string]: unknown;
}
export interface ModelPricingCatalog {
  models: Record<string, ModelPriceRates>;
  syncedAt?: string;
  localOverrides?: Record<string, ModelPriceRates>;
}
export type PricingScope = 'local' | 'account';
const pricingPath = (scope: PricingScope) => scope === 'account' ? 'relay/account/model-pricing' : 'api/config/model-pricing';
export const fetchModelPricing = (scope: PricingScope = 'local') =>
  request<ModelPricingCatalog>(
    pricingPath(scope),
    { cache: 'no-store' },
  );
export const updateModelPricing = (input: {
  id: string;
  reset?: boolean;
  rates?: ModelPriceRates;
}, scope: PricingScope = 'local') =>
  request<ModelPricingCatalog>(
    pricingPath(scope),
    { method: 'PATCH', body: JSON.stringify(input) },
  );

export const importModelPricing = (models: Record<string, ModelPriceRates>) =>
  request<ModelPricingCatalog>('relay/account/model-pricing', {
    method: 'PATCH', body: JSON.stringify({ import: models }),
  });
