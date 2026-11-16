// Tests des rappels manqués (lot B2) : `npm test`, lancé par `make audit`.
// Fuseau fixé à Europe/Paris par setup.mjs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { due, missed, saveFired } from "../../src/reminders.ts";

const event = (fields) => ({
  id: "cal|1",
  source: "local",
  calendar: "local:calendar",
  uid: "u1",
  summary: "Réunion",
  location: "",
  description: "",
  start: { value: "", tzid: "" },
  end: { value: "", tzid: "" },
  duration: "",
  allDay: false,
  rrule: "",
  exdates: [],
  recurrenceId: "",
  organizer: "",
  reminder: 15,
  ...fields,
});
const at = (d, h, min = 0) => new Date(2026, 10, d, h, min).getTime();
const timed = (uid, d, h, extra = {}) =>
  event({
    uid,
    id: `cal|${uid}`,
    summary: uid,
    start: { value: `202611${String(d).padStart(2, "0")}T${String(h).padStart(2, "0")}0000`, tzid: "Europe/Paris" },
    end: { value: `202611${String(d).padStart(2, "0")}T${String(h + 1).padStart(2, "0")}0000`, tzid: "Europe/Paris" },
    ...extra,
  });

test("B2 : rappels manqués pendant la fermeture, du plus ancien au plus récent", () => {
  // Fermée le 12 à 8 h, rouverte le 12 à 18 h.
  const events = [timed("midi", 12, 12), timed("matin", 12, 9), timed("demain", 13, 9), timed("hier", 11, 15)];
  const list = missed(events, {}, at(12, 8), at(12, 18));
  assert.deepEqual(
    list.map((x) => x.o.event.uid),
    ["matin", "midi"],
    "seuls les rendez-vous dont le rappel tombait pendant la fermeture",
  );
});

test("B2 : ni rappel déjà montré, ni rappel encore à venir, ni au-delà de 24 h", () => {
  const events = [timed("montre", 12, 9), timed("bientot", 12, 18), timed("vieux", 10, 9)];
  const first = missed(events, {}, at(9, 8), at(12, 18));
  assert.deepEqual(
    first.map((x) => x.o.event.uid),
    ["montre"],
    "le rendez-vous de 18 h n'est pas manqué (due s'en charge) ; celui du 10 a plus de 24 h",
  );
  const fired = { [first[0].key]: at(12, 9) };
  assert.equal(missed(events, fired, at(9, 8), at(12, 18)).length, 0, "un rappel déjà montré ne revient pas");
});

test("B2 : sans rappel réglé, rien n'est signalé", () => {
  const list = missed([timed("sans", 12, 9, { reminder: null })], {}, at(12, 8), at(12, 18));
  assert.equal(list.length, 0, "rendez-vous sans rappel");
});

test("n° 25 : rappel deux semaines avant, à l'heure et une seule fois", () => {
  const twoWeeks = 14 * 24 * 60;
  const events = [timed("loin", 26, 9, { reminder: twoWeeks })];
  // Le 12 à 9 h 01 : le rappel du 26 à 9 h est dû (l'ancienne fenêtre de 8 jours l'ignorait).
  const list = due(events, {}, at(12, 9, 1));
  assert.deepEqual(list.map((x) => x.o.event.uid), ["loin"]);
  assert.equal(due(events, {}, at(12, 8, 59)).length, 0, "pas avant l'heure");
  // Montré il y a plus de 8 jours, rendez-vous à venir : la trace est gardée.
  const now = Date.now();
  const kept = saveFired({ [`loin|${now + 6 * 86_400_000}|${twoWeeks}`]: now - 9 * 86_400_000, [`vieux|${now - 9 * 86_400_000}|15`]: now - 9 * 86_400_000 });
  assert.deepEqual(Object.keys(kept).map((k) => k.split("|")[0]), ["loin"]);
});
