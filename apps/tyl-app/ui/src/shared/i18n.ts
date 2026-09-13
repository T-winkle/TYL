import { createSignal } from "solid-js";

export type LanguagePreference = "system" | "zh-CN" | "en-US";
export type Locale = "zh-CN" | "en-US";

function systemLocale(): Locale {
  const languages = navigator.languages?.length
    ? navigator.languages
    : [navigator.language];
  return languages[0]?.toLowerCase().startsWith("zh")
    ? "zh-CN"
    : "en-US";
}

const [locale, setLocale] = createSignal<Locale>(systemLocale());

export function applyLanguage(preference: LanguagePreference | string = "system") {
  const next: Locale =
    preference === "zh-CN"
      ? "zh-CN"
      : preference === "en-US"
        ? "en-US"
        : systemLocale();
  setLocale(next);
  document.documentElement.lang = next;
  return next;
}

/** Reactive two-language copy helper. */
export function tr(zhCN: string, enUS: string) {
  return locale() === "zh-CN" ? zhCN : enUS;
}

export function localizedError(value: unknown, englishFallback: string) {
  const message = String(value);
  return locale() === "en-US" && /[\u3400-\u9fff]/.test(message)
    ? englishFallback
    : message;
}

// Keep the document language correct even during the brief settings-load state.
applyLanguage("system");
