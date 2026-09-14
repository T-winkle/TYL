import { tr } from "./i18n";

export const TRANSLATION_LANGUAGES = [
  "zh-CN",
  "zh-TW",
  "en",
  "ja",
  "ko",
  "fr",
  "de",
  "es",
  "ru",
  "pt",
] as const;

export type TranslationLanguage = (typeof TRANSLATION_LANGUAGES)[number];
export type SourceLanguage = TranslationLanguage | "auto";

export function languageLabel(language?: string) {
  return (
    {
      auto: tr("自动检测", "Auto detect"),
      "zh-CN": tr("简体中文", "Simplified Chinese"),
      "zh-TW": tr("繁體中文", "Traditional Chinese"),
      en: tr("英语", "English"),
      ja: tr("日语", "Japanese"),
      ko: tr("韩语", "Korean"),
      fr: tr("法语", "French"),
      de: tr("德语", "German"),
      es: tr("西班牙语", "Spanish"),
      ru: tr("俄语", "Russian"),
      pt: tr("葡萄牙语", "Portuguese"),
    }[language ?? "auto"] ?? language ?? tr("自动检测", "Auto detect")
  );
}

export function shortLanguageLabel(language?: string) {
  return (
    {
      auto: tr("自动", "AUTO"),
      "zh-CN": tr("简中", "ZH-CN"),
      "zh-TW": tr("繁中", "ZH-TW"),
      en: tr("英语", "EN"),
      ja: tr("日语", "JA"),
      ko: tr("韩语", "KO"),
      fr: tr("法语", "FR"),
      de: tr("德语", "DE"),
      es: tr("西语", "ES"),
      ru: tr("俄语", "RU"),
      pt: tr("葡语", "PT"),
    }[language ?? "auto"] ?? language ?? tr("自动", "AUTO")
  );
}
