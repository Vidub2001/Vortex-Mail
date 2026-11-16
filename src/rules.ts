// Règles de classement : appliquées automatiquement aux nouveaux messages des
// boîtes de réception, ou à la demande sur un dossier. Le déplacement peut viser
// un dossier d'un autre compte. Règles gardées dans le stockage local (donc
// comprises dans la sauvegarde de la configuration).
import { useEffect, useState } from "react";
import { api } from "./api";
import { t } from "./i18n";
import type { Folder, Message, MessageSummary, MoveReport } from "./types";

export type RuleField = "from" | "recipients" | "subject" | "body" | "attachment";
export type RuleOp = "contains" | "notContains" | "is" | "startsWith" | "endsWith" | "yes" | "no";

export interface RuleCondition {
  field: RuleField;
  op: RuleOp;
  value: string;
}

export type RuleActionType = "move" | "markRead" | "flag" | "delete";

export interface RuleAction {
  type: RuleActionType;
  /** Dossier de destination (« move »), éventuellement d'un autre compte. */
  folderId?: string;
}

export interface Rule {
  id: string;
  name: string;
  enabled: boolean;
  /** Compte dont les messages sont examinés ; « all » : tous. */
  account: string;
  /** Toutes les conditions, ou au moins une. */
  match: "all" | "any";
  conditions: RuleCondition[];
  actions: RuleAction[];
  /** Ne pas appliquer les règles suivantes au message. */
  stop: boolean;
}

export const FIELDS: { value: RuleField; label: string }[] = [
  { value: "from", label: t("Expéditeur") },
  { value: "recipients", label: t("Destinataires (À, Cc)") },
  { value: "subject", label: t("Objet") },
  { value: "body", label: t("Début du message") },
  { value: "attachment", label: t("Pièce jointe") },
];

export const TEXT_OPS: { value: RuleOp; label: string }[] = [
  { value: "contains", label: t("contient") },
  { value: "notContains", label: t("ne contient pas") },
  { value: "is", label: t("est") },
  { value: "startsWith", label: t("commence par") },
  { value: "endsWith", label: t("se termine par") },
];

export const ATTACHMENT_OPS: { value: RuleOp; label: string }[] = [
  { value: "yes", label: t("présente") },
  { value: "no", label: t("absente") },
];

export const ACTIONS: { value: RuleActionType; label: string }[] = [
  { value: "move", label: t("Déplacer vers") },
  { value: "markRead", label: t("Marquer comme lu") },
  { value: "flag", label: t("Marquer d'un indicateur") },
  { value: "delete", label: t("Supprimer (vers la corbeille)") },
];

// ------------------------------------------------------------------ stockage

const KEY = "vortex.rules";
/**
 * Version 3 : empreinte complétée de l'UID (`doneKey`). La version 1 gardait
 * des numéros de message, la version 2 confondait deux messages du même
 * expéditeur, à la même seconde, de même objet (relecture complète n° 50).
 * À la première lecture de la version 3, chaque boîte est seulement notée :
 * les règles ne repassent pas sur les messages déjà présents.
 */
const DONE_KEY = "vortex.rules.done.v3";
/**
 * Messages déplacés par une règle (empreinte → date) : les règles des boîtes
 * de réception ne les reclassent plus, sinon deux règles qui se renvoient un
 * message d'une boîte à l'autre le feraient circuler à chaque synchronisation
 * (lot B1). Gardés 30 jours.
 */
const MOVED_KEY = "vortex.rules.moved";
const MOVED_MS = 30 * 86_400_000;
const CHANGED = "vortex-rules";

export function loadRules(): Rule[] {
  try {
    const list = JSON.parse(localStorage.getItem(KEY) ?? "[]") as Rule[];
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

export function saveRules(rules: Rule[]) {
  try {
    localStorage.setItem(KEY, JSON.stringify(rules));
  } catch {
    // stockage indisponible : règles valables jusqu'à la fermeture
  }
  window.dispatchEvent(new Event(CHANGED));
}

export function useRules(): Rule[] {
  const [rules, setRules] = useState(loadRules);
  useEffect(() => {
    const reload = () => setRules(loadRules());
    window.addEventListener(CHANGED, reload);
    window.addEventListener("storage", reload);
    return () => {
      window.removeEventListener(CHANGED, reload);
      window.removeEventListener("storage", reload);
    };
  }, []);
  return rules;
}

export function newRule(account = "all"): Rule {
  return {
    id: `r${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
    name: "",
    enabled: true,
    account,
    match: "all",
    conditions: [{ field: "from", op: "contains", value: "" }],
    actions: [{ type: "move", folderId: "" }],
    stop: true,
  };
}

/**
 * Empreinte stable d'un message : les numéros de message changent d'une session
 * à l'autre (ils sont attribués au démarrage), ils ne peuvent donc pas servir à
 * savoir quels messages les règles ont déjà examinés. Hachage 53 bits (cyrb53)
 * de l'expéditeur, de la date et de l'objet.
 */
export function messageKey(m: MessageSummary): string {
  const text = `${m.fromEmail}\u0000${m.date}\u0000${m.subject}`;
  let h1 = 0xdeadbeef;
  let h2 = 0x41c6ce57;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 2654435761);
    h2 = Math.imul(h2 ^ c, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return (4294967296 * (2097151 & h2) + (h1 >>> 0)).toString(36);
}

/**
 * Empreinte d'un message dans sa boîte, pour la liste « déjà examinés » :
 * celle de `messageKey`, complétée de l'UID (deux alertes identiques
 * arrivées dans la même seconde restent deux messages). `messageKey` seule
 * sert aux messages déplacés, dont l'UID change d'un dossier à l'autre.
 */
export function doneKey(m: MessageSummary): string {
  return m.uid ? `${messageKey(m)}.${m.uid}` : messageKey(m);
}

function loadDone(): Record<string, string[]> {
  try {
    // Anciennes versions : numéros instables, puis empreintes sans UID.
    localStorage.removeItem("vortex.rules.done");
    localStorage.removeItem("vortex.rules.done.v2");
    return JSON.parse(localStorage.getItem(DONE_KEY) ?? "{}") as Record<string, string[]>;
  } catch {
    return {};
  }
}

function saveDone(done: Record<string, string[]>) {
  try {
    localStorage.setItem(DONE_KEY, JSON.stringify(done));
  } catch {
    // stockage indisponible : examen refait au prochain démarrage (sans effet sur les messages déjà classés)
  }
}

function loadMoved(): Record<string, number> {
  try {
    const moved = JSON.parse(localStorage.getItem(MOVED_KEY) ?? "{}") as unknown;
    return moved && typeof moved === "object" ? (moved as Record<string, number>) : {};
  } catch {
    return {};
  }
}

/** Note les messages déplacés par une règle ; oublie ceux de plus de 30 jours. */
function noteMoved(keys: string[], now = Date.now()) {
  if (keys.length === 0) return;
  const moved = loadMoved();
  for (const k of keys) moved[k] = now;
  const kept = Object.fromEntries(Object.entries(moved).filter(([, at]) => now - at < MOVED_MS));
  try {
    localStorage.setItem(MOVED_KEY, JSON.stringify(kept));
  } catch {
    // stockage indisponible : protection contre les boucles valable jusqu'à la fermeture seulement
  }
}

// ------------------------------------------------------------------ évaluation

/** Minuscules, sans accents ni espaces superflus. */
const norm = (s: string) =>
  s
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/\s+/g, " ")
    .trim();

function textMatches(op: RuleOp, candidates: string[], value: string): boolean {
  const v = norm(value);
  const test = (c: string) => {
    const t = norm(c);
    switch (op) {
      case "contains":
        return t.includes(v);
      case "notContains":
        return !t.includes(v);
      case "is":
        return t === v;
      case "startsWith":
        return t.startsWith(v);
      case "endsWith":
        return t.endsWith(v);
      default:
        return false;
    }
  };
  // « ne contient pas » : vrai si aucun des textes ne contient la valeur.
  return op === "notContains" ? candidates.every(test) : candidates.some(test);
}

/** Une règle a-t-elle besoin des destinataires (lecture du message complet) ? */
const needsRecipients = (r: Rule) => r.conditions.some((c) => c.field === "recipients");

function conditionMatches(c: RuleCondition, m: MessageSummary, full: Message | null): boolean {
  switch (c.field) {
    case "attachment":
      return c.op === "no" ? !m.hasAttachments : m.hasAttachments;
    case "from":
      return textMatches(c.op, [m.fromEmail, m.fromName, `${m.fromName} <${m.fromEmail}>`], c.value);
    case "subject":
      return textMatches(c.op, [m.subject], c.value);
    case "body":
      return textMatches(c.op, [m.preview], c.value);
    case "recipients": {
      const list = full ? [...full.toList, ...full.ccList, full.to] : [];
      return textMatches(c.op, list.length ? list : [""], c.value);
    }
  }
}

export function ruleMatches(r: Rule, m: MessageSummary, full: Message | null): boolean {
  const valid = r.conditions.filter((c) => c.field === "attachment" || c.value.trim() !== "");
  if (valid.length === 0) return false;
  return r.match === "all"
    ? valid.every((c) => conditionMatches(c, m, full))
    : valid.some((c) => conditionMatches(c, m, full));
}

/** Règle utilisable : active, avec au moins une condition et une action complètes. */
export function ruleReady(r: Rule): boolean {
  const conditions = r.conditions.some((c) => c.field === "attachment" || c.value.trim() !== "");
  const actions = r.actions.some((a) => a.type !== "move" || !!a.folderId);
  return r.enabled && conditions && actions;
}

export interface RulesReport {
  /** Messages concernés par au moins une règle. */
  matched: number;
  moved: number;
  failed: string[];
  /** Messages non examinés (message complet illisible) : à retenter plus tard. */
  skipped?: number[];
  /** Messages déplacés, supprimés ou marqués comme lus par une règle. */
  handled?: number[];
}

/** Passage des règles sur les boîtes de réception, et ce qui y est arrivé. */
export interface InboxReport extends RulesReport {
  /**
   * Nouveaux messages non lus restés dans leur boîte après les règles : ceux
   * qu'on annonce (lot B4 ; un message mis à la corbeille par une règle ne
   * produit plus de notification).
   */
  arrivals: MessageSummary[];
}

/**
 * Applique les règles aux messages (tous du même dossier `folder`) : indicateurs
 * d'abord, puis un seul déplacement par message (le premier demandé ; une
 * suppression l'emporte sur un déplacement).
 */
export async function applyRules(messages: MessageSummary[], folder: Folder, rules: Rule[]): Promise<RulesReport> {
  const active = rules.filter((r) => ruleReady(r) && (r.account === "all" || r.account === folder.accountId));
  const report: RulesReport = { matched: 0, moved: 0, failed: [], skipped: [], handled: [] };
  if (active.length === 0 || messages.length === 0) return report;

  const seen: number[] = [];
  const flagged: number[] = [];
  const trash: number[] = [];
  const moves = new Map<string, number[]>();

  for (const m of messages) {
    let full: Message | null = null;
    let destination: string | null = null;
    let remove = false;
    let hit = false;
    let unreadable = false;
    for (const r of active) {
      if (needsRecipients(r) && !full) {
        try {
          full = await api.getMessage(m.id);
        } catch {
          full = null;
        }
        // Destinataires inconnus : une règle « ne contiennent pas » serait
        // vraie à tort (relecture F9). Le message n'est pas examiné cette
        // fois-ci ; il le sera au prochain passage.
        if (!full) {
          unreadable = true;
          break;
        }
      }
      if (!ruleMatches(r, m, full)) continue;
      hit = true;
      for (const a of r.actions) {
        if (a.type === "markRead" && !m.flags.seen) seen.push(m.id);
        if (a.type === "flag" && !m.flags.flagged) flagged.push(m.id);
        if (a.type === "delete") remove = true;
        if (a.type === "move" && a.folderId && a.folderId !== folder.id && destination === null) destination = a.folderId;
      }
      if (r.stop) break;
    }
    if (unreadable) {
      report.skipped?.push(m.id);
      continue;
    }
    if (!hit) continue;
    report.matched++;
    if (remove) trash.push(m.id);
    else if (destination) moves.set(destination, [...(moves.get(destination) ?? []), m.id]);
  }
  report.handled = [...seen, ...trash, ...[...moves.values()].flat()];

  const unique = (ids: number[]) => [...new Set(ids)];
  // Messages effectivement déplacés (ou mis en attente hors connexion) : notés
  // pour que les règles des boîtes de réception ne les reclassent plus.
  const keyOf = new Map(messages.map((m) => [m.id, messageKey(m)]));
  const movedKeys: string[] = [];
  const note = (ids: number[], r: MoveReport) => {
    const failed = new Set(r.failed.map((f) => f.id));
    for (const id of ids) if (!failed.has(id)) movedKeys.push(keyOf.get(id) ?? "");
  };
  try {
    if (seen.length) await api.setSeen(unique(seen), true);
    if (flagged.length) await api.setFlagged(unique(flagged), true);
  } catch (e) {
    report.failed.push(String(e));
  }
  for (const [target, ids] of moves) {
    try {
      const r = await api.moveMessages(ids, target);
      note(ids, r);
      report.moved += r.moved.length + (r.queued ?? 0);
      report.failed.push(...r.failed.map((f) => f.reason));
    } catch (e) {
      report.failed.push(String(e));
    }
  }
  if (trash.length) {
    try {
      const r = await api.moveToRole(trash, "trash");
      note(trash, r);
      report.moved += r.moved.length + (r.queued ?? 0);
      report.failed.push(...r.failed.map((f) => f.reason));
    } catch (e) {
      report.failed.push(String(e));
    }
  }
  noteMoved(movedKeys.filter(Boolean));
  return report;
}

let running = false;

/**
 * Nouveaux messages des boîtes de réception : chaque boîte est synchronisée,
 * puis les messages jamais examinés passent par les règles (s'il y en a), et
 * les non-lus qui restent sont rendus dans `arrivals`, pour les annoncer. À la première
 * rencontre d'une boîte, les messages présents sont seulement notés (les
 * règles ne s'appliquent qu'aux arrivées ; « Appliquer maintenant » pour le reste).
 * Les messages arrivés pendant que l'application était fermée sont des arrivées
 * comme les autres : ils passent par les règles au premier tour. Un message
 * qu'une règle a déjà déplacé n'est plus reclassé (protection contre les boucles).
 */
export async function runInboxRules(folders: Folder[]): Promise<InboxReport | null> {
  if (running) return null;
  const rules = loadRules().filter(ruleReady);
  running = true;
  const total: InboxReport = { matched: 0, moved: 0, failed: [], arrivals: [] };
  try {
    const inboxes = folders.filter((f) => f.role === "inbox");
    if (inboxes.length === 0) return null;
    const results = await api.syncMessagesMany(inboxes.map((f) => f.id));
    const done = loadDone();
    const moved = loadMoved();
    for (const r of results) {
      if (r.error) continue;
      const folder = inboxes.find((f) => f.id === r.folderId);
      if (!folder) continue;
      const known = done[folder.id];
      const keys = r.messages.map(doneKey);
      if (!known) {
        done[folder.id] = keys;
        continue;
      }
      const knownSet = new Set(known);
      const isMoved = (m: MessageSummary) => moved[messageKey(m)] !== undefined;
      // Message reclassé ici par une règle (depuis une autre boîte) : pas
      // reclassé une seconde fois, mais annoncé s'il arrive non lu (n° 50).
      total.arrivals.push(...r.messages.filter((m, i) => !knownSet.has(keys[i]) && isMoved(m) && !m.flags.seen));
      const fresh = r.messages.filter((m, i) => !knownSet.has(keys[i]) && !isMoved(m));
      if (fresh.length === 0) {
        // Rien à classer : la liste suit quand même la boîte, déplacés par une
        // règle compris (ils ne redeviendront pas « nouveaux » dans 30 jours).
        done[folder.id] = keys;
        continue;
      }
      const report = await applyRules(fresh, folder, rules);
      total.matched += report.matched;
      total.moved += report.moved;
      total.failed.push(...report.failed);
      // Messages présents dans la boîte, tous examinés sauf ceux à retenter : la
      // liste suit la boîte (aucune limite arbitraire qui ferait revenir des
      // messages déjà traités, relecture F3).
      const skipped = new Set(report.skipped ?? []);
      // Un message à retenter sera annoncé au passage où les règles l'auront vu.
      const handled = new Set(report.handled ?? []);
      total.arrivals.push(...fresh.filter((m) => !m.flags.seen && !handled.has(m.id) && !skipped.has(m.id)));
      done[folder.id] = r.messages.filter((m) => !skipped.has(m.id)).map(doneKey);
    }
    saveDone(done);
    return total;
  } catch {
    return null;
  } finally {
    running = false;
  }
}

/**
 * Notification du bureau pour les nouveaux messages restés dans les boîtes de
 * réception, une par compte : expéditeur et objet pour un message seul, le
 * nombre au-delà (arrivée groupée, synchronisation d'un autre appareil).
 */
export function announceArrivals(arrivals: MessageSummary[], accounts: { id: string; name: string }[]) {
  const byAccount = new Map<string, MessageSummary[]>();
  for (const m of arrivals) byAccount.set(m.accountId, [...(byAccount.get(m.accountId) ?? []), m]);
  for (const [accountId, list] of byAccount) {
    const name = accounts.find((a) => a.id === accountId)?.name ?? accountId;
    const [summary, body] =
      list.length === 1
        ? [list[0].fromName || list[0].fromEmail || t("Nouveau message"), `${list[0].subject.trim() || t("(sans objet)")}\n${name}`]
        : [t("{n} nouveaux messages", { n: list.length }), name];
    api.notifyNewMail(summary, body).catch(() => {
      // notification du bureau indisponible : la liste des messages suffit
    });
  }
}

/** « Appliquer maintenant » : toutes les règles actives, sur tout un dossier. */
export async function runRulesOnFolder(folder: Folder, rules = loadRules()): Promise<RulesReport> {
  const messages = await api.syncMessages(folder.id).catch(() => api.listMessages(folder.id));
  return applyRules(messages, folder, rules);
}

/** Libellé d'un dossier, compte compris : « Perso › Factures › 2026 ». */
export function folderLabel(f: Folder, folders: Folder[], accountName: (id: string) => string): string {
  const names: string[] = [];
  let cur: Folder | undefined = f;
  for (let i = 0; cur && i < 20; i++) {
    names.unshift(cur.name);
    const parent: string | null = cur.parentId;
    cur = parent ? folders.find((x) => x.id === parent) : undefined;
  }
  return `${accountName(f.accountId)} › ${names.join(" › ")}`;
}
