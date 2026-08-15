/**
 * Model pricing configuration.
 *
 * Models are loaded from settings store (user-configured providers).
 * Pricing data is injected here for cost estimation.
 */

import type { ModelInfo } from "@/types";

/** Pricing per 1M tokens for cost estimation. */
export interface ModelPricing {
  inputPricePer1M: number;
  outputPricePer1M: number;
}

/** Extended model info with pricing. */
export interface ModelWithPricing extends ModelInfo {
  pricing: ModelPricing;
  /**
   * Backend provider name (`provider.id`) — the session provider hint must
   * match the backend config exactly, not the display name. The display
   * `provider` field stays for the UI; this is the routing key.
   */
  providerId: string;
}

/** DeepSeek official pricing (¥ converted to USD at ~0.14 rate). */
export const DEEPSEEK_PRICING: Record<string, ModelPricing> = {
  "deepseek-v4-pro": { inputPricePer1M: 0.40, outputPricePer1M: 0.80 },
  "deepseek-v4-flash": { inputPricePer1M: 0.13, outputPricePer1M: 0.27 },
};

/** Resolve pricing for a model. Returns DeepSeek pricing if known, zero otherwise. */
export function resolvePricing(modelId: string): ModelPricing {
  return DEEPSEEK_PRICING[modelId] ?? { inputPricePer1M: 0, outputPricePer1M: 0 };
}

/**
 * Calculate the approximate cost of a session.
 * Formula: (promptTokens / 1M) * inputPrice + (completionTokens / 1M) * outputPrice
 */
export function calculateCost(
  promptTokens: number,
  completionTokens: number,
  pricing: ModelPricing,
): number {
  return (
    (promptTokens / 1_000_000) * pricing.inputPricePer1M +
    (completionTokens / 1_000_000) * pricing.outputPricePer1M
  );
}
