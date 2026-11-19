// Listes des dossiers déjà reçues, avec leur version (audit P1) : la
// synchronisation n'envoie plus une liste qui n'a pas changé.
import type { MessageSummary } from "./types";

/** Liste d'un dossier telle que reçue, avec sa version dans le cache. */
export interface KnownList {
  revision: number;
  messages: MessageSummary[];
}

/** Résultat de `sync_messages_many` pour un dossier. */
export interface ListSync {
  folderId: string;
  /** Absente : la liste n'a pas changé depuis la version connue. */
  messages: MessageSummary[] | null;
  revision: number;
  /** `UIDVALIDITY` du dossier : les UID d'avant un changement ne valent plus rien. */
  uidValidity?: number | null;
  error: string | null;
}

/** Versions à envoyer avec la demande de synchronisation, dans l'ordre de `folderIds`. */
export function knownRevisions(known: ReadonlyMap<string, KnownList>, folderIds: readonly string[]): (number | null)[] {
  return folderIds.map((id) => known.get(id)?.revision ?? null);
}

/**
 * Garde les listes reçues et rend celles de chaque dossier, dans l'ordre des
 * résultats ; `changed` est faux si aucune n'a changé : rien à redessiner.
 */
export function applySync(known: Map<string, KnownList>, results: readonly ListSync[]): { changed: boolean; lists: MessageSummary[][] } {
  let changed = false;
  const lists = results.map((r) => {
    if (r.messages) {
      known.set(r.folderId, { revision: r.revision, messages: r.messages });
      changed = true;
      return r.messages;
    }
    return known.get(r.folderId)?.messages ?? [];
  });
  return { changed, lists };
}
