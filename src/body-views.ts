// Vues du corps calculées à la demande (audit P7) : `get_message` rend le
// corps d'origine seul. Le corps nettoyé (mode « HTML simplifié », citation
// d'une réponse ou d'un transfert) et le texte (mode « Texte ») sont demandés
// au backend la première fois qu'on en a besoin, une fois par message.
import { api } from "./api";
import type { BodyMode } from "./general";
import type { Message } from "./types";

export type Views = { simple: string; text: string };

/**
 * Vues des derniers messages, par identifiant et corps : changer un
 * indicateur (nouvel objet, même corps) ne les fait pas recalculer, afficher
 * les images (autre corps) si. Peu de messages gardés : un corps peut peser
 * plusieurs Mo.
 */
const done = new Map<number, { html: string; views: Promise<Views> }>();
const KEPT = 8;

/** Vrai si le message n'a pas encore ses vues (un corps vide n'en a pas besoin). */
export function lacksViews(m: Message): boolean {
  return m.bodyHtml !== "" && m.bodySimple === "" && m.bodyText === "";
}

/** Vrai si ce mode d'affichage a besoin des vues que le message n'a pas. */
export function needsViews(m: Message, mode: BodyMode): boolean {
  return mode !== "original" && lacksViews(m);
}

/**
 * Message avec son corps nettoyé et son texte. Une seule demande par corps,
 * même si l'affichage et une réponse les veulent en même temps ; un échec
 * n'est pas gardé, la demande suivante réessaie.
 */
export function withViews(m: Message, compute: (html: string) => Promise<Views> = api.bodyViews): Promise<Message> {
  if (!lacksViews(m)) return Promise.resolve(m);
  let entry = done.get(m.id);
  if (!entry || entry.html !== m.bodyHtml) {
    const views = compute(m.bodyHtml);
    const fresh = { html: m.bodyHtml, views };
    entry = fresh;
    done.delete(m.id);
    done.set(m.id, fresh);
    if (done.size > KEPT) done.delete(done.keys().next().value as number);
    views.catch(() => {
      if (done.get(m.id) === fresh) done.delete(m.id);
    });
  }
  return entry.views.then((v) => ({ ...m, bodySimple: v.simple, bodyText: v.text }));
}
