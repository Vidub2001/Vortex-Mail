import type { Folder } from "./types";

/**
 * Tables de l'arborescence, construites une fois par liste de dossiers
 * (audit P7) : sans elles, chaque dossier parcourait tous les autres à chaque
 * rendu, sensible au-delà de quelques centaines de dossiers.
 */
export function folderTree(folders: readonly Folder[]) {
  const byId = new Map(folders.map((f) => [f.id, f]));
  const children = new Map<string, Folder[]>();
  /** Dossiers qui ont un descendant abonné. */
  const holdsSubscribed = new Set<string>();
  for (const f of folders) {
    if (f.parentId) {
      const list = children.get(f.parentId);
      if (list) list.push(f);
      else children.set(f.parentId, [f]);
    }
    if (f.subscribed) {
      // Un ancêtre déjà noté a déjà ses propres ancêtres notés.
      let parent = f.parentId;
      while (parent && !holdsSubscribed.has(parent)) {
        holdsSubscribed.add(parent);
        parent = byId.get(parent)?.parentId ?? null;
      }
    }
  }
  return { byId, children, holdsSubscribed };
}
