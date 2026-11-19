// Règles et signatures dans le cache chiffré (audit S8, lot J12b) : elles
// nomment des correspondants, et le stockage de la fenêtre est en clair sur
// le disque. Même interface que `localStorage` (lecture synchrone) : une copie
// en mémoire, chargée au démarrage de la fenêtre principale, et chaque
// écriture envoyée au backend dans l'ordre.
import { api, runningInTauri } from "./api";

/** Clés gardées dans le cache chiffré (même liste côté backend : `SECURE_PREFS`). */
export const SECURE_KEYS = ["vortex.mail", "vortex.rules", "vortex.rules.moved", "vortex.rules.done.v3"] as const;

const secure = new Set<string>(SECURE_KEYS);
const memory = new Map<string, string>();
/** Écritures envoyées une à une, dans l'ordre (la dernière l'emporte). */
let writing: Promise<void> = Promise.resolve();

export function isSecureKey(key: string): boolean {
  return secure.has(key);
}

function persist(key: string, value: string | null) {
  writing = writing
    .then(() => api.prefsSet(key, value))
    .catch(() => {
      // cache indisponible : la valeur reste en mémoire jusqu'à la fermeture
    });
}

/**
 * Comme `localStorage`, pour les clés de `SECURE_KEYS` dans Tauri ; hors de
 * Tauri (aperçu dans le navigateur, tests), simplement `localStorage`.
 */
export const secureStore = {
  getItem(key: string): string | null {
    if (!runningInTauri || !secure.has(key)) return localStorage.getItem(key);
    return memory.get(key) ?? null;
  },
  setItem(key: string, value: string) {
    if (!runningInTauri || !secure.has(key)) return localStorage.setItem(key, value);
    memory.set(key, value);
    persist(key, value);
  },
  removeItem(key: string) {
    if (!runningInTauri || !secure.has(key)) return localStorage.removeItem(key);
    memory.delete(key);
    persist(key, null);
  },
  /** Fenêtre secondaire : valeur reçue de la fenêtre principale, en mémoire seulement. */
  prime(key: string, value: string) {
    if (!runningInTauri || !secure.has(key)) return localStorage.setItem(key, value);
    memory.set(key, value);
  },
  /** Clés présentes (pour la sauvegarde). */
  keys(): string[] {
    return runningInTauri ? [...memory.keys()] : SECURE_KEYS.filter((k) => localStorage.getItem(k) !== null);
  },
};

/**
 * Fenêtre principale, avant le premier affichage : charge les valeurs du
 * cache chiffré, et y déplace celles d'une version précédente, restées en
 * clair dans le stockage de la fenêtre (qui sont ensuite effacées).
 */
export async function loadSecureStore(): Promise<void> {
  if (!runningInTauri) return;
  const stored = new Map(await api.prefsGet());
  for (const key of SECURE_KEYS) {
    let legacy: string | null = null;
    try {
      legacy = localStorage.getItem(key);
    } catch {
      // stockage indisponible : rien à reprendre
    }
    const value = stored.get(key);
    if (value !== undefined) {
      memory.set(key, value);
    } else if (legacy !== null) {
      memory.set(key, legacy);
      // Effacé en clair seulement une fois écrit dans le cache chiffré.
      await api.prefsSet(key, legacy);
    }
    if (legacy !== null) {
      try {
        localStorage.removeItem(key);
      } catch {
        // stockage indisponible
      }
    }
  }
}
