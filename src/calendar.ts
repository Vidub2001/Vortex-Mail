// Agenda : conversion des dates iCalendar (fuseaux compris), calcul des
// répétitions et des occurrences à afficher sur une période.
import type { CalEvent, When } from "./types";
import { loadGeneral, type FirstWeek } from "./general";
import { t } from "./i18n";

/** Une occurrence affichée : instant de début et de fin (ms), et l'événement d'origine. */
export interface Occurrence {
  key: string;
  event: CalEvent;
  start: number;
  end: number;
  allDay: boolean;
  /** Valeur d'origine de cette occurrence (pour la supprimer seule). */
  occurrenceValue: string;
  /** Occurrence modifiée d'une série (`event.recurrenceId` rempli) : la série. */
  series?: CalEvent;
}

/** Noms de fuseaux au format Windows (envoyés par certains serveurs) → noms IANA. */
const WINDOWS_ZONES: Record<string, string> = {
  "Romance Standard Time": "Europe/Paris",
  "W. Europe Standard Time": "Europe/Berlin",
  "Central Europe Standard Time": "Europe/Budapest",
  "Central European Standard Time": "Europe/Warsaw",
  "GMT Standard Time": "Europe/London",
  "Greenwich Standard Time": "Atlantic/Reykjavik",
  "UTC": "UTC",
  "Eastern Standard Time": "America/New_York",
  "Pacific Standard Time": "America/Los_Angeles",
  "Central Standard Time": "America/Chicago",
  "Mountain Standard Time": "America/Denver",
};

const zoneCache = new Map<string, string | null>();

/** Fuseau utilisable par le navigateur, ou null (heure locale alors). */
function zone(tzid: string): string | null {
  if (!tzid) return null;
  if (zoneCache.has(tzid)) return zoneCache.get(tzid)!;
  // « /mozilla.org/20050126_1/Europe/Paris » → « Europe/Paris ».
  const candidates = [WINDOWS_ZONES[tzid], tzid, tzid.split("/").slice(-2).join("/")].filter(Boolean) as string[];
  let found: string | null = null;
  for (const c of candidates) {
    try {
      new Intl.DateTimeFormat("en-US", { timeZone: c });
      found = c;
      break;
    } catch {
      // nom inconnu : candidat suivant
    }
  }
  zoneCache.set(tzid, found);
  return found;
}

const formatters = new Map<string, Intl.DateTimeFormat>();

/** Décalage (ms) du fuseau à cet instant : heure murale − UTC. */
function offset(instant: number, tz: string): number {
  let f = formatters.get(tz);
  if (!f) {
    f = new Intl.DateTimeFormat("en-US", {
      timeZone: tz,
      hourCycle: "h23",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
    formatters.set(tz, f);
  }
  const parts = Object.fromEntries(f.formatToParts(new Date(instant)).map((p) => [p.type, p.value]));
  const asUtc = Date.UTC(+parts.year, +parts.month - 1, +parts.day, +parts.hour, +parts.minute, +parts.second);
  return asUtc - Math.floor(instant / 1000) * 1000;
}

/** Heure murale (champs) → instant, dans un fuseau donné, ou l'heure locale. */
function wallToInstant(y: number, mo: number, d: number, h: number, mi: number, s: number, tz: string | null): number {
  if (!tz) return new Date(y, mo, d, h, mi, s).getTime();
  const guess = Date.UTC(y, mo, d, h, mi, s);
  let t = guess - offset(guess, tz);
  t = guess - offset(t, tz); // seconde passe : changement d'heure
  return t;
}

interface Wall {
  y: number;
  mo: number;
  d: number;
  h: number;
  mi: number;
  s: number;
}

function parseWall(value: string): Wall | null {
  const m = /^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})?)?/.exec(value);
  if (!m) return null;
  return { y: +m[1], mo: +m[2] - 1, d: +m[3], h: +(m[4] ?? 0), mi: +(m[5] ?? 0), s: +(m[6] ?? 0) };
}

/** Instant (ms) d'une date iCalendar ; une date seule = minuit local. */
export function instant(w: When): number {
  const p = parseWall(w.value);
  if (!p) return Number.NaN;
  if (w.value.length === 8) return new Date(p.y, p.mo, p.d).getTime();
  if (w.value.endsWith("Z")) return Date.UTC(p.y, p.mo, p.d, p.h, p.mi, p.s);
  return wallToInstant(p.y, p.mo, p.d, p.h, p.mi, p.s, zone(w.tzid));
}

/** Durée iCalendar (`PT1H30M`, `P1D`, `P1W`) en ms. */
export function durationMs(d: string): number {
  const m = /^([+-])?P(?:(\d+)W)?(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$/.exec(d.trim());
  if (!m) return 0;
  const sign = m[1] === "-" ? -1 : 1;
  const [w, dd, h, mi, s] = [m[2], m[3], m[4], m[5], m[6]].map((x) => +(x ?? 0));
  return sign * ((((w * 7 + dd) * 24 + h) * 60 + mi) * 60 + s) * 1000;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** Instant → date-heure UTC iCalendar (`AAAAMMJJTHHMMSSZ`). */
export function toUtcStamp(t: number): string {
  const d = new Date(t);
  return `${d.getUTCFullYear()}${pad(d.getUTCMonth() + 1)}${pad(d.getUTCDate())}T${pad(d.getUTCHours())}${pad(d.getUTCMinutes())}${pad(d.getUTCSeconds())}Z`;
}

/** Instant → date locale iCalendar (`AAAAMMJJ`). */
export function toDateValue(t: number): string {
  const d = new Date(t);
  return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}`;
}

/** Fuseau du système (IANA, « Europe/Paris »). */
export function systemZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
}

/**
 * Fuseau d'enregistrement d'un rendez-vous (relecture F1) : celui d'origine
 * s'il est connu, sinon celui du système. Jamais l'UTC : une réunion à 9 h
 * doit rester à 9 h après un changement d'heure.
 */
export function eventZone(tzid: string): string {
  return tzid && zone(tzid) ? tzid : systemZone();
}

/** Instant → heure murale d'un fuseau (`AAAAMMJJTHHMMSS`). */
export function toWallValue(t: number, tzid: string): string {
  const tz = zone(tzid);
  const d = new Date(tz ? t + offset(t, tz) : t);
  const get = tz
    ? [d.getUTCFullYear(), d.getUTCMonth() + 1, d.getUTCDate(), d.getUTCHours(), d.getUTCMinutes(), d.getUTCSeconds()]
    : [d.getFullYear(), d.getMonth() + 1, d.getDate(), d.getHours(), d.getMinutes(), d.getSeconds()];
  return `${get[0]}${pad(get[1])}${pad(get[2])}T${pad(get[3])}${pad(get[4])}${pad(get[5])}`;
}

/** Valeur EXDATE désignant l'occurrence qui commence à `t`, dans le format de la série. */
export function exdateFor(e: CalEvent, t: number): string {
  if (e.allDay) return toDateValue(t);
  if (e.start.value.endsWith("Z")) return toUtcStamp(t);
  return toWallValue(t, e.start.tzid);
}

function endOf(e: CalEvent, start: number): number {
  if (e.end.value) return instant(e.end);
  if (e.duration) return start + durationMs(e.duration);
  return start + (e.allDay ? 86_400_000 : 3_600_000);
}

const DAYS = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];
/** Valeur BYDAY valide : jour, précédé ou non d'un rang (`MO`, `2TU`, `-1FR`). */
const BYDAY_VALUE = /^[+-]?([1-9]|[1-4]\d|5[0-3])?(SU|MO|TU|WE|TH|FR|SA)$/;
/** Valeurs BYDAY ou BYMONTHDAY gardées au plus (7 jours × 5 rangs, 62 jours du mois). */
const MAX_BY = 64;

function parseRule(rule: string): Record<string, string> {
  return Object.fromEntries(
    rule
      .split(";")
      .map((p) => p.split("="))
      .filter((p) => p.length === 2)
      .map(([k, v]) => [k.toUpperCase(), v.toUpperCase()]),
  );
}

/** Valeur BYDAY lue : rang (0 : tous) et jour (0 = dimanche). */
interface ByDay {
  n: number;
  wd: number;
}

/** Date civile (année, mois de 0 à 11, jour). */
type Ymd = [number, number, number];

const DAY_MS = 86_400_000;
const daysIn = (y: number, mo: number) => new Date(Date.UTC(y, mo + 1, 0)).getUTCDate();
const weekdayOf = (y: number, mo: number, d: number) => new Date(Date.UTC(y, mo, d)).getUTCDay();
const ymdOf = (ms: number): Ymd => {
  const d = new Date(ms);
  return [d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate()];
};

/** Jours d'un mois correspondant à BYDAY (`MO`, `2TU`, `-1FR`). */
function monthDaysFor(y: number, mo: number, byday: ByDay[]): number[] {
  const last = daysIn(y, mo);
  const out: number[] = [];
  for (const { n, wd } of byday) {
    const all: number[] = [];
    for (let d = 1; d <= last; d++) if (weekdayOf(y, mo, d) === wd) all.push(d);
    if (!n) out.push(...all);
    else {
      const pick = n > 0 ? all[n - 1] : all[all.length + n];
      if (pick) out.push(pick);
    }
  }
  return [...new Set(out)].sort((a, b) => a - b);
}

/** Jours d'une année correspondant à BYDAY, le rang comptant dans l'année (`20MO`, `-1FR`). */
function yearDaysFor(y: number, byday: ByDay[]): Ymd[] {
  const out = new Map<number, Ymd>();
  const jan1 = Date.UTC(y, 0, 1);
  const length = Date.UTC(y + 1, 0, 1) - jan1;
  for (const { n, wd } of byday) {
    const all: number[] = [];
    for (let ms = jan1 + ((wd - new Date(jan1).getUTCDay() + 7) % 7) * DAY_MS; ms < jan1 + length; ms += 7 * DAY_MS) all.push(ms);
    const picked = !n ? all : [n > 0 ? all[n - 1] : all[all.length + n]].filter((x) => x !== undefined);
    for (const ms of picked) out.set(ms, ymdOf(ms));
  }
  return [...out.entries()].sort(([a], [b]) => a - b).map(([, v]) => v);
}

/** Entiers non nuls d'une liste, dans [-max, max] (ou [1, max] sans négatifs), sans doublon et en nombre borné. */
function intList(value: string | undefined, max: number, negative = true): number[] {
  const all = (value ?? "")
    .split(",")
    .map(Number)
    .filter((n) => Number.isInteger(n) && n !== 0 && Math.abs(n) <= max && (negative || n > 0));
  return [...new Set(all)].slice(0, MAX_BY);
}

/** Parties d'une règle que l'agenda sait appliquer (RFC 5545 §3.3.10). */
const SUPPORTED_PARTS = new Set(["FREQ", "INTERVAL", "COUNT", "UNTIL", "BYDAY", "BYMONTHDAY", "BYMONTH", "BYSETPOS", "WKST"]);
const SUPPORTED_FREQ = new Set(["DAILY", "WEEKLY", "MONTHLY", "YEARLY"]);

/**
 * Règle que l'agenda sait développer. Les autres (répétition à l'heure,
 * BYWEEKNO, BYYEARDAY, BYHOUR…) ne montrent que la première occurrence et
 * sont annoncées « non prises en charge », plutôt que des dates fausses
 * (relecture du 12 novembre, n° 1). Les parties `X-` sont sans effet.
 */
export function ruleSupported(rule: string): boolean {
  const r = parseRule(rule);
  return SUPPORTED_FREQ.has(r.FREQ) && Object.keys(r).every((k) => SUPPORTED_PARTS.has(k) || k.startsWith("X-"));
}

/**
 * Fin d'une série : `UNTIL` en date seule couvre toute cette journée, même
 * pour une série à heure fixe (sinon la dernière occurrence manque).
 */
function untilOf(e: CalEvent, until: string): number {
  const p = parseWall(until);
  if (!p) return Infinity;
  if (until.length !== 8 || e.allDay) return instant({ value: until, tzid: e.start.tzid });
  const tz = e.start.value.endsWith("Z") ? "UTC" : zone(e.start.tzid);
  return wallToInstant(p.y, p.mo, p.d + 1, 0, 0, 0, tz) - 1;
}

/**
 * Débuts (heure murale) d'une série, dans l'ordre, jusqu'à `until` (ms) :
 * FREQ (DAILY, WEEKLY, MONTHLY, YEARLY), INTERVAL, COUNT, UNTIL, BYMONTH,
 * BYMONTHDAY, BYDAY, BYSETPOS et WKST, appliqués comme le prévoit la
 * RFC 5545 (§3.3.10 : selon la fréquence, une partie étend ou restreint).
 */
function* recurrences(e: CalEvent, until: number, from = -Infinity): Generator<number> {
  const first = parseWall(e.start.value);
  if (!first) return;
  const rule = parseRule(e.rrule);
  const freq = rule.FREQ;
  const interval = Math.max(1, +(rule.INTERVAL ?? 1) || 1);
  const count = rule.COUNT ? +rule.COUNT : Infinity;
  const tz = e.start.value.endsWith("Z") ? "UTC" : e.allDay ? null : zone(e.start.tzid);
  const toInstant = ([y, mo, d]: Ymd) =>
    e.allDay ? new Date(y, mo, d).getTime() : wallToInstant(y, mo, d, first.h, first.mi, first.s, tz);
  const limit = rule.UNTIL ? Math.min(until, untilOf(e, rule.UNTIL)) : until;
  const startInstant = instant(e.start);
  let emitted = 0;
  const emit = function* (t: number) {
    if (t < startInstant || emitted >= count || t > limit) return;
    emitted++;
    yield t;
  };
  if (!ruleSupported(e.rrule)) {
    yield* emit(startInstant);
    return;
  }
  // Valeurs valides seulement, sans doublon et en nombre borné : une règle
  // de 300 000 valeurs figerait l'affichage (relecture du modèle de menace).
  const byday: ByDay[] = [...new Set((rule.BYDAY ?? "").split(",").filter((b) => BYDAY_VALUE.test(b)))]
    .slice(0, MAX_BY)
    .map((b) => ({ n: Number.parseInt(b, 10) || 0, wd: DAYS.indexOf(b.slice(-2)) }));
  const weekdays = new Set(byday.map((b) => b.wd));
  const bymonthday = intList(rule.BYMONTHDAY, 31);
  const bymonth = intList(rule.BYMONTH, 12, false).map((m) => m - 1);
  const bysetpos = intList(rule.BYSETPOS, 366);
  const wkst = Math.max(0, DAYS.indexOf(rule.WKST ?? "MO"));

  const inMonth = (mo: number) => !bymonth.length || bymonth.includes(mo);
  const onMonthDay = ([y, mo, d]: Ymd) => {
    const last = daysIn(y, mo);
    return bymonthday.some((x) => (x > 0 ? x : last + x + 1) === d);
  };
  const onWeekday = ([y, mo, d]: Ymd) => weekdays.has(weekdayOf(y, mo, d));
  const monthDays = (y: number, mo: number): Ymd[] => {
    const last = daysIn(y, mo);
    return bymonthday.map((x) => (x > 0 ? x : last + x + 1)).filter((d) => d >= 1 && d <= last).sort((a, b) => a - b).map((d) => [y, mo, d]);
  };

  /** Jours d'une période (jour, semaine, mois ou année n° `step`) et premier jour de la période. */
  const period = (step: number): { days: Ymd[]; begins: Ymd } => {
    if (freq === "DAILY") {
      const day = ymdOf(Date.UTC(first.y, first.mo, first.d + step * interval));
      // BYMONTH, BYMONTHDAY et BYDAY restreignent (« chaque jour ouvré »).
      const ok = inMonth(day[1]) && (!bymonthday.length || onMonthDay(day)) && (!byday.length || onWeekday(day));
      return { days: ok ? [day] : [], begins: day };
    }
    if (freq === "WEEKLY") {
      // Semaine commençant au jour WKST (lundi par défaut) qui contient le premier jour.
      const startDay = Date.UTC(first.y, first.mo, first.d);
      const weekBegin = startDay - ((new Date(startDay).getUTCDay() - wkst + 7) % 7) * DAY_MS + step * interval * 7 * DAY_MS;
      const wanted = byday.length ? weekdays : new Set([new Date(startDay).getUTCDay()]);
      const days: Ymd[] = [];
      for (let i = 0; i < 7; i++) {
        const day = ymdOf(weekBegin + i * DAY_MS);
        if (wanted.has(weekdayOf(...day)) && inMonth(day[1])) days.push(day);
      }
      return { days, begins: ymdOf(weekBegin) };
    }
    if (freq === "MONTHLY") {
      const total = first.mo + step * interval;
      const y = first.y + Math.floor(total / 12);
      const mo = ((total % 12) + 12) % 12;
      let days: Ymd[] = [];
      if (inMonth(mo)) {
        if (bymonthday.length) days = monthDays(y, mo).filter((d) => !byday.length || onWeekday(d));
        else if (byday.length) days = monthDaysFor(y, mo, byday).map((d) => [y, mo, d]);
        else if (first.d <= daysIn(y, mo)) days = [[y, mo, first.d]];
      }
      return { days, begins: [y, mo, 1] };
    }
    // YEARLY
    const y = first.y + step * interval;
    let days: Ymd[];
    if (bymonthday.length) {
      const months = bymonth.length ? bymonth : Array.from({ length: 12 }, (_, i) => i);
      days = months.flatMap((mo) => monthDays(y, mo)).filter((d) => !byday.length || onWeekday(d));
    } else if (byday.length) {
      // Avec BYMONTH, le rang compte dans le mois ; sans, dans l'année.
      days = bymonth.length ? bymonth.flatMap((mo) => monthDaysFor(y, mo, byday).map((d): Ymd => [y, mo, d])) : yearDaysFor(y, byday);
    } else {
      const months = bymonth.length ? bymonth : [first.mo];
      days = months.filter((mo) => first.d <= daysIn(y, mo)).map((mo): Ymd => [y, mo, first.d]);
    }
    return { days, begins: [y, 0, 1] };
  };

  // Départ près de la période demandée, pour toutes les fréquences (audit P9) :
  // une série hebdomadaire de 2015 ne parcourt plus 600 semaines à chaque
  // calcul des rappels, et au-delà de 5 000 pas plus rien ne s'afficherait
  // (F13). Avec COUNT, il faut décompter les occurrences sautées : seule une
  // série quotidienne sans restriction (une occurrence par jour) le permet.
  let firstStep = 0;
  const plainDaily = freq === "DAILY" && !byday.length && !bymonthday.length && !bymonth.length && !bysetpos.length;
  if (Number.isFinite(from) && from > startInstant && (!rule.COUNT || plainDaily)) {
    const [fy, fmo] = ymdOf(from);
    const elapsed =
      freq === "DAILY"
        ? (from - startInstant) / DAY_MS
        : freq === "WEEKLY"
          ? (from - startInstant) / (7 * DAY_MS)
          : freq === "MONTHLY"
            ? (fy - first.y) * 12 + (fmo - first.mo)
            : fy - first.y;
    firstStep = Math.max(0, Math.floor(elapsed / interval) - 2);
    if (rule.COUNT) emitted = Math.min(firstStep, count);
  }
  for (let step = firstStep; step < firstStep + 5000 && emitted < count; step++) {
    const { days, begins } = period(step);
    if (toInstant(begins) > limit + DAY_MS) return;
    days.sort((a, b) => Date.UTC(...a) - Date.UTC(...b));
    // BYSETPOS : rang dans l'ensemble de la période (« dernier jour ouvré du mois »).
    const chosen = bysetpos.length
      ? [...new Set(bysetpos.map((p) => (p > 0 ? days[p - 1] : days[days.length + p])).filter((d) => d !== undefined))].sort(
          (a, b) => Date.UTC(...a) - Date.UTC(...b),
        )
      : days;
    const candidates = chosen.map(toInstant);
    if (candidates.length && candidates[0] > limit) return;
    for (const t of candidates) yield* emit(t);
  }
}

/** Occurrences visibles entre `from` et `to` (ms), séries développées et exceptions appliquées. */
export function occurrences(events: CalEvent[], from: number, to: number): Occurrence[] {
  const out: Occurrence[] = [];
  const seriesKey = (e: CalEvent) => `${e.id}|${e.uid}`;
  const masters = new Map(events.filter((e) => !e.recurrenceId && e.rrule).map((e) => [seriesKey(e), e]));
  const overrides = events.filter((e) => e.recurrenceId);
  /** Occurrence d'origine remplacée par une occurrence modifiée. */
  const replaced = (o: CalEvent, series: CalEvent) => instant({ value: o.recurrenceId, tzid: o.start.tzid || series.start.tzid });
  /**
   * Instants exclus, et jours exclus : un `EXDATE` en date seule sur une
   * série à heure fixe retire l'occurrence de ce jour-là (n° 1).
   */
  const excludedOf = (e: CalEvent) => {
    const instants = new Set(e.exdates.map((x) => instant({ value: x, tzid: e.start.tzid })));
    const days = new Set(e.allDay ? [] : e.exdates.filter((x) => x.length === 8));
    return { has: (t: number) => instants.has(t) || (days.size > 0 && days.has(exdateFor(e, t).slice(0, 8))) };
  };
  const visible = (start: number, end: number) => start < to && Math.max(end, start + 1) > from;

  for (const e of events) {
    if (e.recurrenceId) continue;
    const start = instant(e.start);
    if (Number.isNaN(start)) continue;
    const length = Math.max(0, endOf(e, start) - start);
    if (!e.rrule) {
      if (visible(start, start + length)) {
        out.push({ key: e.id + e.uid, event: e, start, end: start + length, allDay: e.allDay, occurrenceValue: e.start.value });
      }
      continue;
    }
    const excluded = excludedOf(e);
    const moved = new Set(overrides.filter((o) => seriesKey(o) === seriesKey(e)).map((o) => replaced(o, e)));
    for (const t of recurrences(e, to, from - length)) {
      if (excluded.has(t) || moved.has(t)) continue;
      // Fin de période exclue : une occurrence commençant pile à `to` appartient à la suivante.
      if (visible(t, t + length)) {
        out.push({ key: `${e.id}${t}`, event: e, start: t, end: t + length, allDay: e.allDay, occurrenceValue: exdateFor(e, t) });
      }
    }
  }

  // Occurrences modifiées, à leur propre date : même avancées dans la période,
  // ou seules sans leur série (invitation à une seule occurrence) — relecture F7.
  for (const o of overrides) {
    const os = instant(o.start);
    if (Number.isNaN(os)) continue;
    const oe = endOf(o, os);
    if (!visible(os, oe)) continue;
    const series = masters.get(seriesKey(o));
    if (series) {
      const t = replaced(o, series);
      if (excludedOf(series).has(t)) continue;
      out.push({
        key: `${series.id}${t}`,
        event: { ...o, id: series.id, calendar: series.calendar },
        series,
        start: os,
        end: oe,
        allDay: o.allDay,
        occurrenceValue: exdateFor(series, t),
      });
    } else {
      out.push({ key: `${o.id}${o.uid}${o.recurrenceId}`, event: o, start: os, end: oe, allDay: o.allDay, occurrenceValue: o.recurrenceId });
    }
  }
  return out.sort((a, b) => a.start - b.start);
}

/** Début (0 h local) de la semaine contenant `t` ; premier jour selon le réglage (lundi par défaut). */
export function weekStart(t: number, firstDay: number = loadGeneral().weekStart): number {
  const d = new Date(t);
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() - ((d.getDay() - firstDay + 7) % 7));
  return d.getTime();
}

/** Début de la semaine 1 de l'année `y`. */
function firstWeekOf(y: number, firstDay: number, rule: FirstWeek): number {
  const jan1 = new Date(y, 0, 1).getTime();
  if (rule === "jan1") return weekStart(jan1, firstDay);
  if (rule === "full") {
    const s = weekStart(jan1, firstDay);
    return s === jan1 ? s : addDays(s, 7);
  }
  // Première semaine ayant au moins 4 jours dans l'année (ISO 8601 avec le lundi).
  return weekStart(new Date(y, 0, 4).getTime(), firstDay);
}

/** Numéro de la semaine contenant `t`, selon le premier jour et la règle de première semaine. */
export function weekNumber(
  t: number,
  firstDay: number = loadGeneral().weekStart,
  rule: FirstWeek = loadGeneral().firstWeek,
): number {
  const start = weekStart(t, firstDay);
  const y = new Date(t).getFullYear();
  for (const year of [y + 1, y, y - 1]) {
    const w1 = firstWeekOf(year, firstDay, rule);
    if (start >= w1) return Math.round((start - w1) / (7 * 86_400_000)) + 1;
  }
  return 1;
}

export function addDays(t: number, days: number): number {
  const d = new Date(t);
  d.setDate(d.getDate() + days);
  return d.getTime();
}

/** Minuit local du jour de `t`. */
function midnight(t: number): number {
  return new Date(t).setHours(0, 0, 0, 0);
}

/**
 * Nombre de jours de calendrier de `a` à `b` (dates locales). Les jours de
 * 23 ou 25 h du changement d'heure comptent pour un jour (relecture F10).
 */
export function dayDiff(a: number, b: number): number {
  return Math.round((midnight(b) - midnight(a)) / 86_400_000);
}

/**
 * Position de l'instant `t` dans la journée commençant à `day` (minuit
 * local), en minutes d'horloge, de 0 à 1440. D'après l'heure affichée et non
 * la durée écoulée depuis minuit : un rendez-vous de 10 h reste sur la ligne
 * de 10 h les jours de changement d'heure (relecture F15).
 */
export function minuteOfDay(t: number, day: number): number {
  if (t <= day) return 0;
  if (t >= addDays(day, 1)) return 24 * 60;
  const d = new Date(t);
  return d.getHours() * 60 + d.getMinutes() + d.getSeconds() / 60;
}

/** Instant du jour `day` à `minutes` d'horloge après minuit (1440 : minuit suivant). */
export function atMinute(day: number, minutes: number): number {
  const d = new Date(day);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate(), 0, minutes).getTime();
}

/** Débuts des semaines qui couvrent le mois de `month` (un instant de ce mois), de 4 à 6. */
export function monthWeeks(month: number, firstDay: number = loadGeneral().weekStart): number[] {
  const d = new Date(month);
  const first = new Date(d.getFullYear(), d.getMonth(), 1).getTime();
  const next = new Date(d.getFullYear(), d.getMonth() + 1, 1).getTime();
  const weeks: number[] = [];
  for (let w = weekStart(first, firstDay); w < next; w = addDays(w, 7)) weeks.push(w);
  return weeks;
}

/** Description lisible d'une règle simple (sinon « Personnalisée »). */
export function describeRule(rule: string): string {
  if (!rule) return t("Aucune");
  const r = parseRule(rule);
  const n = +(r.INTERVAL ?? 1);
  if (Object.keys(r).length === 1 || (Object.keys(r).length === 2 && r.INTERVAL)) {
    switch (r.FREQ) {
      case "DAILY":
        return n > 1 ? t("Tous les {n} jours", { n }) : t("Chaque jour");
      case "WEEKLY":
        return n > 1 ? t("Toutes les {n} semaines", { n }) : t("Chaque semaine");
      case "MONTHLY":
        return n > 1 ? t("Tous les {n} mois", { n }) : t("Chaque mois");
      case "YEARLY":
        return n > 1 ? t("Tous les {n} ans", { n }) : t("Chaque année");
    }
  }
  return t("Personnalisée");
}
