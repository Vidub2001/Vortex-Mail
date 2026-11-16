// Création et modification d'un rendez-vous.
import {
  Button,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Input,
  Menu,
  MenuButton,
  MenuItem,
  MenuList,
  MenuPopover,
  MenuTrigger,
  Select,
  Spinner,
  Switch,
  Textarea,
} from "@fluentui/react-components";
import {
  Alert20Regular,
  ArrowRepeatAll20Regular,
  CalendarLtr20Regular,
  Clock20Regular,
  Delete20Regular,
  Dismiss20Regular,
  Location20Regular,
  People20Regular,
  Person20Regular,
  Save20Regular,
  TextDescription20Regular,
  Video20Regular,
} from "@fluentui/react-icons";
import { useRef, useState } from "react";
import { api } from "../api";
import { ConfirmDialog } from "./Dialogs";
import { SourceDialog } from "./EventSheet";
import { GuestList } from "./Guests";
import { meetingLink } from "../invites";
import { addDays, dayDiff, durationMs, eventZone, instant, toDateValue, toWallValue, type Occurrence } from "../calendar";
import type { Account, CalEvent, PimCollection } from "../types";
import { REMINDER_CHOICES, formatDate, loadGeneral, reminderLabel } from "../general";
import { locale, t } from "../i18n";
import {
  DAY_LONG,
  DAY_SHORT,
  buildRecurrence,
  describeRecurrence,
  editable,
  moveRecurrence,
  parseRecurrence,
  weekdayRank,
  type Freq,
  type Recurrence,
} from "../recurrence";

/** Rendez-vous à créer (`occurrence` nulle) ou à modifier. */
export interface EventDraft {
  occurrence: Occurrence | null;
  calendar: string;
  start: number;
  end: number;
  allDay: boolean;
}

interface Props {
  draft: EventDraft;
  calendars: PimCollection[];
  accounts: Account[];
  onClose: () => void;
  onSaved: (message: string) => void;
  onError: (title: string, e: unknown) => void;
}

const pad = (n: number) => String(n).padStart(2, "0");
const dateInput = (t: number) => {
  const d = new Date(t);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};
const timeInput = (t: number) => {
  const d = new Date(t);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
};
const fromInputs = (date: string, time: string) => new Date(`${date}T${time || "00:00"}`).getTime();

/** Répétitions proposées directement ; le reste passe par « Personnalisée… ». */
const PRESETS = ["", "FREQ=DAILY", "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR", "FREQ=WEEKLY", "FREQ=MONTHLY", "FREQ=YEARLY"];

const UNITS: Record<Freq, "day" | "week" | "month" | "year"> = { DAILY: "day", WEEKLY: "week", MONTHLY: "month", YEARLY: "year" };
/** Nom de l'unité accordé au nombre, dans la langue de l'interface : « jours », « mois », « weeks »… */
const unitName = (f: Freq, n: number) =>
  new Intl.NumberFormat(locale(), { style: "unit", unit: UNITS[f], unitDisplay: "long" })
    .formatToParts(n)
    .filter((p) => p.type === "unit")
    .map((p) => p.value)
    .join("")
    .trim();

/** Réglage détaillé : intervalle, jours de la semaine, jour du mois, fin. */
function RecurrenceEditor({
  rule,
  startIso,
  allDay,
  onChange,
}: {
  rule: string;
  startIso: string;
  allDay: boolean;
  onChange: (rule: string) => void;
}) {
  const rec = parseRecurrence(rule, startIso);
  const set = (patch: Partial<Recurrence>) => onChange(buildRecurrence({ ...rec, ...patch }, startIso, allDay));
  const start = new Date(`${startIso}T00:00`);
  const { nth, last } = weekdayRank(start);
  const firstDay = loadGeneral().weekStart;
  const order = Array.from({ length: 7 }, (_, i) => (firstDay + i) % 7);
  const day = DAY_LONG[start.getDay()];

  return (
    <div className="event-recur">
      <div className="event-recur-line">
        <span>{rec.freq === "WEEKLY" ? t("Répéter toutes les") : t("Répéter tous les")}</span>
        <Input
          type="number"
          className="event-recur-interval"
          min={1}
          max={99}
          value={String(rec.interval)}
          onChange={(_, d) => set({ interval: Math.min(99, Math.max(1, Number(d.value) || 1)) })}
          aria-label={t("Intervalle")}
        />
        <Select value={rec.freq} onChange={(_, d) => set({ freq: d.value as Freq })} aria-label={t("Unité")}>
          {(Object.keys(UNITS) as Freq[]).map((f) => (
            <option key={f} value={f}>
              {unitName(f, rec.interval)}
            </option>
          ))}
        </Select>
      </div>

      {rec.freq === "WEEKLY" && (
        <div className="event-recur-line">
          <span>{t("Le")}</span>
          <div className="event-days" role="group" aria-label={t("Jours de la semaine")}>
            {order.map((d) => {
              const on = rec.days.includes(d);
              return (
                <button
                  key={d}
                  type="button"
                  className={on ? "event-day on" : "event-day"}
                  aria-pressed={on}
                  title={DAY_LONG[d]}
                  onClick={() => {
                    const days = on ? rec.days.filter((x) => x !== d) : [...rec.days, d];
                    // Au moins un jour.
                    if (days.length) set({ days });
                  }}
                >
                  {DAY_SHORT[d]}
                </button>
              );
            })}
          </div>
        </div>
      )}

      {rec.freq === "MONTHLY" && (
        <div className="event-recur-line">
          <span>{t("Le")}</span>
          <Select
            value={rec.monthly}
            onChange={(_, d) => set({ monthly: d.value as Recurrence["monthly"] })}
            aria-label={t("Jour du mois")}
          >
            <option value="monthday">
              {start.getDate() === 1 ? t("1er du mois") : t("{d} du mois", { d: start.getDate() })}
            </option>
            {nth <= 4 && (
              <option value="nth">
                {nth === 1
                  ? t("1er {day} du mois", { day })
                  : nth === 2
                    ? t("2e {day} du mois", { day })
                    : nth === 3
                      ? t("3e {day} du mois", { day })
                      : t("4e {day} du mois", { day })}
              </option>
            )}
            {last && <option value="last">{t("dernier {day} du mois", { day })}</option>}
          </Select>
        </div>
      )}

      <div className="event-recur-line">
        <span>{t("Fin")}</span>
        <Select value={rec.end} onChange={(_, d) => set({ end: d.value as Recurrence["end"] })} aria-label={t("Fin")}>
          <option value="never">{t("Jamais")}</option>
          <option value="until">{t("Le")}</option>
          <option value="count">{t("Après")}</option>
        </Select>
        {rec.end === "until" && (
          <Input type="date" value={rec.until} min={startIso} onChange={(_, d) => d.value && set({ until: d.value })} aria-label={t("Date de fin")} />
        )}
        {rec.end === "count" && (
          <>
            <Input
              type="number"
              className="event-recur-interval"
              min={1}
              max={999}
              value={String(rec.count)}
              onChange={(_, d) => set({ count: Math.min(999, Math.max(1, Number(d.value) || 1)) })}
              aria-label={t("Nombre d'occurrences")}
            />
            <span>{rec.count > 1 ? t("occurrences") : t("occurrence")}</span>
          </>
        )}
      </div>
      <p className="event-recur-summary">{describeRecurrence(rule, startIso)}</p>
    </div>
  );
}

/**
 * Bouton « Supprimer » d'un rendez-vous existant, avec confirmation : pour une
 * série, l'occurrence seule ou toute la série (relecture du 12 novembre, n° 44).
 * Partagé par l'éditeur et la fiche (lot H1b).
 */
export function EventDelete({
  draft,
  busy,
  setBusy,
  onSaved,
  onError,
}: {
  draft: EventDraft;
  busy: boolean;
  setBusy: (busy: boolean) => void;
  onSaved: (message: string) => void;
  onError: (title: string, e: unknown) => void;
}) {
  const original = draft.occurrence?.event ?? null;
  const parent = draft.occurrence?.series ?? null;
  const series = !!original?.rrule;
  const [confirming, setConfirming] = useState<"one" | "all" | null>(null);
  if (!original || !draft.occurrence) return null;
  const occurrence = draft.occurrence;
  const title = original.summary || t("(sans titre)");

  const remove = async (onlyThis: boolean) => {
    setBusy(true);
    try {
      // Série concernée : celle de l'occurrence modifiée, sinon l'événement lui-même.
      const target = parent ?? original;
      if (onlyThis) {
        // Valeur d'origine de l'occurrence (et non sa date déplacée) : F8.
        const exdate = occurrence.occurrenceValue;
        await api.saveEvent({ ...target, recurrenceId: "", exdates: [...target.exdates, exdate] }, target.calendar);
        onSaved(t("Occurrence supprimée"));
      } else {
        await api.pimDelete(target.id);
        onSaved(series || parent ? t("Série supprimée") : t("Rendez-vous supprimé"));
      }
    } catch (e) {
      onError(t("Suppression impossible"), e);
      setBusy(false);
    }
  };

  return (
    <>
      {series || parent ? (
        <Menu positioning="above-start">
          <MenuTrigger disableButtonEnhancement>
            <MenuButton appearance="subtle" className="event-delete" icon={<Delete20Regular />} disabled={busy}>
              {t("Supprimer")}
            </MenuButton>
          </MenuTrigger>
          <MenuPopover>
            <MenuList>
              <MenuItem onClick={() => setConfirming("one")}>{t("Cette occurrence")}</MenuItem>
              <MenuItem onClick={() => setConfirming("all")}>{t("Toute la série")}</MenuItem>
            </MenuList>
          </MenuPopover>
        </Menu>
      ) : (
        <Button appearance="subtle" className="event-delete" icon={<Delete20Regular />} disabled={busy} onClick={() => setConfirming("all")}>
          {t("Supprimer")}
        </Button>
      )}
      {confirming && (
        <ConfirmDialog
          title={confirming === "one" ? t("Supprimer cette occurrence") : series || parent ? t("Supprimer la série") : t("Supprimer le rendez-vous")}
          message={
            confirming === "one"
              ? t("L'occurrence du {date} de « {title} » sera supprimée de l'agenda.", { date: formatDate(draft.start), title })
              : series || parent
                ? t("Toutes les occurrences de « {title} » seront supprimées de l'agenda.", { title })
                : t("« {title} » sera supprimé de l'agenda.", { title })
          }
          confirm={t("Supprimer")}
          onConfirm={() => remove(confirming === "one")}
          onClose={() => setConfirming(null)}
        />
      )}
    </>
  );
}

export function EventDialog({ draft, calendars, accounts, onClose, onSaved, onError }: Props) {
  const original = draft.occurrence?.event ?? null;
  const series = !!original?.rrule;
  /** Occurrence modifiée d'une série : elle seule se modifie ; la série sert à la supprimer. */
  const parent = draft.occurrence?.series ?? null;
  /**
   * UID d'un nouveau rendez-vous, choisi une fois pour toutes : si
   * l'enregistrement échoue sur un délai dépassé alors que le serveur l'a
   * reçu, le nouvel essai remplace ce rendez-vous au lieu d'en créer un second.
   */
  const newUid = useRef(`${crypto.randomUUID()}@vortexmail`);
  const single = !!original?.recurrenceId;
  const [summary, setSummary] = useState(original?.summary ?? "");
  const [location, setLocation] = useState(original?.location ?? "");
  const [description, setDescription] = useState(original?.description ?? "");
  const [calendar, setCalendar] = useState(draft.calendar);
  const [allDay, setAllDay] = useState(draft.allDay);
  // Une série se modifie à partir de sa première occurrence.
  const [startDate, setStartDate] = useState(dateInput(draft.start));
  const [startTime, setStartTime] = useState(timeInput(draft.start));
  const [endDate, setEndDate] = useState(dateInput(draft.allDay ? draft.end - 1 : draft.end));
  const [endTime, setEndTime] = useState(timeInput(draft.end));
  const [rule, setRule] = useState(original?.rrule ?? "");
  // Rappel : celui du rendez-vous, ou celui par défaut (Paramètres › Agenda) pour un nouveau.
  const [reminder, setReminder] = useState<number>(() =>
    original ? (original.reminder ?? -1) : loadGeneral().defaultReminder,
  );
  const [busy, setBusy] = useState(false);
  /** Source du rendez-vous affichée (lot H1b). */
  const [source, setSource] = useState(false);
  // Réglage détaillé ouvert : règle hors des choix simples que l'éditeur sait
  // relire, ou « Personnalisée… » choisi. Une règle qu'il ne sait pas relire
  // (venue d'un autre logiciel) est seulement décrite, et gardée telle quelle.
  const [customOpen, setCustomOpen] = useState(
    () => !!rule && !PRESETS.includes(rule) && editable(rule, dateInput(original && series ? instant(original.start) : draft.start)),
  );

  const accountName = (id: string) => (id === "local" ? t("Sur cet ordinateur") : (accounts.find((a) => a.id === id)?.name ?? id));

  const save = async () => {
    let start = allDay ? fromInputs(startDate, "00:00") : fromInputs(startDate, startTime);
    // Journée entière : la fin est le lendemain du dernier jour, en jours de
    // calendrier (le jour du changement d'heure dure 23 ou 25 h : F10).
    let end = allDay ? addDays(fromInputs(endDate, "00:00"), 1) : fromInputs(endDate, endTime);
    if (!(end > start)) end = allDay ? addDays(start, 1) : start + 3_600_000;
    const timesChanged = !original || start !== draft.start || end !== draft.end || allDay !== draft.allDay;
    const base: CalEvent = original ?? {
      id: "",
      source: "",
      calendar,
      uid: newUid.current,
      summary: "",
      location: "",
      description: "",
      start: { value: "", tzid: "" },
      end: { value: "", tzid: "" },
      duration: "",
      allDay,
      rrule: "",
      exdates: [],
      recurrenceId: "",
      organizer: "",
    };
    const event: CalEvent = {
      ...base,
      summary: summary.trim(),
      location: location.trim(),
      description,
      allDay,
      // Occurrence modifiée : ni répétition, ni changement d'agenda ; elle garde son RECURRENCE-ID.
      rrule: single ? "" : rule,
      calendar: single ? base.calendar : calendar,
      // Durée d'origine (rendez-vous sans fin explicite) gardée tant que l'horaire ne change pas.
      duration: base.duration,
      recurrenceId: single ? base.recurrenceId : "",
      reminder: reminder < 0 ? null : reminder,
    };
    // Heure enregistrée à l'heure locale de son fuseau, jamais en UTC : une
    // réunion à 9 h reste à 9 h après un changement d'heure (relecture F1).
    // Une série enregistrée en UTC par une version précédente est convertie.
    const storedInUtc = !base.allDay && base.start.value.endsWith("Z");
    if (timesChanged || (original && !allDay && storedInUtc)) {
      if (original && series && timesChanged && allDay) {
        // Série de journées entières : écart et durée en jours de calendrier,
        // pas en heures, sinon elle se décale d'un jour au changement d'heure (F10).
        const length = dayDiff(start, end);
        start = addDays(instant(original.start), dayDiff(draft.start, start));
        end = addDays(start, length);
      } else if (original && series && timesChanged) {
        // Série : l'écart appliqué à cette occurrence décale toute la série.
        const length = end - start;
        start = instant(original.start) + (start - draft.start);
        end = start + length;
      } else if (original && series) {
        // Horaire inchangé : seul le format change (UTC → heure locale), à partir du début de la série.
        start = instant(original.start);
        end = original.end.value ? instant(original.end) : start + (durationMs(original.duration) || 3_600_000);
      }
      const tzid = eventZone(storedInUtc ? "" : base.start.tzid);
      event.start = allDay ? { value: toDateValue(start), tzid: "" } : { value: toWallValue(start, tzid), tzid };
      event.end = allDay ? { value: toDateValue(end), tzid: "" } : { value: toWallValue(end, tzid), tzid };
      event.duration = "";
    }
    setBusy(true);
    try {
      await api.saveEvent(event, calendar);
      onSaved(original ? t("Rendez-vous modifié") : t("Rendez-vous ajouté"));
    } catch (e) {
      onError(t("Rendez-vous non enregistré"), e);
      setBusy(false);
    }
  };

  /** Durée lisible entre début et fin (« 1 h », « 1 h 30 », « 2 jours »). */
  const durationText = (() => {
    if (allDay) {
      const days = Math.round((fromInputs(endDate, "00:00") - fromInputs(startDate, "00:00")) / 86_400_000) + 1;
      return days > 1 ? t("{n} jours", { n: days }) : "";
    }
    const minutes = Math.round((fromInputs(endDate, endTime) - fromInputs(startDate, startTime)) / 60_000);
    if (!(minutes > 0)) return "";
    const h = Math.floor(minutes / 60);
    const m = minutes % 60;
    if (h >= 24 && m === 0 && h % 24 === 0) return h > 24 ? t("{n} jours", { n: h / 24 }) : t("1 jour");
    if (h === 0) return t("{m} min", { m });
    return m === 0 ? t("{h} h", { h }) : t("{h} h {m}", { h, m: String(m).padStart(2, "0") });
  })();

  /**
   * Début changé : la fin suit, durée conservée. Pendant la saisie au clavier,
   * la date passe par des valeurs vides ou incomplètes : l'écart se calcule
   * toujours depuis le dernier début valide, sinon la fin dérive (d'un an…).
   */
  const lastStart = useRef({ date: startDate, time: startTime });
  const changeStart = (date: string, time: string) => {
    setStartDate(date);
    setStartTime(time);
    const valid = /^\d{4}-\d{2}-\d{2}$/.test(date) && +date.slice(0, 4) >= 1900 && (allDay || /^\d{2}:\d{2}$/.test(time));
    if (!valid) return;
    const before = fromInputs(lastStart.current.date, allDay ? "00:00" : lastStart.current.time);
    const after = fromInputs(date, allDay ? "00:00" : time);
    const end = fromInputs(endDate, allDay ? "00:00" : endTime);
    if (Number.isNaN(after)) return;
    const previous = lastStart.current;
    lastStart.current = { date, time };
    // La règle suit la nouvelle date (jour de la semaine, rang dans le mois : n° 36).
    if (rule) setRule(moveRecurrence(rule, ruleStartFor(previous.date, previous.time), ruleStartFor(date, time), allDay));
    if (!Number.isNaN(before) && !Number.isNaN(end) && after !== before) {
      const moved = end + (after - before);
      setEndDate(dateInput(moved));
      if (!allDay) setEndTime(timeInput(moved));
    }
  };

  /**
   * Début de la série (date) : la règle se construit et se décrit à partir de
   * lui, pas de l'occurrence ouverte (sinon le jour de la semaine change : F6).
   */
  const ruleStartFor = (date: string, time: string) => {
    if (!original || !series) return date;
    const shift = fromInputs(date, allDay ? "00:00" : time) - draft.start;
    const t0 = allDay
      ? addDays(instant(original.start), Number.isNaN(shift) ? 0 : dayDiff(draft.start, fromInputs(date, "00:00")))
      : instant(original.start) + (Number.isNaN(shift) ? 0 : shift);
    return Number.isNaN(t0) ? date : dateInput(t0);
  };
  const ruleStart = ruleStartFor(startDate, startTime);

  // Suit ce qui est saisi : un lien collé dans le lieu fait apparaître le bouton.
  const meeting = meetingLink({ ...(original ?? ({} as CalEvent)), location, description });

  const calendarLabel = (c: PimCollection) => (c.account === "local" ? c.name : `${c.name} — ${accountName(c.account)}`);

  return (
    <Dialog open onOpenChange={(_, d) => !d.open && !busy && onClose()}>
      <DialogSurface
        className="event-dialog"
        onKeyDown={(e) => {
          // Ctrl+Entrée : enregistrer.
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !busy) {
            e.preventDefault();
            save();
          }
        }}
      >
        <DialogBody>
          <DialogTitle
            action={
              <Button appearance="subtle" aria-label={t("Fermer")} icon={<Dismiss20Regular />} disabled={busy} onClick={onClose} />
            }
          >
            {original ? t("Rendez-vous") : t("Nouveau rendez-vous")}
          </DialogTitle>
          <DialogContent className="event-form">
            <Input
              className="event-title"
              size="large"
              appearance="underline"
              value={summary}
              autoFocus={!original}
              onChange={(_, d) => setSummary(d.value)}
              placeholder={t("Ajouter un titre")}
              aria-label={t("Titre")}
            />

            {original?.organizer && (
              <div className="event-row">
                <Person20Regular className="event-icon" aria-hidden />
                <span className="muted">{t("Organisé par {name}", { name: original.organizer })}</span>
              </div>
            )}
            {!!(parent ?? original)?.attendees?.length && (
              // Organisateur, invités et leur réponse en tête, comme dans Thunderbird (lot H1b).
              <div className="event-row top">
                <People20Regular className="event-icon" aria-hidden />
                <GuestList attendees={(parent ?? original)?.attendees ?? []} />
              </div>
            )}

            <div className="event-row">
              <CalendarLtr20Regular className="event-icon" aria-hidden />
              <Select value={calendar} onChange={(_, d) => setCalendar(d.value)} aria-label={t("Agenda")} disabled={single}>
                {calendars.map((c) => (
                  <option key={c.url} value={c.url}>
                    {calendarLabel(c)}
                  </option>
                ))}
              </Select>
            </div>

            <div className="event-row top">
              <Clock20Regular className="event-icon" aria-hidden />
              <div className="event-times">
                <span className="event-time-label">{t("Début")}</span>
                <Input type="date" value={startDate} onChange={(_, d) => changeStart(d.value, startTime)} aria-label={t("Date de début")} />
                {allDay ? (
                  <span />
                ) : (
                  <Input type="time" value={startTime} onChange={(_, d) => changeStart(startDate, d.value)} aria-label={t("Heure de début")} />
                )}
                <Switch
                  checked={allDay}
                  onChange={(_, d) => {
                    setAllDay(d.checked);
                    // Fin de répétition (UNTIL) écrite comme le début : date seule ou date-heure (RFC 5545).
                    if (rule && editable(rule, ruleStart)) setRule(buildRecurrence(parseRecurrence(rule, ruleStart), ruleStart, d.checked));
                  }}
                  label={t("Toute la journée")}
                />

                <span className="event-time-label">{t("Fin")}</span>
                <Input type="date" value={endDate} onChange={(_, d) => setEndDate(d.value)} aria-label={t("Date de fin")} />
                {allDay ? (
                  <span />
                ) : (
                  <Input type="time" value={endTime} onChange={(_, d) => setEndTime(d.value)} aria-label={t("Heure de fin")} />
                )}
                <span className="muted event-duration">{durationText}</span>
              </div>
            </div>

            <div className="event-row">
              <ArrowRepeatAll20Regular className="event-icon" aria-hidden />
              <div className="event-pair">
                <Select
                  disabled={single}
                  value={customOpen ? "custom" : rule}
                  onChange={(_, d) => {
                    if (d.value === "custom") {
                      setCustomOpen(true);
                      // Point de départ : la règle actuelle, sinon chaque semaine le jour du début.
                      if (!rule) setRule(buildRecurrence(parseRecurrence("FREQ=WEEKLY", ruleStart), ruleStart, allDay));
                    } else {
                      setCustomOpen(false);
                      setRule(d.value);
                    }
                  }}
                  aria-label={t("Répétition")}
                >
                  {PRESETS.map((r) => (
                    <option key={r} value={r}>
                      {describeRecurrence(r, ruleStart)}
                    </option>
                  ))}
                  {rule && !PRESETS.includes(rule) && !customOpen && <option value={rule}>{describeRecurrence(rule, ruleStart)}</option>}
                  <option value="custom">{t("Personnalisée…")}</option>
                </Select>
                <span className="event-pair-item">
                  <Alert20Regular className="event-icon" aria-hidden />
                  <Select value={String(reminder)} onChange={(_, d) => setReminder(Number(d.value))} aria-label={t("Rappel")}>
                    {!REMINDER_CHOICES.some((c) => c.value === reminder) && (
                      <option value={String(reminder)}>{reminderLabel(reminder)}</option>
                    )}
                    {REMINDER_CHOICES.map((c) => (
                      <option key={c.value} value={String(c.value)}>
                        {c.value < 0 ? t("Pas de rappel") : t("Rappel : {label}", { label: c.label.toLocaleLowerCase(locale()) })}
                      </option>
                    ))}
                  </Select>
                </span>
              </div>
            </div>
            {customOpen && (
              <RecurrenceEditor rule={rule || "FREQ=WEEKLY"} startIso={ruleStart} allDay={allDay} onChange={setRule} />
            )}
            {series && <p className="muted event-note">{t("Les modifications s'appliquent à toute la série.")}</p>}
            {single && <p className="muted event-note">{t("Les modifications ne portent que sur cette occurrence.")}</p>}

            <div className="event-row">
              <Location20Regular className="event-icon" aria-hidden />
              <Input value={location} onChange={(_, d) => setLocation(d.value)} placeholder={t("Ajouter un lieu")} aria-label={t("Lieu")} />
            </div>

            {meeting && (
              // Lien de visioconférence (Google Meet, Teams, Zoom…) trouvé dans le lieu ou la description (lot H1b).
              <div className="event-row">
                <Video20Regular className="event-icon" aria-hidden />
                <span>
                  <Button appearance="primary" disabled={busy} onClick={() => void api.openUrl(meeting.url)}>
                    {t("Participer avec {service}", { service: meeting.service })}
                  </Button>
                </span>
              </div>
            )}

            <div className="event-row top">
              <TextDescription20Regular className="event-icon" aria-hidden />
              <Textarea
                value={description}
                rows={4}
                resize="vertical"
                onChange={(_, d) => setDescription(d.value)}
                placeholder={t("Ajouter une description")}
                aria-label={t("Description")}
              />
            </div>

          </DialogContent>

          <DialogActions fluid className="event-actions">
            {original && <EventDelete draft={draft} busy={busy} setBusy={setBusy} onSaved={onSaved} onError={onError} />}
            {original && (
              <Button appearance="subtle" disabled={busy} onClick={() => setSource(true)}>
                {t("Source")}
              </Button>
            )}
            <span className="event-actions-spacer" />
            <Button disabled={busy} onClick={onClose}>
              {t("Annuler")}
            </Button>
            <Button
              appearance="primary"
              disabled={busy}
              onClick={save}
              icon={busy ? <Spinner size="tiny" /> : <Save20Regular />}
              title={t("Enregistrer (Ctrl+Entrée)")}
            >
              {t("Enregistrer")}
            </Button>
          </DialogActions>
        </DialogBody>
        {source && original && <SourceDialog id={original.id} onClose={() => setSource(false)} />}
      </DialogSurface>
    </Dialog>
  );
}
