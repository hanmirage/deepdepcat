/**
 * Vision model presets — free multimodal endpoints (Zhipu GLM, see
 * https://docs.bigmodel.cn/cn/guide/models/free/). All share the same
 * OpenAI-compatible root URL; picking one auto-fills baseUrl + model so
 * the user only needs to paste their own (free) API key.
 */

export interface VisionPreset {
  id: string;
  model: string;
  baseUrl: string;
  /** i18n key for the option label (short, e.g. "GLM-4V-Flash"). */
  labelKey: string;
  /** i18n key for the option description (one-liner, feature hint). */
  descKey: string;
}

/** Sentinel id for the "manual" entry in the preset selector. */
export const CUSTOM_VISION_PRESET_ID = "__custom__";

export const VISION_PRESETS: VisionPreset[] = [
  {
    id: "glm-4v-flash",
    model: "glm-4v-flash",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    labelKey: "settings.modelProviders.visionPresetGlm4v",
    descKey: "settings.modelProviders.visionPresetGlm4vDesc",
  },
  {
    id: "glm-4.6v-flash",
    model: "glm-4.6v-flash",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    labelKey: "settings.modelProviders.visionPresetGlm46v",
    descKey: "settings.modelProviders.visionPresetGlm46vDesc",
  },
  {
    id: "glm-4.1v-thinking-flash",
    model: "glm-4.1v-thinking-flash",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    labelKey: "settings.modelProviders.visionPresetGlm41v",
    descKey: "settings.modelProviders.visionPresetGlm41vDesc",
  },
];

/** The preset whose baseUrl + model match the current values, or undefined. */
export function matchVisionPreset(
  baseUrl: string,
  model: string,
): VisionPreset | undefined {
  const url = baseUrl.trim().replace(/\/+$/, "");
  return VISION_PRESETS.find(
    (p) => p.baseUrl.trim().replace(/\/+$/, "") === url && p.model === model.trim(),
  );
}
