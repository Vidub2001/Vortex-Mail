// Règles de répétition (RRULE) : lecture, écriture et description dans la langue de l'interface.
import { ruleSupported } from "./calendar";
import { formatDate } from "./general";
import { locale, t } from "./i18n";

export type Freq = "DAILY" | "WEEKLY" | "MONTHLY" | "YEARLY";
/** Codes iCalendar des jours, index 0 = dimanche (comme Date.getDay()). */
export const DAY_CODES = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];
/** Noms des jours dans la langue de l'interface (index 0 = dimanche) ; le 1er janvier 2023 est un dimanche. */
const dayNames = (weekday: "long" | "narrow") => {
  const f = new Intl.DateTimeFormat(locale(), { weekday, timeZone: "UTC" });
  return Array.from({ length: 7 }, (_, i) => f.format(new Date(Date.UTC(2023, 0, 1 + i))));
};
export const DAY_LONG = dayNames("long");
/** Initiale des jours (boutons du choix des jours). */
export const DAY_SHORT = dayNames("narrow");

export interface Recurrence {
  freq: Freq;
  interval: number;
  /** Semaine : jours (index 0 = dimanche). */
  days: number[];
  /** Mois : « le 5 » (monthday), « le 2e mardi » (nth), « le dernier mardi » (last). */
  monthly: "monthday" | "nth" | "last";
  /** Fin : jamais, à une date (AAAA-MM-JJ), après un nombre d'occurrences. */
  end: "never" | "until" | "count";
  until: string;
  count: number;
}

const pad = (n: number) => String(n).padStart(2, "0");
const dateOf = (iso: string) => new Date(`${iso}T00:00`);

/** Rang du jour dans le mois (1 à 5) et s'il est le dernier de ce jour. */
export function weekdayRank(d: Date): { nth: number; last: boolean } {
  const length = new Date(d.getFullYear(), d.getMonth() + 1, 0).getDate();
  return { nth: Math.ceil(d.getDate() / 7), last: d.getDate() + 7 > length };
}

/** Lecture d'une règle ; `startIso` (AAAA-MM-JJ) complète les valeurs absentes. */
export function parseRecurrence(rule: string, startIso: string): Recurrence {
  const start = dateOf(startIso);
  const r: Record<string, string> = Object.fromEntries(
    rule
      .split(";")
      .map((p) => p.split("="))
      .filter((p) => p.length === 2)
      .map(([k, v]) => [k.toUpperCase(), v.toUpperCase()]),
  );
  const freq = (["DAILY", "WEEKLY", "MONTHLY", "YEARLY"].includes(r.FREQ) ? r.FREQ : "WEEKLY") as Freq;
  const byday = r.BYDAY ? r.BYDAY.split(",") : [];
  let days = byday.map((b) => DAY_CODES.indexOf(b.slice(-2))).filter((i) => i >= 0);
  if (days.length === 0) days = [start.getDay()];
  let monthly: Recurrence["monthly"] = "monthday";
  if (freq === "MONTHLY" && byday.length) monthly = /^-1/.test(byday[0]) ? "last" : "nth";
  let until = "";
  if (r.UNTIL) {
    // UNTIL en UTC (fin de journée locale convertie) : relu en date locale,
    // sinon la date de fin avance d'un jour à chaque modification (F11).
    const utc = /^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z$/.exec(r.UNTIL);
    const m = /^(\d{4})(\d{2})(\d{2})/.exec(r.UNTIL);
    if (utc) {
      const d = new Date(Date.UTC(+utc[1], +utc[2] - 1, +utc[3], +utc[4], +utc[5], +utc[6]));
      until = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
    } else if (m) until = `${m[1]}-${m[2]}-${m[3]}`;
  }
  return {
    freq,
    interval: Math.max(1, +(r.INTERVAL ?? 1) || 1),
    days: [...new Set(days)].sort((a, b) => a - b),
    monthly,
    end: r.COUNT ? "count" : r.UNTIL ? "until" : "never",
    until: until || startIso,
    count: Math.max(1, +(r.COUNT ?? 10) || 10),
  };
}

/** Écriture de la règle. `allDay` : UNTIL en date seule, sinon en fin de journée (UTC). */
export function buildRecurrence(rec: Recurrence, startIso: string, allDay: boolean): string {
  const start = dateOf(startIso);
  const parts = [`FREQ=${rec.freq}`];
  if (rec.interval > 1) parts.push(`INTERVAL=${rec.interval}`);
  if (rec.freq === "WEEKLY") {
    const days = rec.days.length ? rec.days : [start.getDay()];
    // Le jour du début seul : inutile de le préciser (règle plus simple, mieux comprise partout).
    if (!(days.length === 1 && days[0] === start.getDay())) {
      // Ordre lundi → dimanche, comme le veut l'usage iCalendar.
      const ordered = [...days].sort((a, b) => ((a + 6) % 7) - ((b + 6) % 7));
      parts.push(`BYDAY=${ordered.map((d) => DAY_CODES[d]).join(",")}`);
    }
  }
  if (rec.freq === "MONTHLY" && rec.monthly !== "monthday") {
    const { nth } = weekdayRank(start);
    parts.push(`BYDAY=${rec.monthly === "last" ? -1 : nth}${DAY_CODES[start.getDay()]}`);
  }
  if (rec.end === "count") parts.push(`COUNT=${Math.max(1, rec.count)}`);
  if (rec.end === "until" && rec.until) {
    const [y, m, d] = rec.until.split("-").map(Number);
    if (allDay) parts.push(`UNTIL=${y}${pad(m)}${pad(d)}`);
    else {
      const t = new Date(y, m - 1, d, 23, 59, 59);
      parts.push(
        `UNTIL=${t.getUTCFullYear()}${pad(t.getUTCMonth() + 1)}${pad(t.getUTCDate())}T${pad(t.getUTCHours())}${pad(t.getUTCMinutes())}${pad(t.getUTCSeconds())}Z`,
      );
    }
  }
  return parts.join(";");
}

/** Parties comparées d'une règle : sans `UNTIL` (son écriture dépend de « toute la journée ») ni `INTERVAL=1`. */
const normalized = (rule: string) =>
  rule
    .toUpperCase()
    .split(";")
    .filter((p) => p && !p.startsWith("UNTIL=") && p !== "INTERVAL=1")
    .sort()
    .join(";");

/**
 * Règle que l'éditeur relit et réécrit sans rien perdre. Les autres (venues
 * d'un autre logiciel : BYSETPOS, BYMONTH…) restent telles quelles tant que
 * l'utilisateur ne choisit pas une autre répétition.
 */
export function editable(rule: string, startIso: string): boolean {
  return !rule || normalized(buildRecurrence(parseRecurrence(rule, startIso), startIso, false)) === normalized(rule);
}

/**
 * Règle après un changement de date de début (relecture du 12 novembre,
 * n° 36) : le jour de la semaine et le rang dans le mois suivent la nouvelle
 * date, sinon la règle garde l'ancien jour alors que la fiche annonce le
 * nouveau. Les jours d'une série hebdomadaire sont décalés d'autant, sauf
 * s'ils contiennent déjà le nouveau jour ou sont les jours ouvrés.
 */
export function moveRecurrence(rule: string, fromIso: string, toIso: string, allDay: boolean): string {
  if (!rule || fromIso === toIso || !editable(rule, fromIso)) return rule;
  const rec = parseRecurrence(rule, fromIso);
  const from = dateOf(fromIso);
  const to = dateOf(toIso);
  if (Number.isNaN(from.getTime()) || Number.isNaN(to.getTime())) return rule;
  if (rec.freq === "WEEKLY" && !rec.days.includes(to.getDay()) && rec.days.join() !== "1,2,3,4,5") {
    const delta = to.getDay() - from.getDay();
    rec.days = [...new Set(rec.days.map((d) => (d + delta + 7) % 7))].sort((a, b) => a - b);
  }
  if (rec.freq === "MONTHLY" && rec.monthly !== "monthday") {
    const { nth, last } = weekdayRank(to);
    if (rec.monthly === "last" && !last) rec.monthly = "nth";
    else if (rec.monthly === "nth" && nth > 4) rec.monthly = "last";
  }
  return buildRecurrence(rec, toIso, allDay);
}

const monthName = (d: Date) => new Intl.DateTimeFormat(locale(), { month: "long" }).format(d);
const listOf = (items: string[]) => new Intl.ListFormat(locale(), { style: "long", type: "conjunction" }).format(items);

/** Répétition hebdomadaire : « Chaque semaine, les lundi et jeudi », « Chaque jour de la semaine (lun. – ven.) ». */
function weeklyText(n: number, days: number[]): string {
  const sorted = [...days].sort((a, b) => ((a + 6) % 7) - ((b + 6) % 7));
  if (sorted.join() === "1,2,3,4,5")
    return n > 1 ? t("Toutes les {n} semaines, en semaine (lun. – ven.)", { n }) : t("Chaque jour de la semaine (lun. – ven.)");
  if (sorted.length === 7) return n > 1 ? t("Toutes les {n} semaines, tous les jours", { n }) : t("Chaque semaine, tous les jours");
  if (sorted.length === 1) {
    const day = DAY_LONG[sorted[0]];
    return n > 1 ? t("Toutes les {n} semaines, le {day}", { n, day }) : t("Chaque semaine, le {day}", { day });
  }
  const list = listOf(sorted.map((d) => DAY_LONG[d]));
  return n > 1 ? t("Toutes les {n} semaines, les {days}", { n, days: list }) : t("Chaque semaine, les {days}", { days: list });
}

/** Répétition mensuelle : « Chaque mois, le 5 », « Tous les 2 mois, le 2e mardi », « Chaque mois, le dernier vendredi ». */
function monthlyText(n: number, rec: Recurrence, start: Date): string {
  const day = DAY_LONG[start.getDay()];
  const d = start.getDate();
  if (rec.monthly === "monthday") {
    if (n > 1) return d === 1 ? t("Tous les {n} mois, le 1er", { n }) : t("Tous les {n} mois, le {d}", { n, d });
    return d === 1 ? t("Chaque mois, le 1er") : t("Chaque mois, le {d}", { d });
  }
  if (rec.monthly === "last") return n > 1 ? t("Tous les {n} mois, le dernier {day}", { n, day }) : t("Chaque mois, le dernier {day}", { day });
  const { nth } = weekdayRank(start);
  if (n > 1) {
    switch (nth) {
      case 1:
        return t("Tous les {n} mois, le 1er {day}", { n, day });
      case 2:
        return t("Tous les {n} mois, le 2e {day}", { n, day });
      case 3:
        return t("Tous les {n} mois, le 3e {day}", { n, day });
      case 4:
        return t("Tous les {n} mois, le 4e {day}", { n, day });
      default:
        return t("Tous les {n} mois, le 5e {day}", { n, day });
    }
  }
  switch (nth) {
    case 1:
      return t("Chaque mois, le 1er {day}", { day });
    case 2:
      return t("Chaque mois, le 2e {day}", { day });
    case 3:
      return t("Chaque mois, le 3e {day}", { day });
    case 4:
      return t("Chaque mois, le 4e {day}", { day });
    default:
      return t("Chaque mois, le 5e {day}", { day });
  }
}

/** Répétition annuelle : « Chaque année, le 1er janvier », « Tous les 2 ans, le 14 juillet ». */
function yearlyText(n: number, start: Date): string {
  const month = monthName(start);
  const d = start.getDate();
  if (n > 1) return d === 1 ? t("Tous les {n} ans, le 1er {month}", { n, month }) : t("Tous les {n} ans, le {d} {month}", { n, d, month });
  return d === 1 ? t("Chaque année, le 1er {month}", { month }) : t("Chaque année, le {d} {month}", { d, month });
}

/** Description d'une règle, ex. « Toutes les 2 semaines, les lundi et jeudi, 10 fois ». */
export function describeRecurrence(rule: string, startIso: string): string {
  if (!rule) return t("Ne se répète pas");
  if (!ruleSupported(rule)) return t("Répétition non prise en charge");
  // « Chaque jour ouvré » écrit en quotidien (usage de Thunderbird) : même description qu'en hebdomadaire.
  const core = normalized(rule)
    .split(";")
    .filter((p) => !p.startsWith("COUNT="))
    .join(";");
  if (core === "BYDAY=MO,TU,WE,TH,FR;FREQ=DAILY") {
    const kept = rule.split(";").filter((p) => /^(UNTIL|COUNT)=/i.test(p));
    return describeRecurrence(["FREQ=WEEKLY", "BYDAY=MO,TU,WE,TH,FR", ...kept].join(";"), startIso);
  }
  if (!editable(rule, startIso)) return t("Personnalisée");
  const rec = parseRecurrence(rule, startIso);
  const start = dateOf(startIso);
  const n = rec.interval;
  let text: string;
  switch (rec.freq) {
    case "DAILY":
      text = n > 1 ? t("Tous les {n} jours", { n }) : t("Chaque jour");
      break;
    case "WEEKLY":
      text = weeklyText(n, rec.days);
      break;
    case "MONTHLY":
      text = monthlyText(n, rec, start);
      break;
    default:
      text = yearlyText(n, start);
  }
  if (rec.end === "count")
    return rec.count > 1 ? t("{rule}, {n} fois", { rule: text, n: rec.count }) : t("{rule}, 1 fois", { rule: text });
  if (rec.end === "until") return t("{rule}, jusqu'au {date}", { rule: text, date: formatDate(dateOf(rec.until)) });
  return text;
}
