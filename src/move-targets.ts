// Dossiers proposés par « Déplacer » (clic droit sur un message ou une
// sélection, lot Q) : sans recherche, les derniers dossiers où l'on a déplacé
// des messages ; avec une recherche, les dossiers de tous les comptes dont le
// nom la contient, sans tenir compte de la casse ni des accents (« fact »
// trouve « Factures », « reçus » trouve « Recus »).
import type { Account, Folder } from "./types";

export interface MoveTarget {
  folder: Folder;
  /** Dossiers parents, pour situer un résultat de recherche : « Clients › 2026 » ; vide sans recherche. */
  parents: string;
}

export interface MoveGroup {
  account: Account;
  targets: MoveTarget[];
}

/** Texte comparable : minuscules, sans accents. */
export function folded(s: string): string {
  return s.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
}

/**
 * Dossiers où déplacer, regroupés par compte dans l'ordre des comptes et de
 * l'arborescence. `exclude` : dossiers à ne pas proposer (celui d'où viennent
 * les messages).
 */
export function moveTargets(accounts: Account[], folders: Folder[], query: string, exclude: ReadonlySet<string> = new Set()): MoveGroup[] {
  const wanted = folded(query.trim());
  const byId = new Map(folders.map((f) => [f.id, f]));
  const parentsOf = (f: Folder) => {
    const names: string[] = [];
    let parent = f.parentId ? byId.get(f.parentId) : undefined;
    for (let i = 0; parent && i < 64; i++) {
      names.unshift(parent.name);
      parent = parent.parentId ? byId.get(parent.parentId) : undefined;
    }
    return names.join(" › ");
  };
  return accounts
    .map((account) => ({
      account,
      targets: folders
        .filter((f) => f.accountId === account.id && !exclude.has(f.id) && (wanted === "" || folded(f.name).includes(wanted)))
        .map((folder) => ({ folder, parents: wanted === "" ? "" : parentsOf(folder) })),
    }))
    .filter((g) => g.targets.length > 0);
}

/** Derniers dossiers où l'on a déplacé des messages, du plus récent au plus ancien. */
const RECENT_KEY = "vortex.move.recent";
const RECENT_MAX = 5;

/** Identifiants des dossiers récents (ceux de `localStorage`). */
export function recentMoveIds(): string[] {
  try {
    const list: unknown = JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
    return Array.isArray(list) ? list.filter((x): x is string => typeof x === "string").slice(0, RECENT_MAX) : [];
  } catch {
    return [];
  }
}

/** Note un dossier où des messages viennent d'être déplacés. */
export function rememberMoveTarget(folderId: string) {
  const list = [folderId, ...recentMoveIds().filter((x) => x !== folderId)].slice(0, RECENT_MAX);
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(list));
  } catch {
    // stockage indisponible : pas de dossiers récents
  }
}

/**
 * Dossiers récents encore présents, sauf ceux de `exclude`, chacun avec son
 * compte et ses parents pour le situer (« Clients › 2026 · OVH pro »).
 */
export function recentTargets(
  accounts: Account[],
  folders: Folder[],
  ids: string[],
  exclude: ReadonlySet<string> = new Set(),
): (MoveTarget & { account: Account })[] {
  const byId = new Map(folders.map((f) => [f.id, f]));
  const accountById = new Map(accounts.map((a) => [a.id, a]));
  const out: (MoveTarget & { account: Account })[] = [];
  for (const id of ids) {
    const folder = byId.get(id);
    const account = folder && accountById.get(folder.accountId);
    if (!folder || !account || exclude.has(id)) continue;
    const names: string[] = [];
    let parent = folder.parentId ? byId.get(folder.parentId) : undefined;
    for (let i = 0; parent && i < 64; i++) {
      names.unshift(parent.name);
      parent = parent.parentId ? byId.get(parent.parentId) : undefined;
    }
    out.push({ folder, account, parents: names.join(" › ") });
  }
  return out;
}
