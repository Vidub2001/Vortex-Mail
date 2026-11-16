// Langue de l'interface : français (langue source), anglais, espagnol.
// Les textes sont écrits en français dans le code et passent par `t()` ; les
// traductions sont indexées par le texte français (locales/en.ts, locales/es.ts).
// Un texte sans traduction reste en français.
// Changer de langue recharge les fenêtres : tout est relu dans la nouvelle langue.
import en from "./locales/en";
import es from "./locales/es";

export type Lang = "fr" | "en" | "es";

export const LANGUAGES: { value: Lang; label: string }[] = [
  { value: "fr", label: "Français" },
  { value: "en", label: "English" },
  { value: "es", label: "Español" },
];

const DICTS: Record<Exclude<Lang, "fr">, Record<string, string>> = { en, es };
const LOCALES: Record<Lang, string> = { fr: "fr-FR", en: "en-US", es: "es-ES" };

/** Langue du système, si elle est proposée ; le français sinon. */
function systemLang(): Lang {
  const l = (navigator.language || "fr").slice(0, 2).toLowerCase();
  return l === "en" || l === "es" ? l : "fr";
}

let current: Lang | null = null;

/** Langue choisie dans Paramètres › Général (lue une fois par fenêtre). */
export function lang(): Lang {
  if (current) return current;
  try {
    const saved = (JSON.parse(localStorage.getItem("vortex.general") ?? "{}") as { language?: string }).language;
    current = saved === "fr" || saved === "en" || saved === "es" ? saved : systemLang();
  } catch {
    current = systemLang();
  }
  return current;
}

/** Code régional pour Intl (dates, nombres, tri). */
export function locale(): string {
  return LOCALES[lang()];
}

/**
 * Texte traduit. `vars` remplace les `{nom}` du texte :
 * t("{n} messages", { n: 3 }).
 */
export function t(fr: string, vars?: Record<string, string | number>): string {
  const l = lang();
  const text = l === "fr" ? fr : (DICTS[l][fr] ?? fr);
  return vars ? text.replace(/\{(\w+)\}/g, (all, k: string) => (k in vars ? String(vars[k]) : all)) : text;
}
