// Agenda : vues semaine, mois et année ; à gauche, les agendas
// affichés (cases à cocher) et leur couleur.
import { formatHour, formatTime, useGeneral } from "../general";
import { Button, Checkbox, Popover, PopoverSurface, PopoverTrigger } from "@fluentui/react-components";
import { Add20Regular, ArrowClockwise20Regular, ChevronLeft20Regular, ChevronRight20Regular } from "@fluentui/react-icons";
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import { addDays, atMinute, minuteOfDay, occurrences, weekNumber, weekStart, type Occurrence } from "../calendar";
import type { Account, CalEvent, PimCollection } from "../types";
import { EventDialog, type EventDraft } from "./EventDialog";
import { EventSheet } from "./EventSheet";
import { PimTransfer } from "./PimTransfer";
import { allDeclined } from "../invites";
import { MiniMonths } from "./MiniMonths";
import { colorName } from "../colors";
import { locale, t } from "../i18n";
import { VortexSpinner } from "./VortexSpinner";

interface Props {
  accounts: Account[];
  notify: (title: string, body?: string, intent?: "success" | "error" | "info") => void;
  version: number;
  syncing: boolean;
  onSync: () => void;
}

type Mode = "week" | "month" | "year";

/** Hauteur d'une heure selon l'échelle : chaque case reste assez haute pour être visée. */
const hourHeight = (scale: number) => (scale >= 30 ? 48 : scale === 15 ? 80 : 108);
const DAY_MS = 86_400_000;
const PALETTE = [
  "#1e88e5", "#0f6cbd", "#00838f", "#2e7d32", "#7cb342", "#f9a825",
  "#ef6c00", "#c62828", "#ad1457", "#6a1b9a", "#4e342e", "#6b7280",
];

const dayName = new Intl.DateTimeFormat(locale(), { weekday: "long" });
const monthName = new Intl.DateTimeFormat(locale(), { month: "long" });
const monthYear = new Intl.DateTimeFormat(locale(), { month: "long", year: "numeric" });
/** Période de la vue semaine : « 3 – 9 novembre 2026 », « November 3 – 9, 2026 ». */
const dayRange = new Intl.DateTimeFormat(locale(), { day: "numeric", month: "long", year: "numeric" });
const hourFmt = { format: (d: Date | number) => formatTime(d) };
/** Noms courts des jours (index 0 = dimanche) ; le 1er janvier 2023 est un dimanche. */
const WEEKDAY_NAMES = (() => {
  const f = new Intl.DateTimeFormat(locale(), { weekday: "short", timeZone: "UTC" });
  return Array.from({ length: 7 }, (_, i) => f.format(new Date(Date.UTC(2023, 0, 1 + i))));
})();
/** Noms des jours dans l'ordre de la semaine choisie. */
const weekdays = (first: number) => Array.from({ length: 7 }, (_, i) => WEEKDAY_NAMES[(first + i) % 7]);

// ------------------------------------------------------------------ réglages d'affichage

const PREFS_KEY = "vortex.calendars";

interface Prefs {
  hidden: string[];
  colors: Record<string, string>;
  mode: Mode;
}

function loadPrefs(): Prefs {
  try {
    return { hidden: [], colors: {}, mode: "week", ...(JSON.parse(localStorage.getItem(PREFS_KEY) ?? "{}") as Partial<Prefs>) };
  } catch {
    return { hidden: [], colors: {}, mode: "week" };
  }
}

function savePrefs(p: Prefs) {
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(p));
  } catch {
    // stockage indisponible : réglage valable pour cette session
  }
}

// ------------------------------------------------------------------ outils

const sameDay = (a: number, b: number) => new Date(a).toDateString() === new Date(b).toDateString();
const startOfDay = (t: number) => new Date(t).setHours(0, 0, 0, 0);
const firstOfMonth = (t: number) => {
  const d = new Date(t);
  return new Date(d.getFullYear(), d.getMonth(), 1).getTime();
};
const addMonths = (t: number, n: number) => {
  const d = new Date(t);
  return new Date(d.getFullYear(), d.getMonth() + n, 1).getTime();
};
/** Rang du jour de `t` à partir du jour `from` (minuit local), changements d'heure compris. */
const dayIndex = (from: number, t: number) => Math.round((startOfDay(t) - from) / DAY_MS);
const isAllDay = (o: Occurrence) => o.allDay || o.end - o.start >= DAY_MS;

/** Colonnes côte à côte pour les rendez-vous qui se chevauchent. */
function layout(items: Occurrence[]): { o: Occurrence; col: number; cols: number }[] {
  const out: { o: Occurrence; col: number; cols: number }[] = [];
  let cluster: { o: Occurrence; col: number; cols: number }[] = [];
  let clusterEnd = -Infinity;
  const flush = () => {
    const cols = Math.max(1, ...cluster.map((c) => c.col + 1));
    cluster.forEach((c) => (c.cols = cols));
    out.push(...cluster);
    cluster = [];
  };
  for (const o of items) {
    if (o.start >= clusterEnd && cluster.length) flush();
    const used = new Set(cluster.filter((c) => c.o.end > o.start).map((c) => c.col));
    let col = 0;
    while (used.has(col)) col++;
    cluster.push({ o, col, cols: 1 });
    clusterEnd = Math.max(clusterEnd, o.end);
  }
  if (cluster.length) flush();
  return out;
}

// ------------------------------------------------------------------ vue

export function CalendarView({ accounts, notify, version, syncing, onSync }: Props) {
  // Formats, premier jour, échelle, numéros de semaine (Paramètres › Agenda).
  const general = useGeneral();
  const firstDay = general.weekStart;
  const scale = general.timeScale;
  const HOUR_PX = hourHeight(scale);
  const WEEKDAYS = weekdays(firstDay);
  const weekNo = (t: number) => weekNumber(t, firstDay, general.firstWeek);
  const [prefs, setPrefs] = useState<Prefs>(loadPrefs);
  const [cursor, setCursor] = useState(() => startOfDay(Date.now()));
  const [events, setEvents] = useState<CalEvent[] | null>(null);
  const [calendars, setCalendars] = useState<PimCollection[]>([]);
  const [dialog, setDialog] = useState<EventDraft | null>(null);
  /** Fiche passée à l'éditeur (« Modifier ») ; chaque rendez-vous ouvert repart de sa fiche. */
  const [editing, setEditing] = useState(false);
  useEffect(() => setEditing(false), [dialog]);
  const [now, setNow] = useState(Date.now());
  const scroller = useRef<HTMLDivElement>(null);
  const mode = prefs.mode;
  // Mini-mois de gauche : décalage par rapport au mois en cours.
  const [miniOffset, setMiniOffset] = useState(0);
  // Créneau en cours de sélection à la souris (vue semaine), en minutes depuis minuit.
  const [drag, setDrag] = useState<{ day: number; anchor: number; from: number; to: number } | null>(null);

  const updatePrefs = (patch: Partial<Prefs>) =>
    setPrefs((p) => {
      const next = { ...p, ...patch };
      savePrefs(next);
      return next;
    });

  const reload = async () => {
    const [list, collections] = await Promise.all([api.events(), api.pimCollections()]);
    // Invitations refusées : gardées sur le serveur (le supprimer enverrait un
    // second refus), mais plus affichées, comme dans Thunderbird (lot H1b).
    const emails = accounts.map((a) => a.email);
    setEvents(list.filter((e) => !allDeclined(e, emails)));
    setCalendars(collections.filter((c) => c.kind === "calendar"));
  };

  useEffect(() => {
    reload().catch((e) => notify(t("Agenda illisible"), String(e), "error"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [version]);

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  // Vue semaine : début de journée visible (7 h).
  useEffect(() => {
    if (mode === "week" && scroller.current) scroller.current.scrollTop = 7 * HOUR_PX;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, events === null, HOUR_PX]);

  const accountName = (id: string) => (id === "local" ? t("Sur cet ordinateur") : (accounts.find((a) => a.id === id)?.name ?? id));
  /**
   * Couleur par défaut de chaque agenda : celle du serveur si aucun autre agenda
   * ne l'a déjà, sinon la première couleur libre de la palette (deux comptes
   * Mailo renvoient la même). Les couleurs choisies à la main restent prioritaires.
   */
  const defaults = useMemo(() => {
    const rank = (id: string) => (id === "local" ? Number.MAX_SAFE_INTEGER : accounts.findIndex((a) => a.id === id));
    const ordered = calendars
      .map((c, i) => ({ c, i }))
      .sort((x, y) => rank(x.c.account) - rank(y.c.account) || x.i - y.i)
      .map((x) => x.c);
    const norm = (color: string) => color.trim().toLowerCase();
    const used = new Set(Object.values(prefs.colors).map(norm));
    const out: Record<string, string> = {};
    for (const c of ordered) {
      if (prefs.colors[c.url]) continue;
      let color = c.color && /^#[0-9a-f]{6}$/i.test(c.color) ? c.color : "";
      if (!color || used.has(norm(color))) {
        color =
          PALETTE.find((p) => !used.has(norm(p)) && p !== "#6b7280") ??
          PALETTE[Object.keys(out).length % PALETTE.length];
      }
      used.add(norm(color));
      out[c.url] = color;
    }
    return out;
  }, [calendars, accounts, prefs.colors]);

  const colorFor = (url: string, account: string) =>
    prefs.colors[url] || defaults[url] || accounts.find((a) => a.id === account)?.color || "#1e88e5";
  const colorOf = (o: Occurrence) => colorFor(o.event.calendar, o.event.source);

  // Période affichée.
  const [from, to] = useMemo(() => {
    if (mode === "week") return [weekStart(cursor, firstDay), addDays(weekStart(cursor, firstDay), 7)];
    if (mode === "month") {
      const start = weekStart(firstOfMonth(cursor), firstDay);
      return [start, addDays(start, 42)];
    }
    const y = new Date(cursor).getFullYear();
    return [new Date(y, 0, 1).getTime(), new Date(y + 1, 0, 1).getTime()];
  }, [mode, cursor, firstDay]);

  const hidden = useMemo(() => new Set(prefs.hidden), [prefs.hidden]);
  const visible = useMemo(
    () => occurrences((events ?? []).filter((e) => !hidden.has(e.calendar)), from, to),
    [events, hidden, from, to],
  );

  // Échap : abandonne le créneau en cours de sélection.
  useEffect(() => {
    if (!drag) return;
    const cancel = (e: KeyboardEvent) => e.key === "Escape" && setDrag(null);
    window.addEventListener("keydown", cancel);
    return () => window.removeEventListener("keydown", cancel);
  }, [drag]);

  // Mini-mois : le mois en cours (décalable) et les deux suivants, avec leurs jours occupés.
  const miniFirst = addMonths(firstOfMonth(now), miniOffset);
  const miniMonths = [0, 1, 2].map((i) => addMonths(miniFirst, i));
  const busyDays = useMemo(() => {
    const set = new Set<string>();
    const start = weekStart(miniFirst, firstDay);
    const end = addDays(weekStart(addMonths(miniFirst, 3), firstDay), 7);
    for (const o of occurrences((events ?? []).filter((e) => !hidden.has(e.calendar)), start, end)) {
      for (let d = startOfDay(o.start); d < o.end; d = addDays(d, 1)) set.add(new Date(d).toDateString());
    }
    return set;
  }, [events, hidden, miniFirst, firstDay]);

  const move = (n: number) => {
    if (mode === "week") setCursor(addDays(cursor, 7 * n));
    else if (mode === "month") setCursor(addMonths(cursor, n));
    else setCursor(new Date(new Date(cursor).getFullYear() + n, 0, 1).getTime());
  };

  const title = (() => {
    if (mode === "month") return monthYear.format(cursor);
    if (mode === "year") return String(new Date(cursor).getFullYear());
    const range = dayRange.formatRange(new Date(from), new Date(addDays(from, 6)));
    return general.weekNumbers ? t("{range} · semaine {n}", { range, n: weekNo(from) }) : range;
  })();

  const visibleCalendars = calendars.filter((c) => !hidden.has(c.url));
  const defaultCalendar =
    visibleCalendars.find((c) => c.account !== "local")?.url ?? visibleCalendars[0]?.url ?? calendars[0]?.url ?? "local:calendar";

  /**
   * Occurrence d'une série : la série se modifie depuis cette occurrence ;
   * occurrence modifiée (`series` rempli) : elle seule se modifie. « Toute la
   * journée » vient de l'événement, pas de sa durée (24 h ou plus, F4).
   */
  const openOccurrence = (o: Occurrence) => {
    setDialog({ occurrence: o, calendar: o.event.calendar, start: o.start, end: o.end, allDay: o.allDay });
  };

  /** Nouveau rendez-vous : `minute` depuis minuit (null : toute la journée) ; durée d'une case, 30 min au moins. */
  const createAt = (dayStart: number, minute: number | null) => {
    if (minute === null) {
      setDialog({ occurrence: null, calendar: defaultCalendar, start: dayStart, end: addDays(dayStart, 1), allDay: true });
      return;
    }
    const d = new Date(dayStart);
    const start = new Date(d.getFullYear(), d.getMonth(), d.getDate(), 0, minute).getTime();
    setDialog({ occurrence: null, calendar: defaultCalendar, start, end: start + Math.max(scale, 30) * 60_000, allDay: false });
  };

  /** Nouveau rendez-vous sur le créneau choisi à la souris (minutes depuis minuit). */
  const createRange = (dayStart: number, from: number, to: number) => {
    setDialog({ occurrence: null, calendar: defaultCalendar, start: atMinute(dayStart, from), end: atMinute(dayStart, to), allDay: false });
  };

  /** Case visée dans une colonne de jour, en minutes depuis minuit, selon l'échelle (10, 15, 30 ou 60 min). */
  const slotAt = (e: { clientY: number; currentTarget: HTMLElement }) => {
    const y = e.clientY - e.currentTarget.getBoundingClientRect().top;
    const slot = Math.floor((y / HOUR_PX) * (60 / scale));
    return Math.max(0, Math.min(24 * 60 - scale, slot * scale));
  };

  const openWeek = (day: number) => {
    setCursor(day);
    updatePrefs({ mode: "week" });
  };

  // ---------------------------------------------------------------- panneau des agendas

  const groups = useMemo(() => {
    const map = new Map<string, PimCollection[]>();
    for (const c of calendars) map.set(c.account, [...(map.get(c.account) ?? []), c]);
    // « Sur cet ordinateur » en dernier.
    return [...map.entries()].sort(([a], [b]) => (a === "local" ? 1 : b === "local" ? -1 : accountName(a).localeCompare(accountName(b), locale())));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [calendars, accounts]);

  const sidebar = (
    <aside className="cal-sidebar">
      <Button appearance="primary" icon={<Add20Regular />} className="cal-new" onClick={() => createAt(startOfDay(Date.now()), Math.min(22, new Date().getHours() + 1) * 60)}>
        {t("Nouveau rendez-vous")}
      </Button>
      <MiniMonths
        months={miniMonths}
        firstDay={firstDay}
        weekdays={WEEKDAYS}
        weekNo={weekNo}
        now={now}
        shown={mode === "week" ? [from, to] : null}
        busy={busyDays}
        onPrevious={() => setMiniOffset((n) => n - 1)}
        onNext={() => setMiniOffset((n) => n + 1)}
        onPick={openWeek}
      />
      {groups.map(([account, list]) => (
        <div key={account} className="cal-group">
          <div className="cal-group-title">{accountName(account)}</div>
          {list.map((c) => {
            const color = colorFor(c.url, c.account);
            return (
              <div key={c.url} className="cal-item">
                <Checkbox
                  checked={!hidden.has(c.url)}
                  onChange={(_, d) =>
                    updatePrefs({ hidden: d.checked ? prefs.hidden.filter((u) => u !== c.url) : [...prefs.hidden, c.url] })
                  }
                  label={c.name}
                  className="cal-check"
                  style={{ ["--cal-color" as string]: color }}
                />
                <Popover positioning="below-end">
                  <PopoverTrigger disableButtonEnhancement>
                    <button type="button" className="cal-color" style={{ background: color }} title={t("Couleur")} aria-label={t("Couleur de {name}", { name: c.name })} />
                  </PopoverTrigger>
                  <PopoverSurface className="cal-palette">
                    {PALETTE.map((p) => (
                      <button
                        key={p}
                        type="button"
                        className={p === color ? "on" : undefined}
                        style={{ background: p }}
                        aria-label={colorName(p)}
                        title={colorName(p)}
                        onClick={() => updatePrefs({ colors: { ...prefs.colors, [c.url]: p } })}
                      />
                    ))}
                  </PopoverSurface>
                </Popover>
              </div>
            );
          })}
        </div>
      ))}
    </aside>
  );

  // ---------------------------------------------------------------- vue semaine

  const weekView = () => {
    const days = Array.from({ length: 7 }, (_, i) => addDays(from, i));
    const allDay = visible.filter(isAllDay);
    const timed = visible.filter((o) => !isAllDay(o));
    return (
      <div className="cal-grid">
        <div className="cal-head">
          <div className="cal-gutter" />
          {days.map((d) => (
            <div key={d} className={sameDay(d, now) ? "cal-dayhead today" : "cal-dayhead"}>
              <span className="cal-dayname">{dayName.format(d)}</span>
              <span className="cal-daynum">{new Date(d).getDate()}</span>
            </div>
          ))}
        </div>
        <div className="cal-allday">
          <div className="cal-gutter muted">{t("Journée")}</div>
          <div className="cal-allday-track" style={{ gridTemplateColumns: "repeat(7, minmax(0, 1fr))" }}>
            {allDay.map((o) => {
              // Colonnes d'après les dates du calendrier (jours de 23 ou 25 h au changement d'heure).
              const first = Math.max(0, dayIndex(from, Math.max(o.start, from)));
              const last = Math.min(6, dayIndex(from, Math.min(o.end, to) - 1));
              if (first > 6 || last < 0) return null;
              return (
                <button
                  key={o.key}
                  type="button"
                  className="cal-event allday"
                  style={{ gridColumn: `${first + 1} / ${Math.max(first, last) + 2}`, ["--event-color" as string]: colorOf(o) }}
                  onClick={() => openOccurrence(o)}
                  title={o.event.summary}
                >
                  {o.event.summary || t("(sans titre)")}
                </button>
              );
            })}
          </div>
        </div>
        <div className="cal-scroll" ref={scroller}>
          {events === null && <VortexSpinner className="list-loading" size={22} aria-label={t("Chargement")} />}
          <div className="cal-body" style={{ height: 24 * HOUR_PX }}>
            <div className="cal-gutter cal-hours">
              {Array.from({ length: 24 }, (_, h) => (
                <span key={h} style={{ top: h * HOUR_PX }}>
                  {h > 0 ? formatHour(h) : ""}
                </span>
              ))}
            </div>
            {days.map((d) => {
              const dayEnd = addDays(d, 1);
              const items = layout(timed.filter((o) => o.start < dayEnd && o.end > d));
              return (
                <div
                  key={d}
                  className="cal-day"
                  onDoubleClick={(e) => {
                    if (e.target !== e.currentTarget) return;
                    createAt(d, slotAt(e));
                  }}
                  // Glisser sur plusieurs cases : nouveau rendez-vous sur ce créneau.
                  // Un simple clic ne crée rien (le double-clic crée une case).
                  onPointerDown={(e) => {
                    if (e.button !== 0 || e.target !== e.currentTarget) return;
                    e.currentTarget.setPointerCapture(e.pointerId);
                    const m = slotAt(e);
                    setDrag({ day: d, anchor: m, from: m, to: m + scale });
                  }}
                  onPointerMove={(e) => {
                    if (!drag || drag.day !== d) return;
                    const m = slotAt(e);
                    const from = Math.min(drag.anchor, m);
                    const to = Math.max(drag.anchor, m) + scale;
                    if (from !== drag.from || to !== drag.to) setDrag({ ...drag, from, to });
                  }}
                  onPointerUp={() => {
                    if (!drag || drag.day !== d) return;
                    setDrag(null);
                    if (drag.to - drag.from > scale) createRange(d, drag.from, drag.to);
                  }}
                  onPointerCancel={() => setDrag(null)}
                >
                  {Array.from({ length: (24 * 60) / scale }, (_, i) => (
                    <div
                      key={i}
                      className={(i * scale) % 60 === 0 ? "cal-slot" : "cal-slot sub"}
                      style={{ top: (i * scale * HOUR_PX) / 60, height: (scale * HOUR_PX) / 60 }}
                    />
                  ))}
                  {sameDay(d, now) && <div className="cal-now" style={{ top: (minuteOfDay(now, d) / 60) * HOUR_PX }} />}
                  {drag && drag.day === d && (
                    <div
                      className="cal-selection"
                      style={{ top: (drag.from / 60) * HOUR_PX, height: ((drag.to - drag.from) / 60) * HOUR_PX }}
                      aria-hidden
                    >
                      {hourFmt.format(atMinute(d, drag.from))} – {hourFmt.format(atMinute(d, drag.to))}
                    </div>
                  )}
                  {items.map(({ o, col, cols }) => {
                    // Position d'après l'heure affichée, pas la durée depuis minuit :
                    // juste aussi les jours de changement d'heure (F15).
                    const startMin = minuteOfDay(o.start, d);
                    const endMin = minuteOfDay(o.end, d);
                    const top = (startMin / 60) * HOUR_PX;
                    const height = Math.max(20, ((endMin - startMin) / 60) * HOUR_PX - 2);
                    return (
                      <button
                        key={o.key}
                        type="button"
                        className="cal-event"
                        style={{
                          top,
                          height,
                          left: `calc(${(col / cols) * 100}% + 2px)`,
                          width: `calc(${100 / cols}% - 4px)`,
                          ["--event-color" as string]: colorOf(o),
                        }}
                        onClick={() => openOccurrence(o)}
                        title={`${hourFmt.format(o.start)} – ${hourFmt.format(o.end)} ${o.event.summary}`}
                      >
                        <span className="cal-event-title">{o.event.summary || t("(sans titre)")}</span>
                        {height > 34 && (
                          <span className="cal-event-meta">
                            {hourFmt.format(o.start)} – {hourFmt.format(o.end)}
                            {o.event.location ? ` · ${o.event.location}` : ""}
                          </span>
                        )}
                      </button>
                    );
                  })}
                </div>
              );
            })}
          </div>
        </div>
      </div>
    );
  };

  // ---------------------------------------------------------------- vue mois

  const monthView = () => {
    const month = new Date(cursor).getMonth();
    const days = Array.from({ length: 42 }, (_, i) => addDays(from, i));
    return (
      <div className={general.weekNumbers ? "cal-month with-weeks" : "cal-month"}>
        <div className="cal-month-head">
          {general.weekNumbers && <span className="cal-weekno-head" />}
          {WEEKDAYS.map((w) => (
            <span key={w}>{w}</span>
          ))}
        </div>
        <div className="cal-month-grid">
          {days.map((d, i) => {
            const dayEnd = addDays(d, 1);
            const weekCell =
              general.weekNumbers && i % 7 === 0 ? (
                <button key={`w${d}`} type="button" className="cal-weekno" onClick={() => openWeek(d)} title={t("Voir la semaine")}>
                  {weekNo(d)}
                </button>
              ) : null;
            const items = visible.filter((o) => o.start < dayEnd && o.end > d).sort((a, b) => Number(isAllDay(b)) - Number(isAllDay(a)) || a.start - b.start);
            const outside = new Date(d).getMonth() !== month;
            return [
              weekCell,
              <div
                key={d}
                className={["cal-cell", outside && "outside", sameDay(d, now) && "today"].filter(Boolean).join(" ")}
                onDoubleClick={(e) => e.target === e.currentTarget && createAt(d, null)}
              >
                <button type="button" className="cal-cell-num" onClick={() => openWeek(d)} title={t("Voir la semaine")}>
                  {new Date(d).getDate()}
                </button>
                {items.slice(0, 3).map((o) => (
                  <button
                    key={o.key}
                    type="button"
                    className={isAllDay(o) ? "cal-chip allday" : "cal-chip"}
                    style={{ ["--event-color" as string]: colorOf(o) }}
                    onClick={() => openOccurrence(o)}
                    title={o.event.summary}
                  >
                    {!isAllDay(o) && <span className="cal-chip-time">{hourFmt.format(o.start)}</span>}
                    {o.event.summary || t("(sans titre)")}
                  </button>
                ))}
                {items.length > 3 && (
                  <button type="button" className="cal-more" onClick={() => openWeek(d)}>
                    {t("{n} de plus", { n: items.length - 3 })}
                  </button>
                )}
              </div>,
            ];
          })}
        </div>
      </div>
    );
  };

  // ---------------------------------------------------------------- vue année

  const yearView = () => {
    const year = new Date(cursor).getFullYear();
    // Jours occupés : couleur du premier rendez-vous du jour.
    const busy = new Map<string, string>();
    for (const o of visible) {
      for (let d = startOfDay(Math.max(o.start, from)); d < Math.min(o.end, to); d = addDays(d, 1)) {
        const key = new Date(d).toDateString();
        if (!busy.has(key)) busy.set(key, colorOf(o));
        if (o.end - o.start <= 0) break;
      }
    }
    return (
      <div className="cal-year">
        {Array.from({ length: 12 }, (_, m) => {
          const first = new Date(year, m, 1).getTime();
          const start = weekStart(first, firstDay);
          const cells = Array.from({ length: 42 }, (_, i) => addDays(start, i));
          return (
            <div key={m} className="cal-mini">
              <button type="button" className="cal-mini-title" onClick={() => { setCursor(first); updatePrefs({ mode: "month" }); }}>
                {monthName.format(first)}
              </button>
              <div className={general.weekNumbers ? "cal-mini-grid with-weeks" : "cal-mini-grid"}>
                {general.weekNumbers && <span className="cal-mini-wd" />}
                {WEEKDAYS.map((w) => (
                  <span key={w} className="cal-mini-wd">{w.charAt(0).toUpperCase()}</span>
                ))}
                {cells.map((d, i) => {
                  const inMonth = new Date(d).getMonth() === m;
                  // Numéro de semaine en début de ligne, pour les lignes qui ont des jours du mois.
                  const rowHasMonth = i % 7 === 0 && [0, 6].some((k) => new Date(addDays(d, k)).getMonth() === m);
                  const weekCell =
                    general.weekNumbers && i % 7 === 0 ? (
                      <button
                        key={`w${d}`}
                        type="button"
                        className={rowHasMonth ? "cal-mini-weekno" : "cal-mini-weekno outside"}
                        onClick={() => rowHasMonth && openWeek(inMonth ? d : addDays(d, 6))}
                        title={t("Voir la semaine")}
                        tabIndex={rowHasMonth ? 0 : -1}
                      >
                        {weekNo(d)}
                      </button>
                    ) : null;
                  const color = inMonth ? busy.get(new Date(d).toDateString()) : undefined;
                  return [
                    weekCell,
                    <button
                      key={d}
                      type="button"
                      className={["cal-mini-day", !inMonth && "outside", sameDay(d, now) && "today", color && "busy"].filter(Boolean).join(" ")}
                      style={color ? { ["--event-color" as string]: color } : undefined}
                      onClick={() => inMonth && openWeek(d)}
                      tabIndex={inMonth ? 0 : -1}
                    >
                      {new Date(d).getDate()}
                    </button>,
                  ];
                })}
              </div>
            </div>
          );
        })}
      </div>
    );
  };

  return (
    <div className="pim-view calendar-view">
      {sidebar}
      <div className="cal-main">
        <div className="cal-toolbar">
          <Button
            onClick={() => {
              setCursor(startOfDay(Date.now()));
              setMiniOffset(0);
            }}
          >
            {t("Aujourd'hui")}
          </Button>
          <Button appearance="subtle" icon={<ChevronLeft20Regular />} aria-label={t("Précédent")} onClick={() => move(-1)} />
          <Button appearance="subtle" icon={<ChevronRight20Regular />} aria-label={t("Suivant")} onClick={() => move(1)} />
          <h2 className="cal-title">{title}</h2>
          <span className="status-spacer" />
          <div className="fmt-segmented cal-modes" role="radiogroup" aria-label={t("Vue")}>
            {(
              [
                ["week", t("Semaine")],
                ["month", t("Mois")],
                ["year", t("Année")],
              ] as const
            ).map(([value, label]) => (
              <button
                key={value}
                type="button"
                role="radio"
                aria-checked={mode === value}
                className={mode === value ? "on" : ""}
                onClick={() => updatePrefs({ mode: value })}
              >
                {label}
              </button>
            ))}
          </div>
          <PimTransfer kind="calendar" collections={calendars} accounts={accounts} notify={notify} onImported={() => void reload()} />
          <Button appearance="subtle" icon={syncing ? <VortexSpinner size={16} /> : <ArrowClockwise20Regular />} disabled={syncing} onClick={onSync}>
            {t("Synchroniser")}
          </Button>
        </div>
        {mode === "week" ? weekView() : mode === "month" ? monthView() : yearView()}
      </div>

      {dialog && dialog.occurrence && !editing ? (
        // Rendez-vous existant : sa fiche d'abord ; « Modifier » ouvre l'éditeur (lot H1b).
        <EventSheet
          draft={dialog}
          calendars={calendars}
          accounts={accounts}
          onClose={() => setDialog(null)}
          onEdit={() => setEditing(true)}
          onSaved={async (message) => {
            setDialog(null);
            await reload();
            notify(message);
          }}
          onError={(title, e) => notify(title, String(e), "error")}
        />
      ) : dialog && (
        <EventDialog
          draft={dialog}
          calendars={calendars}
          accounts={accounts}
          onClose={() => setDialog(null)}
          onSaved={async (message) => {
            setDialog(null);
            await reload();
            notify(message);
          }}
          onError={(title, e) => notify(title, String(e), "error")}
        />
      )}
    </div>
  );
}
