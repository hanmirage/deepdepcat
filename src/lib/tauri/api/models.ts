/**
 * Model discovery API — provider `/models` lists are fetched from the Rust
 * side (native HTTP) because provider APIs don't send CORS headers to the
 * webview. The command returns the raw JSON body; the settings store keeps
 * response parsing in one place.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../core";

export const modelsApi = {
  fetchProviderModels: (
    baseUrl: string,
    apiKey: string,
    apiFormat: string,
  ): Promise<unknown> => {
    if (!isTauri) return Promise.reject(new Error("NOT_TAURI"));
    return invoke<unknown>("fetch_provider_models", { baseUrl, apiKey, apiFormat });
  },
};
