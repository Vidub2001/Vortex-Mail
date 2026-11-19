// Fiche d'un rendez-vous (lot H1b) : ce qui s'ouvre d'un clic dans l'agenda.
// - Rendez-vous organisé par quelqu'un d'autre : en lecture, avec la réponse
//   du compte (au choix du compte quand plusieurs sont invités) ; seul le
//   rappel se modifie.
// - Autre rendez-vous : même fiche, avec « Modifier » et « Supprimer ».
import {
  Avatar,
  Button,
  Checkbox,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Select,
  Spinner,
} from "@fluentui/react-components";
import {
  Alert20Regular,
  ArrowRepeatAll20Regular,
  Clock20Regular,
  Dismiss20Regular,
  Edit20Regular,
  Location20Regular,
  People20Regular,
  Person20Regular,
  TextDescription20Regular,
  Video20Regular,
} from "@fluentui/react-icons";
import { Fragment, useEffect, useState, type ReactNode } from "react";
import { api } from "../api";
import { instant } from "../calendar";
import { REMINDER_CHOICES, reminderLabel } from "../general";
import { t } from "../i18n";
import { rangeLabel } from "../invitation";
import { answerOf, cleanDescription, invitedAccounts, isInvited, meetingLink } from "../invites";
import { describeRecurrence } from "../recurrence";
import { trimEndOf } from "../text";
import type { Account, CalEvent, InvitationAnswer, PimCollection } from "../types";
import { AnswerAs, GuestList } from "./Guests";
import { EventDelete, type EventDraft } from "./EventDialog";

interface Props {
  draft: EventDraft;
  calendars: PimCollection[];
  accounts: Account[];
  onClose: () => void;
  /** Passe à l'éditeur (rendez-vous qu'on organise). */
  onEdit: () => void;
  onSaved: (message: string) => void;
  onError: (title: string, e: unknown) => void;
}

const lower = (s: string) => s.trim().toLowerCase();

const URL_PATTERN = /https?:\/\/[^\s<>"]+/g;

/** Texte où les adresses web s'ouvrent dans le navigateur (jamais dans l'application). */
function Linked({ text }: { text: string }) {
  const parts: ReactNode[] = [];
  let last = 0;
  for (const found of text.matchAll(URL_PATTERN)) {
    const url = trimEndOf(found[0], ").,;");
    const at = found.index ?? 0;
    parts.push(text.slice(last, at));
    parts.push(
      <a
        key={at}
        href={url}
        onClick={(e) => {
          e.preventDefault();
          void api.openUrl(url);
        }}
      >
        {url}
      </a>,
    );
    last = at + url.length;
  }
  parts.push(text.slice(last));
  return <>{parts.map((p, i) => (typeof p === "string" ? <Fragment key={`t${i}`}>{p}</Fragment> : p))}</>;
}

/**
 * Source d'un rendez-vous telle que le serveur l'a donnée (iCalendar, ou JSON
 * d'Outlook.com), pour comprendre un rendez-vous mal lu (lot H1b).
 */
export function SourceDialog({ id, onClose }: { id: string; onClose: () => void }) {
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    api
      .pimSource(id)
      .then(setText)
      .catch((e: unknown) => setText(String(e)));
  }, [id]);
  return (
    <Dialog open onOpenChange={(_, d) => !d.open && onClose()}>
      <DialogSurface className="source-dialog">
        <DialogBody>
          <DialogTitle>{t("Source du rendez-vous")}</DialogTitle>
          <DialogContent>{text === null ? <Spinner size="small" /> : <pre className="source-text">{text}</pre>}</DialogContent>
          <DialogActions>
            <Button disabled={!text} onClick={() => text && void navigator.clipboard.writeText(text)}>
              {t("Copier")}
            </Button>
            <Button appearance="primary" onClick={onClose}>
              {t("Fermer")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}

const pad = (n: number) => String(n).padStart(2, "0");
/** Date du début d'une série, pour décrire sa répétition. */
const isoDay = (t: number) => {
  const d = new Date(t);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};

export function EventSheet({ draft, calendars, accounts, onClose, onEdit, onSaved, onError }: Props) {
  const occurrence = draft.occurrence;
  /** La série porte la réponse, le rappel et la répétition d'une occurrence qui en fait partie. */
  const event = (occurrence?.series ?? occurrence?.event) as CalEvent;
  const shown = occurrence?.event ?? event;
  const emails = accounts.map((a) => a.email);
  const invited = isInvited(event, emails);
  /** Comptes invités : chacun répond pour lui ; le premier par défaut. */
  const mine = invitedAccounts(event, accounts);
  const [asAccount, setAsAccount] = useState(mine[0]?.id ?? "");
  const me = mine.find((a) => a.id === asAccount) ?? mine[0];
  const lines = event.attendees ?? [];
  const answered = me ? answerOf(lines.find((a) => lower(a.email) === lower(me.email))) : null;
  const [send, setSend] = useState(true);
  const [reminder, setReminder] = useState<number>(event.reminder ?? -1);
  const [busy, setBusy] = useState(false);
  const [source, setSource] = useState(false);
  const when = rangeLabel(draft.start, draft.end, draft.allDay);
  const meeting = meetingLink(shown);
  const description = cleanDescription(shown.description);
  const calendar = calendars.find((c) => c.url === event.calendar);
  const accountName = (id: string) => (id === "local" ? t("Sur cet ordinateur") : (accounts.find((a) => a.id === id)?.name ?? id));
  const repeats = event.rrule ? describeRecurrence(event.rrule, isoDay(instant(event.start))) : "";

  const run = async (work: () => Promise<string>) => {
    setBusy(true);
    try {
      onSaved(await work());
    } catch (e) {
      onError(t("Rendez-vous non modifié"), e);
    } finally {
      setBusy(false);
    }
  };

  const answer = (choice: InvitationAnswer) =>
    run(async () => {
      await api.answerEvent(event.id, choice, send, when, me?.id);
      if (choice === "declined") return send ? t("Refus envoyé à l'organisateur") : t("Invitation refusée");
      return send ? t("Réponse envoyée, agenda mis à jour") : t("Agenda mis à jour");
    });

  const said: Record<InvitationAnswer, string> = {
    accepted: t("Vous avez accepté cette invitation."),
    tentative: t("Vous avez répondu « Peut-être » à cette invitation."),
    declined: t("Vous avez refusé cette invitation."),
  };
  const answers: [InvitationAnswer, string][] = [
    ["accepted", t("Oui")],
    ["tentative", t("Peut-être")],
    ["declined", t("Non")],
  ];

  const counts = lines.reduce(
    (n, a) => {
      const k = a.partstat === "ACCEPTED" ? 0 : a.partstat === "TENTATIVE" ? 1 : a.partstat === "DECLINED" ? 2 : 3;
      n[k] += 1;
      return n;
    },
    [0, 0, 0, 0],
  );
  const tally = [
    counts[0] && t("{n} oui", { n: counts[0] }),
    counts[1] && t("{n} peut-être", { n: counts[1] }),
    counts[2] && t("{n} non", { n: counts[2] }),
    counts[3] && t("{n} en attente", { n: counts[3] }),
  ]
    .filter(Boolean)
    .join(" · ");
  const organizerName = event.organizer || event.organizerEmail || "";
  const reminderChanged = reminder !== (event.reminder ?? -1);

  return (
    <Dialog open onOpenChange={(_, d) => !d.open && !busy && onClose()}>
      <DialogSurface className="invitation-dialog" style={{ ["--event-color" as string]: calendar?.color || undefined }}>
        <DialogBody>
          <DialogTitle
            className="sheet-title"
            action={<Button appearance="subtle" aria-label={t("Fermer")} icon={<Dismiss20Regular />} onClick={onClose} disabled={busy} />}
          >
            <span className="sheet-title-text">{shown.summary || t("(sans titre)")}</span>
            <span className="sheet-when">
              <Clock20Regular aria-hidden />
              {when}
            </span>
            {calendar && (
              <span className="sheet-calendar">
                <span className="sheet-dot" aria-hidden />
                {calendar.account === "local" ? calendar.name : `${calendar.name} — ${accountName(calendar.account)}`}
              </span>
            )}
          </DialogTitle>
          <DialogContent className="sheet">
            {invited && (
              // Participation en premier : c'est ce qu'on vient faire.
              <section className={answered ? `sheet-rsvp answered-${answered}` : "sheet-rsvp"} aria-label={t("Votre réponse")}>
                <div className="sheet-rsvp-status">{answered ? said[answered] : t("En attente de votre réponse")}</div>
                <AnswerAs
                  addresses={mine.map((a) => ({ account: a.id, email: a.email }))}
                  value={me?.id ?? ""}
                  onChange={setAsAccount}
                  disabled={busy}
                />
                <div className="sheet-rsvp-row">
                  <div className="invitation-choices" role="group" aria-label={t("Votre réponse")}>
                    {answers.map(([choice, label]) => (
                      <Button
                        key={choice}
                        shape="circular"
                        appearance={choice === answered ? "primary" : "outline"}
                        aria-pressed={choice === answered}
                        disabled={busy}
                        onClick={() => answer(choice)}
                      >
                        {label}
                      </Button>
                    ))}
                  </div>
                  {busy && <Spinner size="tiny" />}
                  <Checkbox
                    className="sheet-send"
                    label={t("Envoyer la réponse à l'organisateur")}
                    checked={send}
                    disabled={busy}
                    onChange={(_, d) => setSend(d.checked === true)}
                  />
                </div>
              </section>
            )}

            {meeting && (
              <Button className="sheet-meeting" appearance="primary" icon={<Video20Regular />} onClick={() => void api.openUrl(meeting.url)}>
                {t("Participer avec {service}", { service: meeting.service })}
              </Button>
            )}

            <dl className="sheet-facts">
              {shown.location && shown.location !== meeting?.url && (
                <>
                  <dt>
                    <Location20Regular aria-hidden />
                    {t("Lieu")}
                  </dt>
                  <dd>
                    <Linked text={shown.location} />
                  </dd>
                </>
              )}
              {repeats && (
                <>
                  <dt>
                    <ArrowRepeatAll20Regular aria-hidden />
                    {t("Répétition")}
                  </dt>
                  <dd>{repeats}</dd>
                </>
              )}
              {organizerName && (
                <>
                  <dt>
                    <Person20Regular aria-hidden />
                    {t("Organisateur")}
                  </dt>
                  <dd className="sheet-person">
                    <Avatar name={organizerName} size={24} color="colorful" />
                    <span>
                      {organizerName}
                      {event.organizerEmail && event.organizerEmail !== organizerName && <span className="muted"> · {event.organizerEmail}</span>}
                    </span>
                  </dd>
                </>
              )}
              <dt>
                <Alert20Regular aria-hidden />
                {t("Rappel")}
              </dt>
              <dd>
                <Select size="small" value={String(reminder)} onChange={(_, d) => setReminder(Number(d.value))} aria-label={t("Rappel")} disabled={busy}>
                  {!REMINDER_CHOICES.some((c) => c.value === reminder) && <option value={String(reminder)}>{reminderLabel(reminder)}</option>}
                  {REMINDER_CHOICES.map((c) => (
                    <option key={c.value} value={String(c.value)}>
                      {c.value < 0 ? t("Pas de rappel") : c.label}
                    </option>
                  ))}
                </Select>
              </dd>
            </dl>

            {lines.length > 0 && (
              <section className="sheet-section">
                <h3>
                  <People20Regular aria-hidden />
                  {t("Participants ({n})", { n: lines.length })}
                  {tally && <span className="muted"> · {tally}</span>}
                </h3>
                <GuestList attendees={lines} mine={mine.map((a) => ({ email: a.email, answer: null }))} />
              </section>
            )}

            {description && (
              <section className="sheet-section">
                <h3>
                  <TextDescription20Regular aria-hidden />
                  {t("Description")}
                </h3>
                <div className="invitation-description">
                  <Linked text={description} />
                </div>
              </section>
            )}
          </DialogContent>
          <DialogActions fluid className="event-actions">
            {!invited && <EventDelete draft={draft} busy={busy} setBusy={setBusy} onSaved={onSaved} onError={onError} />}
            <Button appearance="subtle" disabled={busy} onClick={() => setSource(true)}>
              {t("Source")}
            </Button>
            <span className="event-actions-spacer" />
            {reminderChanged && (
              <Button
                appearance={invited ? "primary" : "secondary"}
                disabled={busy}
                onClick={() =>
                  run(async () => {
                    await api.saveEvent({ ...event, reminder: reminder < 0 ? null : reminder }, event.calendar);
                    return t("Rappel enregistré");
                  })
                }
              >
                {t("Enregistrer le rappel")}
              </Button>
            )}
            {invited ? (
              <Button disabled={busy} onClick={onClose}>
                {t("Fermer")}
              </Button>
            ) : (
              <Button appearance="primary" icon={<Edit20Regular />} disabled={busy} onClick={onEdit}>
                {t("Modifier")}
              </Button>
            )}
          </DialogActions>
        </DialogBody>
        {source && <SourceDialog id={event.id} onClose={() => setSource(false)} />}
      </DialogSurface>
    </Dialog>
  );
}
