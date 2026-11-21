// The interface's language: English unless the user chose Russian. Every app of Lumen takes it from here; the words
// live next to what shows them, as `words(en, ru)`: `ru` must have every key `en` has, or `tsc` fails.
export type Lang = "en" | "ru";

/** The languages in the globe's order (`LangToggle`), each by its own name. */
export const LANGS: readonly { id: Lang; name: string }[] = [
  { id: "en", name: "English" },
  { id: "ru", name: "Русский" },
];

export const isLang = (l: unknown): l is Lang => l === "en" || l === "ru";

let current: Lang = "en";

export const lang = (): Lang => current;

/** Anything but a known language changes nothing. */
export function setLang(l: unknown) {
  if (!isLang(l) || l === current) return;
  current = l;
  if (typeof document !== "undefined") document.documentElement.lang = l;
}

// The page's address may name it: the overlay window is opened so, and the stand-ins (`?lang=ru`).
if (typeof location !== "undefined") setLang(new URLSearchParams(location.search).get("lang"));

/** The words of the current language: call it while rendering, not once at load. */
export function words<T>(en: T, ru: NoInfer<T>): () => T {
  return () => (current === "ru" ? ru : en);
}
