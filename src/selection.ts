// Sélection de messages en temps linéaire (audit P3) : avec « Tout
// sélectionner », 10 000 messages sélectionnés faisaient 100 millions de
// comparaisons à chaque rendu (`Array.includes` dans un `filter`).
import type { Flags, MessageSummary } from "./types";

/** Messages de `list` dont l'identifiant est dans `ids`, dans l'ordre de la liste. */
export function pickById<T extends { id: number }>(list: readonly T[], ids: ReadonlySet<number>): T[] {
  return list.filter((m) => ids.has(m.id));
}

/**
 * `flag` mis à `value` sur les messages `ids` ; les autres messages, et ceux
 * qui avaient déjà cette valeur, gardent le même objet (rien à redessiner).
 */
export function withFlag(
  list: readonly MessageSummary[],
  ids: Iterable<number>,
  flag: keyof Pick<Flags, "seen" | "flagged">,
  value: boolean,
): MessageSummary[] {
  const set = new Set(ids);
  return list.map((m) => (set.has(m.id) && m.flags[flag] !== value ? { ...m, flags: { ...m.flags, [flag]: value } } : m));
}

/**
 * Lance `call` et n'applique `apply` (changement de l'affichage) que si le
 * backend a accepté (audit B3) ; sinon l'erreur va à `fail`. `always` passe
 * dans les deux cas (compteur des actions en attente).
 */
export async function whenAccepted(
  call: Promise<unknown>,
  apply: () => void,
  fail: (e: unknown) => void,
  always?: () => void,
): Promise<void> {
  try {
    await call;
  } catch (e) {
    fail(e);
    return;
  } finally {
    always?.();
  }
  apply();
}
