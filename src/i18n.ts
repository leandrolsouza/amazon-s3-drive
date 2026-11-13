import portuguese from "./locales/pt-BR.json";

const translations: Readonly<Record<string, string>> = portuguese;
export let locale: "en-US" | "pt-BR" = "en-US";

export function initializeLocale(preference: unknown) {
  locale = preference === "pt-BR" ? "pt-BR" : "en-US";
  document.documentElement.lang = locale;
}

export function t(message: string, ...values: (string | number)[]): string {
  const translated = locale === "pt-BR" && Object.prototype.hasOwnProperty.call(translations, message)
    ? translations[message]
    : message;
  return translated.replace(/\{(\d+)\}/g, (token, index) => String(values[index] ?? token));
}
