// Copie de secours des brouillons sur cet ordinateur, tant que le serveur ne
// les a pas (hors connexion, serveur injoignable). Retrouvée au démarrage.
// Dans l'application, toutes passent par le backend, qui les garde dans le
// cache chiffré (lot G4) ; le stockage de la fenêtre ne sert qu'à la démo.
import { api, runningInTauri } from "./api";
import type { Draft } from "./types";

const KEY = "vortex.drafts.local";

/**
 * Fenêtre de rédaction : son stockage est séparé de celui de la fenêtre
 * principale (lot 9c), la copie de secours passe donc par le backend, une
 * par fenêtre.
 */
const secondary = runningInTauri && new URLSearchParams(window.location.search).has("view");

export interface DraftBackup {
  accountId: string;
  draft: Draft;
  draftId: number | null;
  /** Horodatage (ms) de la copie. */
  at?: number;
}

function load(): Record<string, DraftBackup> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, DraftBackup>;
  } catch {
    return {};
  }
}

function store(all: Record<string, DraftBackup>) {
  try {
    if (Object.keys(all).length === 0) localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, JSON.stringify(all));
  } catch {
    // stockage plein ou indisponible : la copie de secours est perdue, pas le brouillon en cours
  }
}

export function writeBackup(key: number | string, backup: DraftBackup) {
  if (secondary) {
    api.saveDraftBackup(JSON.stringify({ ...backup, at: Date.now() })).catch(() => {});
    return;
  }
  if (runningInTauri) {
    api.saveDraftBackup(JSON.stringify({ ...backup, at: Date.now() }), String(key)).catch(() => {});
    return;
  }
  const all = load();
  all[String(key)] = { ...backup, at: Date.now() };
  store(all);
}

export function clearBackup(key: number | string) {
  if (secondary) {
    api.saveDraftBackup(null).catch(() => {});
    return;
  }
  if (String(key).startsWith(WINDOW_PREFIX)) {
    api.clearDraftBackup(String(key).slice(WINDOW_PREFIX.length)).catch(() => {});
    return;
  }
  if (runningInTauri) {
    api.saveDraftBackup(null, String(key)).catch(() => {});
    return;
  }
  const all = load();
  if (String(key) in all) {
    delete all[String(key)];
    store(all);
  }
}

/** Préfixe des copies gardées par le backend (fenêtres de rédaction, et fenêtre principale depuis le lot G4). */
const WINDOW_PREFIX = "fenetre:";

/**
 * Copies de secours restantes (brouillons jamais arrivés sur le serveur) :
 * celles de cette fenêtre et celles des fenêtres de rédaction.
 */
export async function backups(): Promise<[string, DraftBackup][]> {
  if (runningInTauri) {
    // Copies des versions d'avant le lot G4, restées en clair dans le stockage
    // de la fenêtre : confiées au backend (cache chiffré), puis retirées.
    for (const [key, b] of Object.entries(load())) {
      try {
        await api.saveDraftBackup(JSON.stringify(b), key);
        const left = load();
        delete left[key];
        store(left);
      } catch {
        // gardée ici : reproposée au prochain démarrage
      }
    }
  }
  const all: [string, DraftBackup][] = Object.entries(load());
  for (const [key, text] of await api.draftBackups().catch(() => [] as [string, string][])) {
    try {
      all.push([`${WINDOW_PREFIX}${key}`, JSON.parse(text) as DraftBackup]);
    } catch {
      api.clearDraftBackup(key).catch(() => {});
    }
  }
  return all.sort((a, b) => (a[1].at ?? 0) - (b[1].at ?? 0));
}
