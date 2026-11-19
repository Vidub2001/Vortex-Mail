// Réglages de « Paramètres › Général » : formats d'affichage, disposition,
// format de rédaction, ordre des noms. Partagés par toutes les fenêtres
// (stockage local, même origine) ; un changement est appliqué partout.
import { useEffect, useState } from "react";
import { lang, locale, t, type Lang } from "./i18n";

export type TimeFormat = "24h" | "12h";
export type DateFormat = "dd/mm/yyyy" | "dd.mm.yyyy" | "yyyy-mm-dd" | "mm/dd/yyyy" | "d mmm yyyy";
export type Layout = "right" | "bottom" | "hidden";
export type ComposeFormat = "html" | "text";
export type NameOrder = "first-last" | "last-first";
/**
 * Affichage du corps des messages (lot 4b) : HTML d'origine, HTML nettoyé par
 * liste blanche, ou texte brut.
 */
export type BodyMode = "original" | "simple" | "text";
/** Premier jour de la semaine (0 = dimanche, 1 = lundi, 6 = samedi). */
export type WeekDay = 0 | 1 | 6;
/** Échelle de la grille de l'agenda, en minutes. */
export type TimeScale = 10 | 15 | 30 | 60;
/**
 * Première semaine de l'année : celle qui contient le 1er janvier, la première
 * semaine de 4 jours (ISO 8601, usage européen) ou la première semaine complète.
 */
export type FirstWeek = "iso" | "jan1" | "full";

export interface GeneralSettings {
  /** Langue de l'interface. */
  language: Lang;
  timeFormat: TimeFormat;
  dateFormat: DateFormat;
  /** Volet de lecture : à droite de la liste, en dessous, ou masqué. */
  layout: Layout;
  composeFormat: ComposeFormat;
  /** Contacts : « Prénom Nom » ou « Nom Prénom ». */
  nameOrder: NameOrder;
  // Agenda
  weekStart: WeekDay;
  timeScale: TimeScale;
  weekNumbers: boolean;
  firstWeek: FirstWeek;
  /** Rappel proposé pour un nouveau rendez-vous, en minutes avant ; -1 : aucun. */
  defaultReminder: number;
  /** Affichage du corps des messages. */
  bodyMode: BodyMode;
  /**
   * Fermer la fenêtre principale la cache dans la zone de notification : la
   * réception, les règles et les rappels continuent (lot B3).
   */
  keepInTray: boolean;
  /** Signaler les nouvelles versions publiées (lot K1) : GitHub interrogé une fois par jour. */
  checkUpdates: boolean;
}

const KEY = "vortex.general";
const CHANGED = "vortex-general";

const DEFAULTS: GeneralSettings = {
  language: lang(),
  timeFormat: "24h",
  dateFormat: "dd/mm/yyyy",
  layout: "right",
  composeFormat: "html",
  nameOrder: "first-last",
  weekStart: 1,
  timeScale: 30,
  weekNumbers: true,
  firstWeek: "iso",
  defaultReminder: 15,
  bodyMode: "original",
  keepInTray: true,
  checkUpdates: true,
};

let cached: GeneralSettings | null = null;

export function loadGeneral(): GeneralSettings {
  if (cached) return cached;
  try {
    cached = { ...DEFAULTS, ...(JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<GeneralSettings>) };
  } catch {
    cached = { ...DEFAULTS };
  }
  return cached;
}

export function saveGeneral(patch: Partial<GeneralSettings>) {
  cached = { ...loadGeneral(), ...patch };
  try {
    localStorage.setItem(KEY, JSON.stringify(cached));
  } catch {
    // stockage indisponible : réglage valable jusqu'à la fermeture
  }
  window.dispatchEvent(new Event(CHANGED));
}

/** Réglages à jour, y compris quand une autre fenêtre les modifie. */
export function useGeneral(): GeneralSettings {
  const [settings, setSettings] = useState(loadGeneral);
  useEffect(() => {
    const reload = () => {
      cached = null;
      setSettings(loadGeneral());
    };
    window.addEventListener(CHANGED, reload);
    window.addEventListener("storage", reload);
    return () => {
      window.removeEventListener(CHANGED, reload);
      window.removeEventListener("storage", reload);
    };
  }, []);
  return settings;
}

// ------------------------------------------------------------------ dates et heures

const pad = (n: number) => String(n).padStart(2, "0");
const monthShort = new Intl.DateTimeFormat(locale(), { month: "short" });

/** Heure selon le réglage : « 14:05 » ou « 2:05 PM ». */
export function formatTime(d: Date | number, format = loadGeneral().timeFormat): string {
  const t = typeof d === "number" ? new Date(d) : d;
  if (format === "24h") return `${pad(t.getHours())}:${pad(t.getMinutes())}`;
  const h = t.getHours() % 12 || 12;
  return `${h}:${pad(t.getMinutes())} ${t.getHours() < 12 ? "AM" : "PM"}`;
}

/** Heure ronde de la grille de l'agenda : « 08:00 » ou « 8 AM ». */
export function formatHour(h: number, format = loadGeneral().timeFormat): string {
  if (format === "24h") return `${pad(h)}:00`;
  return `${h % 12 || 12} ${h < 12 ? "AM" : "PM"}`;
}

/** Date courte selon le réglage. */
export function formatDate(d: Date | number, format = loadGeneral().dateFormat): string {
  const t = typeof d === "number" ? new Date(d) : d;
  const [dd, mm, yyyy] = [pad(t.getDate()), pad(t.getMonth() + 1), String(t.getFullYear())];
  switch (format) {
    case "dd.mm.yyyy":
      return `${dd}.${mm}.${yyyy}`;
    case "yyyy-mm-dd":
      return `${yyyy}-${mm}-${dd}`;
    case "mm/dd/yyyy":
      return `${mm}/${dd}/${yyyy}`;
    case "d mmm yyyy":
      return `${t.getDate()} ${monthShort.format(t)} ${yyyy}`;
    default:
      return `${dd}/${mm}/${yyyy}`;
  }
}

/** Choix proposés dans Paramètres, avec un exemple (31 décembre 2026, 14:05). */
export const TIME_CHOICES: { value: TimeFormat; label: string }[] = [
  { value: "24h", label: t("24 heures (14:05)") },
  { value: "12h", label: t("12 heures (2:05 PM)") },
];

const SAMPLE = new Date(2026, 11, 31, 14, 5);
export const DATE_CHOICES: { value: DateFormat; label: string }[] = (
  ["dd/mm/yyyy", "dd.mm.yyyy", "yyyy-mm-dd", "mm/dd/yyyy", "d mmm yyyy"] as DateFormat[]
).map((value) => ({ value, label: formatDate(SAMPLE, value) }));

// ------------------------------------------------------------------ noms

/** Nom affiché d'un contact selon l'ordre choisi (le nom complet s'il n'y a pas de prénom/nom). */
export function contactName(
  c: { name: string; givenName?: string; familyName?: string },
  order = loadGeneral().nameOrder,
): string {
  const given = (c.givenName ?? "").trim();
  const family = (c.familyName ?? "").trim();
  if (!given || !family) return c.name || [given, family].filter(Boolean).join(" ");
  return order === "last-first" ? `${family} ${given}` : `${given} ${family}`;
}

// ------------------------------------------------------------------ agenda

/** Rappels proposés (minutes avant le début). */
export const REMINDER_CHOICES: { value: number; label: string }[] = [
  { value: -1, label: t("Aucun") },
  { value: 0, label: t("Au début") },
  { value: 5, label: t("{n} minutes avant", { n: 5 }) },
  { value: 10, label: t("{n} minutes avant", { n: 10 }) },
  { value: 15, label: t("{n} minutes avant", { n: 15 }) },
  { value: 30, label: t("{n} minutes avant", { n: 30 }) },
  { value: 60, label: t("1 heure avant") },
  { value: 120, label: t("{n} heures avant", { n: 2 }) },
  { value: 720, label: t("{n} heures avant", { n: 12 }) },
  { value: 1440, label: t("1 jour avant") },
  { value: 2880, label: t("{n} jours avant", { n: 2 }) },
  { value: 10080, label: t("1 semaine avant") },
];

/** Libellé d'un rappel quelconque (valeur venue du serveur comprise). */
export function reminderLabel(minutes: number): string {
  const known = REMINDER_CHOICES.find((c) => c.value === minutes);
  if (known) return known.label;
  if (minutes % 1440 === 0) return t("{n} jours avant", { n: minutes / 1440 });
  if (minutes % 60 === 0) return t("{n} heures avant", { n: minutes / 60 });
  return t("{n} minutes avant", { n: minutes });
}
