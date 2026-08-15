/**
 * i18next initialization.
 *
 * Resources are bundled (no lazy loading) — the app ships with both
 * Chinese and English translations in the JS bundle.
 *
 * Language switching is driven by settingsStore.general.language,
 * which calls i18n.changeLanguage() directly.
 */

import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { zh } from "./zh";
import { en } from "./en";

void i18n.use(initReactI18next).init({
  resources: {
    zh: { translation: zh },
    en: { translation: en },
  },
  lng: "zh",
  fallbackLng: "zh",
  interpolation: {
    escapeValue: false,
  },
});

export default i18n;
