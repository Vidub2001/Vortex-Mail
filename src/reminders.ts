// Rappels de rendez-vous : vérifiés toutes les 30 s dans la fenêtre principale,
// à partir des agendas en cache. Chaque rappel n'est montré qu'une fois, même
// après un redémarrage ; un rappel manqué (application fermée) est montré au
// démarrage tant que le rendez-vous n'est pas commencé depuis plus de 15 min.
// Au-delà, les rappels manqués des dernières 24 h sont résumés au démarrage
// (lot B2) : l'application note toutes les 30 s qu'elle tourne encore.
import { useEffect, useRef } from "react";
import { api } from "./api";
import { occurrences, type Occurrence } from "./calendar";
import { formatDate, formatTime } from "./general";
import { t } from "./i18n";
import type { CalEvent } from "./types";

const FIRED_KEY = "vortex.reminders.fired";
/** Dernier moment où l'application tournait (rappels manqués, lot B2). */
const ALIVE_KEY = "vortex.reminders.alive";
const CHECK_MS = 30_000;
const RELOAD_MS = 5 * 60_000;
const LATE_MS = 15 * 60_000;
const DAY_MS = 86_400_000;

function loadFired(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem(FIRED_KEY) ?? "{}") as Record<string, number>;
  } catch {
    return {};
  }
}

export function saveFired(fired: Record<string, number>) {
  // On garde les rappels montrés depuis moins de 8 jours, et ceux dont le
  // rendez-vous n'est pas encore passé : un rappel deux semaines avant ne
  // doit pas revenir une semaine plus tard (clé : `uid|début|minutes`).
  const limit = Date.now() - 8 * DAY_MS;
  const kept = Object.fromEntries(Object.entries(fired).filter(([key, t]) => t > limit || Number(key.split("|").slice(-2)[0]) > limit));
  try {
    localStorage.setItem(FIRED_KEY, JSON.stringify(kept));
  } catch {
    // stockage indisponible : rappels gardés en mémoire pour cette session
  }
  return kept;
}

function loadAlive(): number | null {
  try {
    const at = Number(localStorage.getItem(ALIVE_KEY));
    return Number.isFinite(at) && at > 0 ? at : null;
  } catch {
    return null;
  }
}

function saveAlive(now: number) {
  try {
    localStorage.setItem(ALIVE_KEY, String(now));
  } catch {
    // stockage indisponible : pas de résumé des rappels manqués au prochain démarrage
  }
}

/** Quand a lieu le rendez-vous, en clair : « 14:30 (dans 15 min) », « demain 09:00 »… */
export function whenText(o: Occurrence, now = Date.now()): string {
  const sameDay = new Date(o.start).toDateString() === new Date(now).toDateString();
  const tomorrow = new Date(o.start).toDateString() === new Date(now + DAY_MS).toDateString();
  if (o.allDay) {
    if (sameDay) return t("aujourd'hui, toute la journée");
    if (tomorrow) return t("demain, toute la journée");
    return t("le {date}, toute la journée", { date: formatDate(o.start) });
  }
  const time = formatTime(o.start);
  const when = sameDay
    ? t("aujourd'hui à {time}", { time })
    : tomorrow
      ? t("demain à {time}", { time })
      : t("le {date} à {time}", { date: formatDate(o.start), time });
  const minutes = Math.round((o.start - now) / 60_000);
  if (minutes > 0 && minutes < 120) return t("{when} (dans {n} min)", { when, n: minutes });
  if (minutes <= 0 && minutes > -60) return t("{when} (commencé)", { when });
  return when;
}

/** Rappels dus maintenant (et pas encore montrés). */
export function due(events: CalEvent[], fired: Record<string, number>, now: number): { o: Occurrence; key: string }[] {
  const withReminder = events.filter((e) => e.reminder != null && e.reminder >= 0);
  if (withReminder.length === 0) return [];
  // Les séries et exceptions doivent rester ensemble : on garde tous les événements des UID concernés.
  const uids = new Set(withReminder.map((e) => e.uid));
  const pool = events.filter((e) => uids.has(e.uid));
  // Rendez-vous examinés jusqu'au plus long rappel présent (un rappel lu
  // dans une invitation peut précéder de deux semaines), au moins 8 jours.
  const longest = Math.max(...withReminder.map((e) => e.reminder ?? 0)) * 60_000;
  const list = occurrences(pool, now - LATE_MS, now + Math.max(8 * DAY_MS, longest + DAY_MS));
  const out: { o: Occurrence; key: string }[] = [];
  for (const o of list) {
    const minutes = o.event.reminder;
    if (minutes == null || minutes < 0) continue;
    const at = o.start - minutes * 60_000;
    const key = `${o.event.uid}|${o.start}|${minutes}`;
    const late = o.allDay ? now >= o.end : now > o.start + LATE_MS;
    if (at <= now && !late && !fired[key]) out.push({ o, key });
  }
  return out;
}

/**
 * Rappels manqués pendant que l'application était fermée (depuis `since`, au
 * plus 24 h), devenus trop tardifs pour `due` : rendez-vous commencé depuis
 * plus de 15 min, ou journée entière terminée. Les plus anciens d'abord.
 */
export function missed(events: CalEvent[], fired: Record<string, number>, since: number, now: number): { o: Occurrence; key: string }[] {
  const from = Math.max(since, now - DAY_MS);
  const withReminder = events.filter((e) => e.reminder != null && e.reminder >= 0);
  if (withReminder.length === 0 || from >= now) return [];
  const uids = new Set(withReminder.map((e) => e.uid));
  const pool = events.filter((e) => uids.has(e.uid));
  const out: { o: Occurrence; key: string }[] = [];
  for (const o of occurrences(pool, from, now)) {
    const minutes = o.event.reminder;
    if (minutes == null || minutes < 0) continue;
    const at = o.start - minutes * 60_000;
    const key = `${o.event.uid}|${o.start}|${minutes}`;
    const late = o.allDay ? now >= o.end : now > o.start + LATE_MS;
    if (at > from && at <= now && late && !fired[key]) out.push({ o, key });
  }
  return out.sort((a, b) => a.o.start - b.o.start);
}

/**
 * Surveille les rappels. `version` change après une synchronisation des agendas ;
 * `notify` affiche aussi le rappel dans l'application.
 */
export function useReminders(version: number, notify: (title: string, body?: string, intent?: "success" | "error" | "info") => void) {
  const events = useRef<CalEvent[]>([]);
  const fired = useRef<Record<string, number>>(loadFired());
  const notifyRef = useRef(notify);
  notifyRef.current = notify;
  /** Dernier signe de vie de la session précédente, lu une seule fois. */
  const lastAlive = useRef<number | null>(loadAlive());

  useEffect(() => {
    let alive = true;
    const reload = () =>
      api
        .events()
        .then((list) => {
          if (alive) events.current = list;
        })
        .catch(() => {
          // agenda illisible : nouvel essai au prochain rechargement
        });
    reload().then(check);
    const timer = window.setInterval(reload, RELOAD_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [version]);

  function check() {
    const now = Date.now();
    saveAlive(now);
    reportMissed(now);
    const list = due(events.current, fired.current, now);
    if (list.length === 0) return;
    for (const { o, key } of list) {
      fired.current[key] = now;
      const title = o.event.summary || t("(sans titre)");
      const body = [whenText(o, now), o.event.location].filter(Boolean).join("\n");
      api.showReminder(t("Rappel : {title}", { title }), body).catch(() => {
        // notification du bureau indisponible : le message dans l'application suffit
      });
      notifyRef.current(t("Rappel : {title}", { title }), body.replace("\n", " · "), "info");
    }
    fired.current = saveFired(fired.current);
  }

  /** Une seule fois, au premier passage avec les agendas chargés. */
  function reportMissed(now: number) {
    const since = lastAlive.current;
    if (since === null || events.current.length === 0) return;
    lastAlive.current = null;
    const list = missed(events.current, fired.current, since, now);
    if (list.length === 0) return;
    for (const { key } of list) fired.current[key] = now;
    fired.current = saveFired(fired.current);
    const line = (o: Occurrence) => `${o.event.summary || t("(sans titre)")} · ${whenText(o, now)}`;
    const title =
      list.length === 1
        ? t("Rappel manqué : {title}", { title: list[0].o.event.summary || t("(sans titre)") })
        : t("{n} rappels manqués pendant la fermeture", { n: list.length });
    const shown = list.slice(0, 5).map(({ o }) => line(o));
    if (list.length > 5) shown.push(t("et {n} autres", { n: list.length - 5 }));
    const body = list.length === 1 ? whenText(list[0].o, now) : shown.join("\n");
    api.showReminder(title, body).catch(() => {
      // notification du bureau indisponible : le message dans l'application suffit
    });
    notifyRef.current(title, body.replaceAll("\n", " · "), "info");
  }

  useEffect(() => {
    const timer = window.setInterval(check, CHECK_MS);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}
