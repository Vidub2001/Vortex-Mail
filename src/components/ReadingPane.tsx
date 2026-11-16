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
  Menu,
  MenuItemRadio,
  MenuList,
  MenuPopover,
  MenuTrigger,
  Spinner,
} from "@fluentui/react-components";
import {
  ArrowDownload16Regular,
  CalendarLtr20Regular,
  Eye20Regular,
  ImageOff20Regular,
  LockClosed20Regular,
  MailRead20Regular,
  PersonWarning20Regular,
  Warning24Regular,
} from "@fluentui/react-icons";
import { cloneElement, isValidElement, useEffect, useRef, useState, type ReactNode } from "react";
import { mailtoDraft } from "../compose";
import { api } from "../api";
import { fileSize, hasRemoteImages, longDate, wrapBody, wrapText } from "../format";
import { saveGeneral, useGeneral, type BodyMode } from "../general";
import { linkRequest, prepareLinks } from "../links";
import { onLinkRequest, openComposeWindow } from "../windows";
import type { Attendee, AttachmentInfo, CalEvent, InvitationAnswer, Message, MyAddress } from "../types";
import { FileTypeIcon } from "./FileTypeIcon";
import { SignatureBar } from "./SignatureBar";
import { notExamined, signatureOf } from "../signature";
import { canAnswer, eventWhen, invitationOf, planInvitation, type InvitationKind } from "../invitation";
import { AnswerAs, GuestList } from "./Guests";
import { t } from "../i18n";

interface Props {
  message: Message | null;
  loading: boolean;
  selectionCount: number;
  /** Barre d'actions affichée en tête du volet. */
  toolbar: ReactNode;
  /** Thème sombre : le corps du message suit (fond sombre, texte clair). */
  dark: boolean;
  onOpenAttachment: (message: Message, a: AttachmentInfo) => Promise<void>;
  onSaveAttachments: (message: Message, list: AttachmentInfo[]) => Promise<void>;
  /** « Afficher les images » : téléchargées, intégrées et mémorisées pour ce message. */
  onShowImages: (message: Message) => Promise<void>;
  /** Invitation (.ics) : l'appliquer à l'agenda (ajout, mise à jour ou retrait). */
  onAddInvitation?: (message: Message, a: AttachmentInfo) => Promise<void>;
  /**
   * Réponse à une invitation (lot H1) ; `send` : envoyée à l'organisateur ;
   * `asAccount` : autre compte invité qui répond (lot H1b).
   */
  onAnswerInvitation?: (message: Message, a: AttachmentInfo, answer: InvitationAnswer, send: boolean, asAccount?: string) => Promise<void>;
  /**
   * Signature S/MIME examinée de nouveau (lot D1). `quiet` : premier examen
   * d'un message gardé avant le lot D1, sans message d'erreur.
   */
  onCheckSignature?: (message: Message, quiet: boolean) => Promise<void>;
}

/** Texte de la barre d'invitation, et libellé du bouton s'il y a une action. */
function invitationText(kind: InvitationKind): { text: string; action?: string } {
  switch (kind) {
    case "add":
      return { text: t("Ce message contient une invitation."), action: t("Ajouter à l'agenda") };
    case "update":
      return { text: t("Cette invitation modifie un rendez-vous de votre agenda."), action: t("Mettre à jour l'agenda") };
    case "current":
      return { text: t("Ce rendez-vous est déjà dans votre agenda.") };
    case "outdated":
      return { text: t("Votre agenda contient une version plus récente de ce rendez-vous.") };
    case "cancel":
      return { text: t("Ce message annule un rendez-vous de votre agenda."), action: t("Retirer de l'agenda") };
    case "gone":
      return { text: t("Ce message annule un rendez-vous absent de votre agenda.") };
    default:
      return { text: t("Ce message contient une réponse à une invitation.") };
  }
}

/**
 * Barre d'invitation : ce que l'invitation propose dépend de sa méthode
 * (`REQUEST`, `CANCEL`, `REPLY`…) et de l'agenda (relecture du 12 novembre,
 * n° 7). Lecture impossible : l'ajout reste proposé (l'erreur s'affichera).
 */
function InvitationBar({
  message,
  ics,
  onApply,
  onAnswer,
}: {
  message: Message;
  ics: AttachmentInfo;
  onApply: (m: Message, a: AttachmentInfo) => Promise<void>;
  onAnswer?: (m: Message, a: AttachmentInfo, answer: InvitationAnswer, send: boolean, asAccount?: string) => Promise<void>;
}) {
  const [kind, setKind] = useState<InvitationKind | null>(null);
  /** Demande (`REQUEST`) à laquelle répondre (lot H1). */
  const [answerable, setAnswerable] = useState(false);
  /** Comptes qui peuvent répondre, avec la réponse déjà donnée à cette version. */
  const [addresses, setAddresses] = useState<MyAddress[]>([]);
  /** Compte qui répond : celui qui a reçu l'invitation, sauf choix (lot H1b). */
  const [asAccount, setAsAccount] = useState("");
  /** Rendez-vous de l'invitation, pour la carte. */
  const [event, setEvent] = useState<CalEvent | null>(null);
  /** Invités : ceux du rendez-vous de l'agenda s'il y est (tenus à jour par
   * l'organisateur), sinon ceux de l'invitation. */
  const [guests, setGuests] = useState<Attendee[]>([]);
  /** Envoyer la réponse à l'organisateur : choisi à chaque fois, coché par défaut. */
  const [send, setSend] = useState(true);
  const [round, setRound] = useState(0);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let alive = true;
    setKind(null);
    planInvitation(message, ics)
      .then((p) => {
        if (!alive) return;
        setKind(p.kind);
        const mine = p.invitation.addresses ?? [];
        setAnswerable(canAnswer(p) || mine.some((a) => a.answer));
        setAddresses(mine);
        setAsAccount((chosen) => (mine.some((a) => a.account === chosen) ? chosen : (mine[0]?.account ?? "")));
        setEvent(p.event);
        setGuests(p.existing?.attendees?.length ? p.existing.attendees : (p.event.attendees ?? []));
      })
      .catch(() => alive && setKind("add"));
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [message.id, ics.index, round]);
  if (!kind) return null;
  const { text, action } = invitationText(kind);
  const run = async (work: () => Promise<void>) => {
    setBusy(true);
    try {
      await work();
    } finally {
      setBusy(false);
      setRound((r) => r + 1);
    }
  };
  if (answerable && onAnswer && event) {
    const answers: [InvitationAnswer, string][] = [
      ["accepted", t("Oui")],
      ["tentative", t("Peut-être")],
      ["declined", t("Non")],
    ];
    const said: Record<InvitationAnswer, string> = {
      accepted: t("Vous avez accepté cette invitation."),
      tentative: t("Vous avez répondu « Peut-être » à cette invitation."),
      declined: t("Vous avez refusé cette invitation."),
    };
    const when = eventWhen(event);
    const me = addresses.find((a) => a.account === asAccount) ?? addresses[0];
    const answered = me?.answer ?? null;
    // Carte d'invitation, comme celles de Gmail et de Thunderbird : le
    // rendez-vous, l'état de la réponse, puis Oui, Peut-être, Non groupés ; la
    // réponse déjà donnée est mise en avant, les autres restent pour changer d'avis.
    return (
      <section className="invitation-card" aria-label={t("Invitation")}>
        <div className="invitation-head">
          <CalendarLtr20Regular />
          <div>
            <div className="invitation-title">{event.summary || t("(sans titre)")}</div>
            {when && <div className="invitation-when">{when}</div>}
          </div>
        </div>
        <dl className="invitation-details">
          {event.location && (
            <>
              <dt>{t("Lieu")}</dt>
              <dd>{event.location}</dd>
            </>
          )}
          {event.organizer && (
            <>
              <dt>{t("Organisateur")}</dt>
              <dd>{event.organizer}</dd>
            </>
          )}
          {guests.length > 0 && (
            <>
              <dt>{t("Invités")}</dt>
              <dd>
                <GuestList attendees={guests} mine={addresses} />
              </dd>
            </>
          )}
        </dl>
        <div className={answered ? `invitation-status answered-${answered}` : "invitation-status"}>
          {answered ? said[answered] : t("En attente de votre réponse")}
          {!answered && kind === "update" && <span className="invitation-note"> · {t("Cette invitation modifie un rendez-vous de votre agenda.")}</span>}
        </div>
        <AnswerAs addresses={addresses} value={me?.account ?? ""} onChange={setAsAccount} disabled={busy} />
        <div className="invitation-actions">
          <span className="invitation-ask">{t("Participer ?")}</span>
          <div className="invitation-choices" role="group" aria-label={t("Votre réponse")}>
            {answers.map(([answer, label]) => (
              <Button
                key={answer}
                size="small"
                shape="circular"
                appearance={answer === answered ? "primary" : "outline"}
                aria-pressed={answer === answered}
                disabled={busy}
                onClick={() => run(() => onAnswer(message, ics, answer, send, me && me.account !== message.accountId ? me.account : undefined))}
              >
                {label}
              </Button>
            ))}
          </div>
          {busy && <Spinner size="extra-tiny" />}
        </div>
        <Checkbox label={t("Envoyer la réponse à l'organisateur")} checked={send} disabled={busy} onChange={(_, d) => setSend(d.checked === true)} />
      </section>
    );
  }
  return (
    <div className="invitation-bar">
      <CalendarLtr20Regular />
      <span>{text}</span>
      {action && (
        <Button
          size="small"
          appearance="primary"
          disabled={busy}
          icon={busy ? <Spinner size="extra-tiny" /> : undefined}
          onClick={async () => {
            setBusy(true);
            try {
              await onApply(message, ics);
            } finally {
              setBusy(false);
              setRound((r) => r + 1);
            }
          }}
        >
          {action}
        </Button>
      )}
    </div>
  );
}

export function ReadingPane({
  message,
  loading,
  selectionCount,
  toolbar,
  dark,
  onOpenAttachment,
  onSaveAttachments,
  onShowImages,
  onAddInvitation,
  onAnswerInvitation,
  onCheckSignature,
}: Props) {
  const { bodyMode } = useGeneral(); // redessine aussi si le format de date change
  const frame = useRef<HTMLIFrameElement>(null);
  /** Lien trompeur en attente de confirmation. */
  const [suspect, setSuspect] = useState<{ url: string; shown: string; real: string } | null>(null);
  const accountRef = useRef<string | null>(null);
  accountRef.current = message?.accountId ?? null;

  // Clic sur un lien réécrit (lien trompeur, mailto:) : renvoyé par le
  // backend à cette fenêtre (voir src/links.ts).
  useEffect(
    () =>
      onLinkRequest((raw) => {
        const action = linkRequest(raw);
        if (action.kind === "confirm") setSuspect(action);
        else if (action.kind === "mailto" && accountRef.current) {
          void openComposeWindow(mailtoDraft(accountRef.current, action));
        }
      }),
    [],
  );

  // Message gardé avant le lot D1 : sa signature est examinée une fois, en
  // arrière-plan (seul un message annoncé signé est téléchargé de nouveau).
  const examine = message && onCheckSignature && notExamined(message) ? message : null;
  useEffect(() => {
    if (examine && onCheckSignature) void onCheckSignature(examine, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [examine?.id]);

  /** Corps chargé : liens réécrits pour s'ouvrir hors du volet (lot 4b). */
  const onBodyLoad = () => {
    const doc = frame.current?.contentDocument;
    if (doc) prepareLinks(doc);
  };

  const BODY_MODES: { value: BodyMode; label: string }[] = [
    { value: "original", label: t("Original") },
    { value: "simple", label: t("HTML simplifié") },
    { value: "text", label: t("Texte brut") },
  ];
  /** Pièces en cours de téléchargement (clé : index, ou -1 pour « tout »). */
  const [busy, setBusy] = useState<number[]>([]);
  const run = async (key: number, job: () => Promise<void>) => {
    setBusy((b) => [...b, key]);
    try {
      await job();
    } finally {
      setBusy((b) => b.filter((x) => x !== key));
    }
  };

  const empty = (content: ReactNode) => (
    <section className="pane reading" aria-label={t("Volet de lecture")}>
      {toolbar}
      <div className="reading-empty">{content}</div>
    </section>
  );

  if (selectionCount > 1) {
    return empty(
      <>
        <MailRead20Regular />
        <p>{t("{n} éléments sélectionnés", { n: selectionCount })}</p>
        <p className="muted">{t("Glissez-les vers un dossier, de n'importe quel compte.")}</p>
      </>,
    );
  }
  if (loading) return empty(<Spinner size="small" />);
  if (!message) {
    return empty(
      <>
        <MailRead20Regular />
        <p>{t("Sélectionnez un élément à lire")}</p>
        <p className="muted">{t("Rien n'est sélectionné")}</p>
      </>,
    );
  }

  return (
    <section className="pane reading" aria-label={t("Volet de lecture")}>
      <div className="reading-content">
      {/* Objet à gauche, actions à droite sur la même ligne. */}
      <div className="reading-head">
        <h1 className="reading-subject">{message.subject || t("(sans objet)")}</h1>
        {isValidElement<{ compact?: boolean }>(toolbar) ? cloneElement(toolbar, { compact: true }) : toolbar}
      </div>

      <div className="reading-sender">
        <Avatar name={message.fromName} size={40} color="colorful" />
        <div className="reading-sender-info">
          <div>
            <strong>{message.fromName}</strong>{" "}
            <span className="muted">&lt;{message.fromEmail}&gt;</span>
          </div>
          <div className="muted">{t("À : {to}", { to: message.to })}</div>
        </div>
        <div className="reading-actions">
          <div className="muted reading-date">{longDate(message.date)}</div>
          <Menu
            checkedValues={{ bodyMode: [bodyMode] }}
            onCheckedValueChange={(_, d) => saveGeneral({ bodyMode: d.checkedItems[0] as BodyMode })}
          >
            <MenuTrigger disableButtonEnhancement>
              <Button size="small" appearance="subtle" icon={<Eye20Regular />} title={t("Affichage du message")}>
                {BODY_MODES.find((m) => m.value === bodyMode)?.label}
              </Button>
            </MenuTrigger>
            <MenuPopover>
              <MenuList>
                {BODY_MODES.map((m) => (
                  <MenuItemRadio key={m.value} name="bodyMode" value={m.value}>
                    {m.label}
                  </MenuItemRadio>
                ))}
              </MenuList>
            </MenuPopover>
          </Menu>
        </div>
      </div>

      {message.senderCheck && (
        <div className="signature-bar signature-warning" role="alert">
          <div className="signature-line">
            <PersonWarning20Regular />
            <span className="signature-text">
              {t(
                "Expéditeur non vérifié : le serveur de réception n'a pas pu confirmer que ce message vient de {domain}. Méfiez-vous des liens, des pièces jointes et des demandes de paiement.",
                { domain: message.fromEmail.split("@").pop() || message.fromEmail },
              )}
              {` (${message.senderCheck === "dmarc" ? "DMARC" : "SPF"})`}
            </span>
          </div>
        </div>
      )}

      {message.encryption && (
        <div
          className={`signature-bar ${message.encryption === "ok" ? "encryption-ok" : "signature-warning"}`}
          role="status"
        >
          <div className="signature-line">
            <LockClosed20Regular />
            <span className="signature-text">
              {message.encryption === "ok"
                ? t("Message chiffré, lu avec votre certificat. Son contenu n'est pas gardé en clair sur cet ordinateur.")
                : message.encryption}
            </span>
          </div>
        </div>
      )}

      {(() => {
        const signature = signatureOf(message);
        return signature && onCheckSignature ? (
          <SignatureBar key={message.id} signature={signature} onCheck={() => onCheckSignature(message, false)} />
        ) : null;
      })()}

      {(() => {
        const ics = onAddInvitation ? invitationOf(message) : undefined;
        return ics && onAddInvitation ? <InvitationBar key={`${message.id}|${ics.index}`} message={message} ics={ics} onApply={onAddInvitation} onAnswer={onAnswerInvitation} /> : null;
      })()}

      {message.attachments.length > 0 && (
        <div className="attachment-list">
          {message.attachments.map((a) => (
            <div key={a.index} className="attachment-card">
              <button
                type="button"
                className="attachment-open"
                title={t("Ouvrir {name}", { name: a.name })}
                disabled={busy.includes(a.index)}
                onClick={() => run(a.index, () => onOpenAttachment(message, a))}
              >
                {busy.includes(a.index) ? <Spinner size="extra-tiny" /> : <FileTypeIcon name={a.name} contentType={a.contentType} />}
                <span className="attachment-text">
                  <span className="attachment-name">{a.name}</span>
                  <span className="attachment-size">{fileSize(a.size)}</span>
                </span>
              </button>
              <Button
                size="small"
                appearance="subtle"
                icon={<ArrowDownload16Regular />}
                title={t("Télécharger")}
                aria-label={t("Télécharger {name}", { name: a.name })}
                disabled={busy.includes(a.index)}
                onClick={() => run(a.index, () => onSaveAttachments(message, [a]))}
              />
            </div>
          ))}
          {message.attachments.length > 1 && (
            <Button
              size="small"
              appearance="transparent"
              icon={busy.includes(-1) ? <Spinner size="extra-tiny" /> : <ArrowDownload16Regular />}
              disabled={busy.includes(-1)}
              onClick={() => run(-1, () => onSaveAttachments(message, message.attachments))}
            >
              {t("Tout télécharger ({n})", { n: message.attachments.length })}
            </Button>
          )}
        </div>
      )}

      {bodyMode === "original" && !message.imagesAllowed && !message.encryption && hasRemoteImages(message.bodyHtml) && (
        <div className="images-banner">
          <ImageOff20Regular />
          <span>{t("Pour protéger votre vie privée, les images distantes de ce message ne sont pas affichées.")}</span>
          <Button
            size="small"
            appearance="secondary"
            icon={busy.includes(-2) ? <Spinner size="extra-tiny" /> : undefined}
            disabled={busy.includes(-2)}
            onClick={() => run(-2, () => onShowImages(message))}
          >
            {busy.includes(-2) ? t("Téléchargement…") : t("Afficher les images")}
          </Button>
        </div>
      )}

      {/*
        Aucun script, aucun formulaire, aucune fenêtre surgissante.
        allow-same-origin : seulement pour que cette fenêtre réécrive les
        liens (sans allow-scripts, le message n'en tire rien) ; les liens en
        target="_blank" deviennent des demandes de fenêtre, toutes refusées
        par le backend (lien ouvert dans le navigateur externe).
      */}
      <iframe
        ref={frame}
        key={`${message.id}-${message.imagesAllowed}-${dark}-${bodyMode}`}
        className="reading-body"
        title={t("Contenu du message")}
        sandbox="allow-same-origin allow-popups"
        srcDoc={/* nosemgrep: vortex-raw-html-sink -- corps du mail dans l'iframe sans script : texte échappé, HTML simplifié par ammonia, ou Original sans meta, link ni base, comme Thunderbird */
          bodyMode === "text"
            ? wrapText(message.bodyText, dark)
            : wrapBody(bodyMode === "simple" ? message.bodySimple : message.bodyHtml, message.imagesAllowed, dark)
        }
        onLoad={onBodyLoad}
      />
      {suspect && (
        <Dialog open onOpenChange={(_, d) => !d.open && setSuspect(null)}>
          <DialogSurface className="small-dialog">
            <DialogBody>
              <DialogTitle action={null}>
                <span className="suspect-title">
                  <Warning24Regular />
                  {t("Lien trompeur")}
                </span>
              </DialogTitle>
              <DialogContent>
                <p className="dialog-text">
                  {t("Ce lien affiche « {shown} » mais mène à « {real} ». C'est souvent signe d'hameçonnage.", {
                    shown: suspect.shown,
                    real: suspect.real,
                  })}
                </p>
                <p className="dialog-text link-target">{suspect.url}</p>
              </DialogContent>
              <DialogActions>
                <Button appearance="primary" onClick={() => setSuspect(null)}>
                  {t("Annuler")}
                </Button>
                <Button
                  onClick={() => {
                    void api.openUrl(suspect.url).catch(() => {});
                    setSuspect(null);
                  }}
                >
                  {t("Ouvrir quand même")}
                </Button>
              </DialogActions>
            </DialogBody>
          </DialogSurface>
        </Dialog>
      )}
      </div>
    </section>
  );
}
