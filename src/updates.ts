// Nouvelle version publiée (lot K1) : GitHub est interrogé par le backend au
// plus une fois par jour, si le réglage « Signaler les nouvelles versions »
// est actif. Une version écartée (« Ne plus la signaler ») ne revient pas ;
// la suivante, si.
import { useEffect, useState } from "react";
import { api } from "./api";
import { useGeneral } from "./general";
import type { NewVersion } from "./types";

const LAST_CHECK = "vortex.update.lastCheck";
const SKIPPED = "vortex.update.skipped";
const DAY_MS = 24 * 3600_000;
/** Premier essai peu après le démarrage, sans retarder l'affichage. */
const FIRST_DELAY_MS = 60_000;
/** Vérifie toutes les heures si une journée s'est écoulée (application jamais fermée). */
const TICK_MS = 3600_000;

/** Faut-il interroger GitHub ? Au plus une fois par jour ; horloge reculée : oui. */
export function checkDue(now: number, last: number | null): boolean {
  return last === null || now - last >= DAY_MS || now < last;
}

/** Version à signaler : ni déjà écartée, ni vide. */
export function worthShowing(found: NewVersion | null, skipped: string | null): found is NewVersion {
  return found !== null && found.version !== "" && found.version !== skipped;
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // stockage indisponible : nouvel essai au prochain démarrage
  }
}

/**
 * Un passage : interroge GitHub (`check`) si une journée s'est écoulée depuis
 * la dernière réponse, et rend la version à signaler (null : rien). Une
 * erreur (hors ligne, GitHub indisponible) est transmise sans noter le
 * passage : nouvel essai à l'heure suivante.
 */
export async function checkOnce(now: number, check: () => Promise<NewVersion | null>): Promise<NewVersion | null> {
  const last = Number(read(LAST_CHECK));
  if (!checkDue(now, Number.isFinite(last) && last > 0 ? last : null)) return null;
  const result = await check();
  write(LAST_CHECK, String(now));
  return worthShowing(result, read(SKIPPED)) ? result : null;
}

/** « Ne plus la signaler » : cette version ne revient pas, la suivante si. */
export function skipVersion(version: string) {
  write(SKIPPED, version);
}

/**
 * Version plus récente à proposer (null : rien), `later` pour fermer, et
 * `skip` pour ne plus signaler celle-ci.
 */
export function useUpdateCheck(): { found: NewVersion | null; later: () => void; skip: () => void } {
  const { checkUpdates } = useGeneral();
  const [found, setFound] = useState<NewVersion | null>(null);
  useEffect(() => {
    if (!checkUpdates) {
      setFound(null);
      return;
    }
    let alive = true;
    const run = async () => {
      try {
        const result = await checkOnce(Date.now(), api.checkUpdate);
        if (alive && result) setFound(result);
      } catch {
        // hors ligne, ou GitHub indisponible : nouvel essai à l'heure suivante
      }
    };
    const first = window.setTimeout(run, FIRST_DELAY_MS);
    const timer = window.setInterval(run, TICK_MS);
    return () => {
      alive = false;
      window.clearTimeout(first);
      window.clearInterval(timer);
    };
  }, [checkUpdates]);
  return {
    found,
    later: () => setFound(null),
    skip: () => {
      if (found) skipVersion(found.version);
      setFound(null);
    },
  };
}
