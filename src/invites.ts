// Rendez-vous où l'on est invité (lot H1b) : réponse du compte, lien de
// visioconférence, description sans le bloc ajouté par le service de visio,
// et invitations en attente à signaler.
import { api } from "./api";
import { instant } from "./calendar";
import { t } from "./i18n";
import { trimEndOf } from "./text";
import type { Account, Attendee, CalEvent, InvitationAnswer } from "./types";

const lower = (s: string) => s.trim().toLowerCase();

/** Ligne d'invité d'un des comptes (`emails`), s'il y en a une. */
export function myAttendee(e: CalEvent, emails: string[]): Attendee | undefined {
  const mine = new Set(emails.map(lower));
  return e.attendees?.find((a) => mine.has(lower(a.email)));
}

/** Rendez-vous organisé par quelqu'un d'autre, où un des comptes est invité. */
export function isInvited(e: CalEvent, emails: string[]): boolean {
  const organizer = lower(e.organizerEmail ?? "");
  if (!organizer || emails.some((m) => lower(m) === organizer)) return false;
  return !!myAttendee(e, emails);
}

/** Réponse écrite sur une ligne d'invité, `null` : en attente. */
export function answerOf(a: Attendee | undefined): InvitationAnswer | null {
  switch (a?.partstat) {
    case "ACCEPTED":
      return "accepted";
    case "TENTATIVE":
      return "tentative";
    case "DECLINED":
      return "declined";
    default:
      return null;
  }
}

/** Réponse déjà donnée par le premier compte invité, `null` : en attente. */
export function myAnswer(e: CalEvent, emails: string[]): InvitationAnswer | null {
  return answerOf(myAttendee(e, emails));
}

/** Comptes invités au rendez-vous, dans l'ordre des comptes (lot H1b). */
export function invitedAccounts(e: CalEvent, accounts: Account[]): Account[] {
  const listed = new Set((e.attendees ?? []).map((a) => lower(a.email)));
  return accounts.filter((a) => listed.has(lower(a.email)));
}

/** Tous les comptes invités ont refusé : le rendez-vous ne s'affiche plus. */
export function allDeclined(e: CalEvent, emails: string[]): boolean {
  const mine = new Set(emails.map(lower));
  const lines = (e.attendees ?? []).filter((a) => mine.has(lower(a.email)));
  return lines.length > 0 && lines.every((a) => a.partstat === "DECLINED");
}

/** Services de visioconférence reconnus, et le nom montré sur le bouton. */
const MEETINGS: [RegExp, string][] = [
  [/https:\/\/meet\.google\.com\/[a-z0-9-]+/i, "Google Meet"],
  [/https:\/\/teams\.(?:microsoft|live)\.com\/[^\s<>"]+/i, "Microsoft Teams"],
  [/https:\/\/(?:[a-z0-9-]+\.)?zoom\.us\/[jw]\/[^\s<>"]+/i, "Zoom"],
  [/https:\/\/[a-z0-9-]+\.webex\.com\/[^\s<>"]+/i, "Webex"],
  [/https:\/\/meet\.jit\.si\/[^\s<>"]+/i, "Jitsi Meet"],
];

/** Lien de visioconférence du rendez-vous (lieu, puis description). */
export function meetingLink(e: CalEvent): { url: string; service: string } | null {
  for (const text of [e.location, e.description]) {
    for (const [pattern, service] of MEETINGS) {
      const found = pattern.exec(text ?? "");
      if (found) return { url: trimEndOf(found[0], ").,;"), service };
    }
  }
  return null;
}

/**
 * Description sans le bloc ajouté par Google Meet (entre deux lignes
 * `-::~:~::~…`) ni celui de Microsoft Teams (entre deux lignes de `_`) : le
 * bouton « Participer » le remplace. Le rendez-vous enregistré ne change pas.
 */
export function cleanDescription(text: string): string {
  const lines = (text ?? "").replace(/\r\n/g, "\n").split("\n");
  const google = (l: string) => /^-?::~[:~]*::-?$/.test(l.trim());
  const rule = (l: string) => /^_{20,}$/.test(l.trim());
  const cut = (isMark: (l: string) => boolean, needle: RegExp) => {
    const first = lines.findIndex(isMark);
    const last = lines.length - 1 - [...lines].reverse().findIndex(isMark);
    if (first >= 0 && last > first && needle.test(lines.slice(first, last + 1).join("\n"))) lines.splice(first, last - first + 1);
  };
  cut(google, /meet\.google\.com|google/i);
  cut(rule, /teams\.(?:microsoft|live)\.com/i);
  return lines.join("\n").replace(/\n{3,}/g, "\n\n").trim();
}

const NOTIFIED = "vortex.invitations.notified";

/** Clé d'une version d'invitation : une invitation modifiée est signalée de nouveau. */
const keyOf = (e: CalEvent) => `${e.uid}|${e.recurrenceId}|${e.sequence ?? 0}`;

/** Invitations à venir qui attendent une réponse d'un des comptes. */
export function pendingInvitations(events: CalEvent[], emails: string[], now = Date.now()): CalEvent[] {
  return events.filter((e) => {
    const start = instant(e.start);
    const repeats = !!e.rrule;
    return isInvited(e, emails) && myAnswer(e, emails) === null && e.status !== "CANCELLED" && (repeats || !(start < now));
  });
}

/**
 * Signale les nouvelles invitations en attente (lot H1b), une fois chacune.
 * Au tout premier passage, celles déjà là sont notées sans être signalées.
 * Renvoie les invitations signalées.
 */
export function newInvitations(events: CalEvent[], emails: string[], now = Date.now()): CalEvent[] {
  const pending = pendingInvitations(events, emails, now);
  let known: string[] | null = null;
  try {
    known = JSON.parse(localStorage.getItem(NOTIFIED) ?? "null") as string[] | null;
  } catch {
    known = null;
  }
  const seen = new Set(known ?? []);
  const fresh = known === null ? [] : pending.filter((e) => !seen.has(keyOf(e)));
  try {
    // Seules les invitations encore en attente sont gardées : la liste reste courte.
    localStorage.setItem(NOTIFIED, JSON.stringify(pending.map(keyOf)));
  } catch {
    // stockage indisponible : au pire, une invitation est signalée de nouveau
  }
  return fresh;
}

/** Notification du bureau pour chaque nouvelle invitation. */
export function announceInvitations(fresh: CalEvent[], when: (e: CalEvent) => string) {
  for (const e of fresh.slice(0, 5)) {
    const title = e.summary || t("(sans titre)");
    const by = e.organizer ? t("De {name}", { name: e.organizer }) : "";
    api.showReminder(t("Nouvelle invitation : {title}", { title }), [when(e), by].filter(Boolean).join("\n")).catch(() => {});
  }
}
