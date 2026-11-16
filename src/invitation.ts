// Invitation reçue (.ics) : selon sa méthode (RFC 5546), ajout ou mise à jour
// du rendez-vous dans l'agenda, ou retrait s'il est annulé (relecture du
// 12 novembre, n° 7). Une réponse d'invité n'est pas proposée à l'ajout.
import { api } from "./api";
import { durationMs, exdateFor, instant } from "./calendar";
import { formatDate, formatTime } from "./general";
import { locale, t } from "./i18n";
import type { AttachmentInfo, CalEvent, Invitation, InvitationAnswer, Message } from "./types";

/** Pièce jointe d'invitation, s'il y en a une. */
export function invitationOf(m: Message): AttachmentInfo | undefined {
  return m.attachments.find((a) => a.contentType === "text/calendar" || /\.ics$/i.test(a.name));
}

/**
 * Ce que propose l'invitation :
 * - `add` : nouveau rendez-vous ;
 * - `update` : version plus récente d'un rendez-vous de l'agenda (même UID) ;
 * - `current` : déjà dans l'agenda, à jour ;
 * - `outdated` : l'agenda en a une version plus récente ;
 * - `cancel` : annulation d'un rendez-vous de l'agenda ;
 * - `gone` : annulation d'un rendez-vous absent de l'agenda ;
 * - `reply` : réponse d'un invité (`REPLY`, `COUNTER`…), rien à ajouter.
 */
export type InvitationKind = "add" | "update" | "current" | "outdated" | "cancel" | "gone" | "reply";

export interface InvitationPlan {
  kind: InvitationKind;
  invitation: Invitation;
  /** Rendez-vous de l'invitation (la série, sinon le premier). */
  event: CalEvent;
  /** Rendez-vous de même UID déjà dans l'agenda (la série de préférence). */
  existing?: CalEvent;
}

const REPLIES = new Set(["REPLY", "COUNTER", "DECLINECOUNTER", "REFRESH"]);

/** Même contenu visible (horaire, titre, lieu, répétition). */
function sameContent(a: CalEvent, b: CalEvent): boolean {
  return (
    instant(a.start) === instant(b.start) &&
    (a.end.value ? instant(a.end) : 0) === (b.end.value ? instant(b.end) : 0) &&
    a.summary === b.summary &&
    a.location === b.location &&
    a.rrule === b.rrule
  );
}

/** Choix d'après l'invitation et les rendez-vous en cache. */
export function planFor(invitation: Invitation, events: CalEvent[]): InvitationPlan {
  const event = invitation.events.find((e) => !e.recurrenceId) ?? invitation.events[0];
  const same = events.filter((e) => e.uid === event.uid);
  const existing = same.find((e) => !e.recurrenceId) ?? same[0];
  const method = (invitation.method ?? "").toUpperCase();
  let kind: InvitationKind;
  if (REPLIES.has(method)) kind = "reply";
  else if (method === "CANCEL" || event.status === "CANCELLED") kind = existing ? "cancel" : "gone";
  else if (!existing) kind = "add";
  else {
    const theirs = event.sequence ?? 0;
    const ours = existing.sequence ?? 0;
    if (theirs < ours) kind = "outdated";
    else kind = theirs > ours || !sameContent(event, existing) ? "update" : "current";
  }
  return { kind, invitation, event, existing };
}

const longWeekday = new Intl.DateTimeFormat(locale(), { weekday: "long" });

/** Jour en toutes lettres : « Jeudi 12/11/2026 ». */
function dayLabel(t: number): string {
  const d = new Date(t);
  const s = `${longWeekday.format(d)} ${formatDate(d)}`;
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function sameDay(a: number, b: number): boolean {
  return new Date(a).toDateString() === new Date(b).toDateString();
}

/**
 * Date et heure d'un rendez-vous, dans l'heure locale : « Jeudi 12/11/2026 ·
 * 11:00 – 13:00 », ou les jours seuls pour une journée entière (lot H1).
 */
export function eventWhen(e: CalEvent): string {
  const start = instant(e.start);
  if (Number.isNaN(start)) return "";
  const allDay = e.allDay || e.start.value.length === 8;
  const end = e.end.value ? instant(e.end) : start + durationMs(e.duration || (allDay ? "P1D" : "PT0S"));
  return rangeLabel(start, end, allDay);
}

/** Début et fin (ms, fin exclusive pour une journée entière) mis en forme comme [`eventWhen`]. */
export function rangeLabel(start: number, end: number, allDay: boolean): string {
  if (Number.isNaN(start)) return "";
  if (allDay) {
    // Fin exclusive : la veille du jour de fin.
    const last = Number.isNaN(end) || end <= start ? start : end - 1;
    return sameDay(start, last) ? dayLabel(start) : `${dayLabel(start)} – ${dayLabel(last)}`;
  }
  if (Number.isNaN(end) || end <= start) return `${dayLabel(start)} · ${formatTime(new Date(start))}`;
  if (sameDay(start, end)) return `${dayLabel(start)} · ${formatTime(new Date(start))} – ${formatTime(new Date(end))}`;
  return `${dayLabel(start)} ${formatTime(new Date(start))} – ${dayLabel(end)} ${formatTime(new Date(end))}`;
}

/** Invitation à laquelle répondre (lot H1) : demande (`REQUEST`) encore d'actualité. */
export function canAnswer(plan: InvitationPlan): boolean {
  return (plan.invitation.method ?? "").toUpperCase() === "REQUEST" && ["add", "update", "current"].includes(plan.kind);
}

export async function planInvitation(m: Message, a: AttachmentInfo): Promise<InvitationPlan> {
  const [invitation, events] = await Promise.all([api.readInvitation(m.id, a.index, m.accountId), api.events()]);
  return planFor(invitation, events);
}

/**
 * Applique l'invitation : ajout à l'agenda du compte qui l'a reçue (sinon au
 * premier agenda synchronisé, sinon « Sur cet ordinateur »), mise à jour du
 * rendez-vous en place, ou retrait. Renvoie le titre de la notification et
 * celui du rendez-vous.
 */
export async function applyInvitation(m: Message, a: AttachmentInfo): Promise<{ done: string; title: string }> {
  const plan = await planInvitation(m, a);
  const { invitation, event, existing } = plan;
  const title = event.summary || existing?.summary || t("(sans titre)");
  switch (plan.kind) {
    case "add":
      await saveToAgenda(m, plan, invitation.raw);
      return { done: t("Ajouté à l'agenda"), title };
    case "update": {
      if (!existing) throw new Error(t("Rendez-vous introuvable dans l'agenda."));
      await saveToAgenda(m, plan, invitation.raw);
      return { done: t("Agenda mis à jour"), title };
    }
    case "cancel": {
      if (!existing) throw new Error(t("Rendez-vous introuvable dans l'agenda."));
      await removeFromAgenda(event, existing);
      return { done: t("Retiré de l'agenda"), title };
    }
    default:
      return { done: t("Rien à changer dans l'agenda"), title };
  }
}

/**
 * Ajoute le rendez-vous à l'agenda du compte qui l'a reçu (sinon au premier
 * agenda synchronisé, sinon « Sur cet ordinateur »), ou le met à jour en place.
 */
async function saveToAgenda(m: Message, plan: InvitationPlan, raw: string) {
  const { event, existing } = plan;
  if (existing) {
    await api.saveEvent({ ...event, id: existing.id, source: existing.source, calendar: existing.calendar }, existing.calendar, raw);
    return;
  }
  const calendars = (await api.pimCollections()).filter((c) => c.kind === "calendar");
  const target = calendars.find((c) => c.account === m.accountId) ?? calendars.find((c) => c.account !== "local") ?? calendars[0];
  const url = target?.url ?? "local:calendar";
  await api.saveEvent({ ...event, id: "", source: "", calendar: url }, url, raw);
}

/** Retire le rendez-vous, ou la seule occurrence visée, de sa série. */
async function removeFromAgenda(event: CalEvent, existing: CalEvent) {
  if (event.recurrenceId && existing.rrule) {
    const at = instant({ value: event.recurrenceId, tzid: event.start.tzid || existing.start.tzid });
    await api.saveEvent({ ...existing, exdates: [...existing.exdates, exdateFor(existing, at)] }, existing.calendar);
  } else {
    await api.pimDelete(existing.id);
  }
}

/**
 * Répond à l'invitation (lot H1), comme Thunderbird : Accepter et Peut-être
 * ajoutent ou mettent à jour le rendez-vous avec la réponse, Refuser le retire
 * s'il est dans l'agenda. Avec `send`, la réponse part aussi à l'organisateur.
 * `asAccount` : autre compte invité au nom duquel répondre (lot H1b).
 */
export async function answerInvitation(
  m: Message,
  a: AttachmentInfo,
  answer: InvitationAnswer,
  send: boolean,
  asAccount?: string,
): Promise<{ done: string; title: string }> {
  const plan = await planInvitation(m, a);
  const { event, existing } = plan;
  const title = event.summary || existing?.summary || t("(sans titre)");
  const { raw } = await api.answerInvitation(m.id, a.index, m.accountId, answer, send, eventWhen(event), asAccount);
  if (answer === "declined") {
    if (existing) await removeFromAgenda(event, existing);
    return { done: send ? t("Refus envoyé à l'organisateur") : t("Invitation refusée"), title };
  }
  await saveToAgenda(m, plan, raw);
  return { done: send ? t("Réponse envoyée, agenda mis à jour") : t("Agenda mis à jour"), title };
}
