// Invitations selon leur méthode (relecture du 12 novembre, n° 7) : `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { api } from "../../src/api.ts";
import { t } from "../../src/i18n.ts";
import { answerInvitation, applyInvitation, eventWhen, planFor, rangeLabel } from "../../src/invitation.ts";

const event = (fields) => ({
  id: "",
  source: "",
  calendar: "",
  uid: "reunion@exemple.fr",
  summary: "Réunion",
  location: "Salle A",
  description: "",
  start: { value: "20261120T090000", tzid: "Europe/Paris" },
  end: { value: "20261120T100000", tzid: "Europe/Paris" },
  duration: "",
  allDay: false,
  rrule: "",
  exdates: [],
  recurrenceId: "",
  organizer: "o@exemple.fr",
  sequence: 0,
  status: "",
  ...fields,
});
const invitation = (method, fields = {}) => ({ events: [event(fields)], raw: "", method });
const stored = event({ id: "cal|1", calendar: "cal", sequence: 1 });

test("n° 7 : demande, mise à jour, version plus ancienne", () => {
  assert.equal(planFor(invitation("REQUEST"), []).kind, "add");
  assert.equal(planFor(invitation(""), []).kind, "add", "simple publication");
  assert.equal(planFor(invitation("REQUEST", { sequence: 2, location: "Salle B" }), [stored]).kind, "update");
  assert.equal(planFor(invitation("REQUEST", { sequence: 1 }), [stored]).kind, "current");
  assert.equal(planFor(invitation("REQUEST", { sequence: 1, location: "Salle C" }), [stored]).kind, "update", "même version, contenu plus récent");
  assert.equal(planFor(invitation("REQUEST", { sequence: 0 }), [stored]).kind, "outdated");
  assert.equal(planFor(invitation("REQUEST", { sequence: 2 }), [stored]).existing, stored);
});

test("n° 7 : annulation et réponse d'invité", () => {
  assert.equal(planFor(invitation("CANCEL", { sequence: 2 }), [stored]).kind, "cancel");
  assert.equal(planFor(invitation("CANCEL"), []).kind, "gone");
  assert.equal(planFor(invitation("REQUEST", { status: "CANCELLED" }), [stored]).kind, "cancel");
  for (const method of ["REPLY", "COUNTER", "reply"]) assert.equal(planFor(invitation(method), [stored]).kind, "reply", method);
});

test("S6 : mise à jour ou annulation d'un autre organisateur signalée", () => {
  const other = { organizer: "Intrus", organizerEmail: "intrus@ailleurs.fr" };
  const update = planFor(invitation("REQUEST", { sequence: 2, location: "Salle B", ...other }), [stored]);
  assert.equal(update.kind, "update");
  assert.deepEqual(update.otherOrganizer, { theirs: "intrus@ailleurs.fr", ours: "o@exemple.fr" });
  const cancel = planFor(invitation("CANCEL", { sequence: 2, ...other }), [stored]);
  assert.deepEqual(cancel.otherOrganizer, { theirs: "intrus@ailleurs.fr", ours: "o@exemple.fr" });
  // Même organisateur, écrit autrement : rien à signaler.
  assert.equal(planFor(invitation("CANCEL", { sequence: 2, organizer: "mailto:O@Exemple.fr" }), [stored]).otherOrganizer, undefined);
  // Nouveau rendez-vous : aucun organisateur à comparer.
  assert.equal(planFor(invitation("REQUEST", other), []).otherOrganizer, undefined);
});

// Date d'un rendez-vous en clair (lot H1). Fuseau Europe/Paris (setup.mjs).
const local = (d, h = 0, min = 0) => new Date(2026, 10, d, h, min).getTime();

test("H1 : date et heure d'un rendez-vous", () => {
  assert.equal(rangeLabel(Number.NaN, 0, false), "");
  const day = rangeLabel(local(20), local(21), true);
  assert.ok(!day.includes("–"), `journée entière d'un jour : ${day}`);
  assert.equal(rangeLabel(local(20), local(20), true), day, "fin absente ou avant le début : le jour seul");
  assert.match(rangeLabel(local(20), local(23), true), /^.+ – .+$/, "fin exclusive : du 20 au 22");
  assert.match(rangeLabel(local(20, 11), local(20, 13), false), /^.+ · .+ – .+$/, "même jour : un jour, deux heures");
  assert.match(rangeLabel(local(20, 11), local(20, 11), false), /^[^–]+ · [^–]+$/, "sans fin : l'heure de début");
  assert.ok(!rangeLabel(local(20, 22), local(21, 1), false).includes(" · "), "sur deux jours : jour et heure de chaque côté");
});

test("H1 : fin tirée de la durée, journée entière d'après la date", () => {
  const thirty = event({ end: { value: "", tzid: "" }, duration: "PT30M" });
  assert.equal(eventWhen(thirty), rangeLabel(local(20, 9), local(20, 9, 30), false));
  const allDay = event({ start: { value: "20261120", tzid: "" }, end: { value: "", tzid: "" } });
  assert.equal(eventWhen(allDay), rangeLabel(local(20), local(21), true), "date seule : journée entière d'un jour");
  assert.equal(eventWhen(event({ start: { value: "pas une date", tzid: "" } })), "");
});

// Ajout, mise à jour, retrait et réponse, avec un backend joué.
const message = { id: 7, accountId: "perso", attachments: [] };
const part = { index: 1, name: "invite.ics", contentType: "text/calendar" };
async function withBackend(invite, events, run) {
  const calls = [];
  const stubs = {
    readInvitation: async () => invite,
    events: async () => events,
    pimCollections: async () => [
      { kind: "contacts", account: "perso", url: "https://dav/perso/contacts" },
      { kind: "calendar", account: "local", url: "local:calendar" },
      { kind: "calendar", account: "perso", url: "https://dav/perso/agenda" },
    ],
    saveEvent: async (e, collection, raw) => {
      calls.push(["save", e, collection, raw]);
      return e;
    },
    pimDelete: async (id) => {
      calls.push(["delete", id]);
    },
    answerInvitation: async (...args) => {
      calls.push(["answer", ...args]);
      return { raw: "BEGIN:VCALENDAR (réponse)" };
    },
  };
  const saved = Object.fromEntries(Object.keys(stubs).map((k) => [k, api[k]]));
  Object.assign(api, stubs);
  try {
    const result = await run();
    return { result, calls };
  } finally {
    Object.assign(api, saved);
  }
}

test("H1 : nouvelle invitation ajoutée à l'agenda du compte qui l'a reçue", async () => {
  const invite = { ...invitation("REQUEST"), raw: "BEGIN:VCALENDAR" };
  const { result, calls } = await withBackend(invite, [], () => applyInvitation(message, part));
  assert.deepEqual(result, { done: t("Ajouté à l'agenda"), title: "Réunion" });
  const [[kind, saved, collection, raw]] = calls;
  assert.equal(kind, "save");
  assert.equal(collection, "https://dav/perso/agenda", "agenda du compte, pas « Sur cet ordinateur »");
  assert.equal(saved.calendar, collection);
  assert.equal(raw, "BEGIN:VCALENDAR");
});

test("H1 : mise à jour en place, annulation, rien à changer", async () => {
  const update = await withBackend(invitation("REQUEST", { sequence: 2, location: "Salle B" }), [stored], () => applyInvitation(message, part));
  assert.equal(update.result.done, t("Agenda mis à jour"));
  const [, saved, collection] = update.calls[0];
  assert.deepEqual([saved.id, saved.calendar, collection, saved.location], ["cal|1", "cal", "cal", "Salle B"]);

  const cancel = await withBackend(invitation("CANCEL", { sequence: 2 }), [stored], () => applyInvitation(message, part));
  assert.equal(cancel.result.done, t("Retiré de l'agenda"));
  assert.deepEqual(cancel.calls, [["delete", "cal|1"]]);

  const reply = await withBackend(invitation("REPLY"), [stored], () => applyInvitation(message, part));
  assert.deepEqual(reply.result, { done: t("Rien à changer dans l'agenda"), title: "Réunion" });
  assert.deepEqual(reply.calls, []);
});

test("H1 : annulation d'une seule occurrence retirée de la série", async () => {
  const series = event({ id: "cal|2", calendar: "cal", sequence: 1, rrule: "FREQ=WEEKLY" });
  const one = invitation("CANCEL", { sequence: 2, recurrenceId: "20261127T090000" });
  const { calls } = await withBackend(one, [series], () => applyInvitation(message, part));
  const [[kind, saved]] = calls;
  assert.equal(kind, "save", "la série est gardée");
  assert.equal(saved.exdates.length, 1, "l'occurrence du 27 en exception");
});

test("H1 : refuser retire le rendez-vous, accepter l'enregistre avec la réponse", async () => {
  const declined = await withBackend(invitation("REQUEST", { sequence: 1 }), [stored], () => answerInvitation(message, part, "declined", true));
  assert.equal(declined.result.done, t("Refus envoyé à l'organisateur"));
  assert.deepEqual(declined.calls.at(-1), ["delete", "cal|1"]);
  const [answer] = declined.calls;
  assert.deepEqual(answer.slice(0, 6), ["answer", 7, 1, "perso", "declined", true]);

  const quiet = await withBackend(invitation("REQUEST"), [], () => answerInvitation(message, part, "declined", false));
  assert.equal(quiet.result.done, t("Invitation refusée"));
  assert.equal(quiet.calls.length, 1, "absent de l'agenda : rien à retirer");

  const accepted = await withBackend(invitation("REQUEST"), [], () => answerInvitation(message, part, "accepted", false, "pro"));
  assert.equal(accepted.result.done, t("Agenda mis à jour"));
  assert.equal(accepted.calls[0][7], "pro", "au nom d'un autre compte invité");
  assert.equal(accepted.calls[1][3], "BEGIN:VCALENDAR (réponse)", "rendez-vous enregistré avec la réponse");
});
