import { request } from './api';
export interface ModelPriceRates {
  inputUsdPerMillion: number;
  cachedInputUsdPerMillion: number;
  outputUsdPerMillion: number;
  cacheWriteInputUsdPerMillion?: number;
  cacheWriteOneHourInputUsdPerMillion?: number;
  aliases?: string[];
  sourceUrl?: string;
  verifiedAt?: string;
  notes?: string;
  custom?: boolean;
  longContextThresholdTokens?: number;
  longContextInputMultiplier?: number;
  longContextOutputMultiplier?: number;
  [key: string]: unknown;
}
export const fetchModelPricing = () =>
  request<{ models: Record<string, ModelPriceRates> }>(
    'api/config/model-pricing',
    { cache: 'no-store' },
  );
export const updateModelPricing = (input: {
  id: string;
  reset?: boolean;
  rates?: ModelPriceRates;
}) =>
  request<{ models: Record<string, ModelPriceRates> }>(
    'api/config/model-pricing',
    { method: 'PATCH', body: JSON.stringify(input) },
  );
