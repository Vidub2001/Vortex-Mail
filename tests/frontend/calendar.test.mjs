// Tests de l'agenda (lot 6a) : fuseaux, séries, occurrences modifiées.
// Fuseau fixé à Europe/Paris par setup.mjs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { eventZone, occurrences, systemZone, toWallValue } from "../../src/calendar.ts";
import { buildRecurrence, parseRecurrence } from "../../src/recurrence.ts";

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
  ...fields,
});
const day = (y, m, d) => new Date(y, m - 1, d).getTime();
const hourOf = (t) => new Date(t).getHours();

test("F1 : heure locale avec fuseau, jamais UTC", () => {
  assert.equal(systemZone(), "Europe/Paris");
  assert.equal(eventZone(""), "Europe/Paris");
  assert.equal(eventZone("America/New_York"), "America/New_York");
  assert.equal(eventZone("Pas/Un_Fuseau"), "Europe/Paris");
  // 9 h à Paris le 9 novembre (hiver) = 8 h UTC.
  assert.equal(toWallValue(Date.UTC(2026, 10, 9, 8, 0, 0), "Europe/Paris"), "20261109T090000");
});

test("F1 : une réunion hebdomadaire à 9 h reste à 9 h après le changement d'heure", () => {
  const series = event({
    start: { value: "20261109T090000", tzid: "Europe/Paris" },
    end: { value: "20261109T100000", tzid: "Europe/Paris" },
    rrule: "FREQ=WEEKLY",
  });
  const list = occurrences([series], day(2027, 3, 22), day(2027, 4, 12));
  assert.equal(list.length, 3);
  for (const o of list) assert.equal(hourOf(o.start), 9, new Date(o.start).toString());
});

test("F7 et F8 : occurrence déplacée avant la date d'origine, et sa valeur d'origine", () => {
  const series = event({
    start: { value: "20261102T090000", tzid: "Europe/Paris" },
    end: { value: "20261102T100000", tzid: "Europe/Paris" },
    rrule: "FREQ=WEEKLY",
  });
  // Lundi 16 novembre déplacé au vendredi 13.
  const moved = event({
    recurrenceId: "20261116T090000",
    summary: "Réunion (avancée)",
    start: { value: "20261113T090000", tzid: "Europe/Paris" },
    end: { value: "20261113T100000", tzid: "Europe/Paris" },
  });
  const week = occurrences([series, moved], day(2026, 11, 9), day(2026, 11, 16));
  assert.deepEqual(week.map((o) => new Date(o.start).getDate()), [9, 13]);
  const friday = week[1];
  assert.equal(friday.event.summary, "Réunion (avancée)");
  assert.equal(friday.series, series);
  assert.equal(friday.occurrenceValue, "20261116T090000");
  // La semaine suivante : plus rien le lundi 16.
  const next = occurrences([series, moved], day(2026, 11, 16), day(2026, 11, 23));
  assert.deepEqual(next.map((o) => new Date(o.start).getDate()), []);
  // Supprimée par EXDATE : plus affichée du tout.
  const deleted = occurrences([{ ...series, exdates: ["20261116T090000"] }, moved], day(2026, 11, 9), day(2026, 11, 16));
  assert.deepEqual(deleted.map((o) => new Date(o.start).getDate()), [9]);
});

test("F7 (corpus) : invitation à une seule occurrence d'une série", () => {
  const eml = readFileSync(new URL("../corpus/attaques/F7-occurrence-seule.eml", import.meta.url), "utf8");
  const part = eml.split("--CORPUS").find((p) => p.includes("Content-Type: text/calendar"));
  const ical = Buffer.from(part.split(/\r?\n\r?\n/)[1].replace(/\s+/g, ""), "base64").toString("utf8");
  const prop = (name) => {
    const m = new RegExp(`^${name}(?:;TZID=([^:]+))?:(.+)$`, "m").exec(ical);
    return { value: m[2].trim(), tzid: m[1] ?? "" };
  };
  const only = event({
    uid: prop("UID").value,
    summary: prop("SUMMARY").value,
    recurrenceId: prop("RECURRENCE-ID").value,
    start: prop("DTSTART"),
    end: prop("DTEND"),
  });
  const list = occurrences([only], day(2026, 11, 9), day(2026, 11, 16));
  assert.equal(list.length, 1);
  const d = new Date(list[0].start);
  assert.deepEqual([d.getDay(), d.getDate(), d.getMonth() + 1, d.getHours()], [5, 13, 11, 9]);
});

test("F13 : série quotidienne très ancienne", () => {
  const daily = event({ start: { value: "20100104T080000", tzid: "Europe/Paris" }, end: { value: "20100104T081500", tzid: "Europe/Paris" }, rrule: "FREQ=DAILY" });
  assert.equal(occurrences([daily], day(2026, 11, 9), day(2026, 11, 16)).length, 7);
  const counted = event({ ...daily, rrule: "FREQ=DAILY;COUNT=3" });
  assert.equal(occurrences([counted], day(2026, 11, 9), day(2026, 11, 16)).length, 0);
});

test("Z11 : règle BYDAY démesurée bornée, sans figer l'affichage", () => {
  const huge = "MO,".repeat(300_000) + "TU,XX,99MO";
  const weekly = event({ start: { value: "20261102T090000", tzid: "Europe/Paris" }, end: { value: "20261102T100000", tzid: "Europe/Paris" }, rrule: `FREQ=WEEKLY;BYDAY=${huge}` });
  const started = Date.now();
  const found = occurrences([weekly], day(2026, 11, 2), day(2026, 11, 16));
  assert.ok(Date.now() - started < 2000, "calcul trop long");
  // Lundis et mardis de deux semaines, sans doublon.
  assert.equal(found.length, 4);
  const monthly = event({ ...weekly, rrule: `FREQ=MONTHLY;BYMONTHDAY=${"1,".repeat(300_000)}0,40,2` });
  assert.equal(occurrences([monthly], day(2026, 11, 1), day(2026, 12, 1)).length, 1);
});

test("F11 : la date de fin de répétition ne bouge pas d'une modification à l'autre", () => {
  const rule = buildRecurrence({ ...parseRecurrence("FREQ=WEEKLY", "2026-11-02"), end: "until", until: "2026-11-30" }, "2026-11-02", false);
  assert.match(rule, /UNTIL=20261130T225959Z/);
  const back = parseRecurrence(rule, "2026-11-02");
  assert.equal(back.until, "2026-11-30");
  assert.equal(buildRecurrence(back, "2026-11-02", false), rule);
});

test("F6 : jour de la semaine construit à partir du début de la série", () => {
  // Série du lundi 2 novembre, ouverte depuis un mercredi : on ne garde que le mercredi.
  const rec = parseRecurrence("FREQ=WEEKLY;BYDAY=MO,WE", "2026-11-02");
  const rule = buildRecurrence({ ...rec, days: [3] }, "2026-11-02", false);
  assert.match(rule, /BYDAY=WE/);
});

// --- Lot 6b : jours de changement d'heure (F10, F15) et mini-mois -----------------

import { addDays, atMinute, dayDiff, minuteOfDay, monthWeeks, toDateValue } from "../../src/calendar.ts";
import { groupByDate } from "../../src/format.ts";
import { t as tr } from "../../src/i18n.ts";

test("F15 : un rendez-vous de 10 h reste sur la ligne de 10 h les jours de changement d'heure", () => {
  // 29 mars 2026 (23 h) et 25 octobre 2026 (25 h), Europe/Paris.
  for (const [y, m, d] of [[2026, 3, 29], [2026, 10, 25]]) {
    const midnight = day(y, m, d);
    assert.equal(minuteOfDay(new Date(y, m - 1, d, 10, 0).getTime(), midnight), 600);
    assert.equal(minuteOfDay(new Date(y, m - 1, d, 23, 30).getTime(), midnight), 23 * 60 + 30);
  }
  // Bornes : avant minuit, 0 ; à partir du minuit suivant, 1440.
  const midnight = day(2026, 3, 29);
  assert.equal(minuteOfDay(midnight - 3_600_000, midnight), 0);
  assert.equal(minuteOfDay(addDays(midnight, 1), midnight), 1440);
  // Créneau choisi à la souris : 10 h 00 du jour, quelle que soit la durée de la journée.
  assert.equal(hourOf(atMinute(midnight, 600)), 10);
  assert.equal(atMinute(midnight, 1440), addDays(midnight, 1));
});

test("F15 : les messages de la veille d'un jour de 25 h sont rangés dans « Hier »", () => {
  // Le 26 octobre 2026 à 10 h ; message du 25 octobre à 0 h 30 (le 25 dure 25 h).
  const now = new Date(2026, 9, 26, 10, 0);
  const at = (y, m, d, h, min) => Math.floor(new Date(y, m - 1, d, h, min).getTime() / 1000);
  const groups = groupByDate([{ date: at(2026, 10, 25, 0, 30) }, { date: at(2026, 10, 20, 0, 30) }], now);
  assert.deepEqual(groups.map((g) => g.label), [tr("Hier"), tr("Cette semaine")]);
});

test("F10 : une journée entière le jour du changement d'heure finit le lendemain", () => {
  const start = day(2026, 10, 25);
  const end = addDays(start, 1);
  assert.equal(toDateValue(start), "20261025");
  assert.equal(toDateValue(end), "20261026");
  // L'ancien calcul (+ 24 h) restait sur le même jour.
  assert.equal(toDateValue(start + 86_400_000), "20261025");
  // Écart en jours de calendrier, malgré les jours de 23 ou 25 h.
  assert.equal(dayDiff(day(2026, 10, 24), day(2026, 10, 27)), 3);
  assert.equal(dayDiff(day(2026, 3, 29), day(2026, 3, 28)), -1);
});

test("Mini-mois : semaines qui couvrent le mois, numéros de semaine compris", () => {
  // Novembre 2026, semaine commençant le lundi : du 26 octobre au 6 décembre.
  const weeks = monthWeeks(day(2026, 11, 15), 1);
  assert.equal(weeks.length, 6);
  assert.equal(weeks[0], day(2026, 10, 26));
  assert.equal(weeks[5], day(2026, 11, 30));
  // Février 2027 commence un lundi et compte 28 jours : quatre semaines.
  assert.equal(monthWeeks(day(2027, 2, 1), 1).length, 4);
  // Semaine commençant le dimanche.
  assert.equal(monthWeeks(day(2026, 11, 1), 0)[0], day(2026, 11, 1));
});

// --- Lot F2 : règles de répétition complètes (relecture du 12 novembre, n° 1) ---------

import { ruleSupported } from "../../src/calendar.ts";

const at9 = (rrule, start = "20261102T090000", fields = {}) =>
  event({ start: { value: start, tzid: "Europe/Paris" }, end: { value: start.replace("T09", "T10"), tzid: "Europe/Paris" }, rrule, ...fields });
const dates = (list) => list.map((o) => { const d = new Date(o.start); return `${d.getMonth() + 1}/${d.getDate()}`; });

test("n° 1 : chaque jour ouvré (DAILY + BYDAY)", () => {
  const list = occurrences([at9("FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR")], day(2026, 11, 2), day(2026, 11, 16));
  assert.equal(list.length, 10);
  assert.ok(list.every((o) => ![0, 6].includes(new Date(o.start).getDay())));
});

test("n° 1 : dernier jour ouvré du mois (BYSETPOS=-1)", () => {
  const list = occurrences([at9("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1")], day(2026, 11, 1), day(2027, 2, 1));
  assert.deepEqual(dates(list), ["11/30", "12/31", "1/29"]);
});

test("n° 1 : 4e jeudi de novembre (YEARLY + BYMONTH + BYDAY)", () => {
  const list = occurrences([at9("FREQ=YEARLY;BYMONTH=11;BYDAY=4TH", "20261126T090000")], day(2026, 1, 1), day(2029, 1, 1));
  assert.deepEqual(list.map((o) => new Date(o.start).toDateString()), [
    new Date(2026, 10, 26).toDateString(),
    new Date(2027, 10, 25).toDateString(),
    new Date(2028, 10, 23).toDateString(),
  ]);
  // Sans BYMONTH, le rang compte dans l'année : 20e lundi.
  const twentieth = occurrences([at9("FREQ=YEARLY;BYDAY=20MO", "20260518T090000")], day(2026, 1, 1), day(2028, 1, 1));
  assert.deepEqual(dates(twentieth), ["5/18", "5/17"]);
  // BYMONTH seul : chaque mois cité, au jour du début.
  const twice = occurrences([at9("FREQ=YEARLY;BYMONTH=1,7", "20260115T090000")], day(2026, 1, 1), day(2027, 1, 1));
  assert.deepEqual(dates(twice), ["1/15", "7/15"]);
});

test("n° 1 : UNTIL en date seule inclut ce jour-là", () => {
  const list = occurrences([at9("FREQ=DAILY;UNTIL=20261106")], day(2026, 11, 2), day(2026, 11, 16));
  assert.deepEqual(dates(list), ["11/2", "11/3", "11/4", "11/5", "11/6"]);
});

test("n° 1 : EXDATE en date seule sur une série à heure fixe", () => {
  const list = occurrences([at9("FREQ=DAILY;COUNT=5", "20261102T090000", { exdates: ["20261104"] })], day(2026, 11, 2), day(2026, 11, 16));
  assert.deepEqual(dates(list), ["11/2", "11/3", "11/5", "11/6"]);
});

test("n° 1 : règle non prise en charge, première occurrence seule et signalée", () => {
  for (const rule of ["FREQ=HOURLY", "FREQ=WEEKLY;BYWEEKNO=20", "FREQ=DAILY;BYHOUR=9,17"]) {
    assert.equal(ruleSupported(rule), false, rule);
    assert.equal(occurrences([at9(rule)], day(2026, 11, 1), day(2026, 12, 1)).length, 1, rule);
  }
  assert.equal(ruleSupported("FREQ=MONTHLY;BYDAY=MO;BYSETPOS=1;WKST=SU;X-NAME=1"), true);
});

test("n° 1 : BYMONTHDAY restreint par BYDAY (vendredi 13), et WKST", () => {
  const friday13 = occurrences([at9("FREQ=MONTHLY;BYDAY=FR;BYMONTHDAY=13", "20261113T090000")], day(2026, 11, 1), day(2027, 12, 31));
  assert.deepEqual(dates(friday13), ["11/13", "8/13"]);
  // Exemple de la RFC 5545 : toutes les 2 semaines, mardi et dimanche ; seul WKST change.
  const rfc = (wkst) =>
    dates(occurrences([at9(`FREQ=WEEKLY;INTERVAL=2;COUNT=4;BYDAY=TU,SU;WKST=${wkst}`, "19970805T090000")], day(1997, 8, 1), day(1997, 9, 5)));
  assert.deepEqual(rfc("MO"), ["8/5", "8/10", "8/19", "8/24"]);
  assert.deepEqual(rfc("SU"), ["8/5", "8/17", "8/19", "8/31"]);
});

import { describeRecurrence, editable, moveRecurrence } from "../../src/recurrence.ts";

test("n° 36 : la règle suit la nouvelle date de début", () => {
  // « 2e mardi du mois » créé le mardi 10 novembre, date passée au mercredi 11.
  assert.equal(moveRecurrence("FREQ=MONTHLY;BYDAY=2TU", "2026-11-10", "2026-11-11", false), "FREQ=MONTHLY;BYDAY=2WE");
  // Dernier vendredi déplacé à un vendredi qui n'est pas le dernier : rang.
  assert.equal(moveRecurrence("FREQ=MONTHLY;BYDAY=-1FR", "2026-11-27", "2026-11-13", false), "FREQ=MONTHLY;BYDAY=2FR");
  // Hebdomadaire lundi et mercredi décalé d'un jour : mardi et jeudi.
  assert.equal(moveRecurrence("FREQ=WEEKLY;BYDAY=MO,WE", "2026-11-02", "2026-11-03", false), "FREQ=WEEKLY;BYDAY=TU,TH");
  // Jour du début seul : la règle reste « chaque semaine ».
  assert.equal(moveRecurrence("FREQ=WEEKLY", "2026-11-02", "2026-11-04", false), "FREQ=WEEKLY");
  // Jours ouvrés : inchangés.
  assert.equal(moveRecurrence("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR", "2026-11-02", "2026-11-03", false), "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR");
  // Règle venue d'ailleurs, que l'éditeur ne sait pas relire : gardée telle quelle.
  const foreign = "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1";
  assert.equal(editable(foreign, "2026-11-30"), false);
  assert.equal(moveRecurrence(foreign, "2026-11-30", "2026-12-01", false), foreign);
});

test("n° 1 : description des règles venues d'autres logiciels", () => {
  assert.equal(describeRecurrence("FREQ=HOURLY;INTERVAL=2", "2026-11-02"), tr("Répétition non prise en charge"));
  assert.equal(describeRecurrence("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1", "2026-11-30"), tr("Personnalisée"));
  assert.equal(describeRecurrence("FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR", "2026-11-02"), describeRecurrence("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR", "2026-11-02"));
});
