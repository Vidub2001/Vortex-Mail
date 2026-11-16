// Préparation des brouillons : nouveau message, réponse, réponse à tous, transfert.
import { longDate } from "./format";
import { t } from "./i18n";
import { signatureBlock } from "./settings";
import type { Account, AttachmentInput, Draft, DraftContent, Message } from "./types";

/** Pièce jointe d'un brouillon. */
export interface ComposeAttachment extends AttachmentInput {
  key: string;
  size: number;
}

export type ComposeMode = "new" | "reply" | "replyAll" | "forward" | "draft";

/** État initial du formulaire de rédaction. */
export interface ComposeInit {
  /** Clé React : un nouveau formulaire à chaque ouverture. */
  key: number;
  mode: ComposeMode;
  accountId: string;
  to: string;
  cc: string;
  subject: string;
  /** HTML initial de l'éditeur (citation du message d'origine). */
  html: string;
  inReplyTo: string | null;
  references: string | null;
  /** Message auquel on répond : marqué « répondu » après l'envoi. */
  answeredId: number | null;
  /** Pièces jointes reprises (transfert). */
  attachments: ComposeAttachment[];
  /** Destinataires en copie cachée (brouillon repris). */
  bcc?: string;
  /** Brouillon déjà enregistré dans « Brouillons » : remplacé à chaque enregistrement. */
  draftId?: number | null;
  /** Format imposé (brouillon repris) ; sinon le dernier choisi. */
  plainText?: boolean;
  /** Brouillon chiffré repris : « Chiffrer » enfoncé (lot D4). */
  encrypt?: boolean;
  /** « Signer » tel qu'il était (envoi échoué, lot E1) ; sinon le réglage du compte. */
  sign?: boolean;
  /**
   * Copie de secours retrouvée au démarrage : la fenêtre s'ouvre « modifiée »
   * et l'enregistre aussitôt (relecture complète n° 33).
   */
  recovered?: boolean;
  /** Erreur à afficher à l'ouverture (envoi en arrière-plan échoué, lot E1). */
  error?: string;
}

/** Reprise d'un brouillon enregistré. */
export function draftInit(d: DraftContent): ComposeInit {
  return {
    key: ++counter,
    mode: "draft",
    accountId: d.accountId,
    to: d.to,
    cc: d.cc,
    bcc: d.bcc,
    subject: d.subject,
    html: d.html || "<p><br></p>",
    inReplyTo: d.inReplyTo,
    references: d.references,
    answeredId: null,
    draftId: d.id,
    plainText: d.plainText,
    encrypt: d.encrypt,
    attachments: d.attachments.map((a, i) => ({
      key: `d${d.id}-${i}`,
      name: a.name,
      contentType: a.contentType,
      size: a.size,
      data: a.data,
    })),
  };
}

let counter = 0;

/** Style d'un bloc cité : écrit dans le HTML, pour que les destinataires voient aussi la barre. */
export const QUOTE_STYLE = "margin:0 0 0 4px;padding:2px 0 2px 12px;border-left:3px solid #9e9e9e;color:#555555";

/**
 * Découpe une liste saisie aux `,` et `;` placés hors des guillemets et des
 * chevrons : « "Dupont, Marie" <m@x.fr>; b@y.fr » donne deux parties
 * (relecture complète n° 12).
 */
export function splitAddressList(input: string): string[] {
  const parts: string[] = [];
  let current = "";
  let quoted = false;
  let angle = false;
  for (let i = 0; i < input.length; i++) {
    const c = input[i];
    if (c === "\\" && quoted && i + 1 < input.length) {
      current += c + input[++i];
      continue;
    }
    if (c === '"') quoted = !quoted;
    else if (c === "<" && !quoted) angle = true;
    else if (c === ">" && !quoted) angle = false;
    else if ((c === "," || c === ";") && !quoted && !angle) {
      parts.push(current);
      current = "";
      continue;
    }
    current += c;
  }
  parts.push(current);
  return parts;
}

/** Liste d'adresses saisie (« a@b.fr; "Dupont, Marie" <c@d.fr> ») → adresses seules. */
export function parseAddresses(input: string): string[] {
  const out: string[] = [];
  for (const part of splitAddressList(input)) {
    const angle = part.match(/<([^>]+)>\s*$/);
    const addr = (angle ? angle[1] : part).trim().replace(/^"|"$/g, "");
    if (addr && !out.some((x) => x.toLowerCase() === addr.toLowerCase())) out.push(addr);
  }
  return out;
}

/** « Nom <adresse> » pour le champ de saisie, nom entre guillemets s'il contient un séparateur. */
export function mailboxEntry(name: string, email: string): string {
  const n = name.trim();
  if (!n || n === email) return email;
  return /[",;<>()@:\\]/.test(n) ? `"${n.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}" <${email}>` : `${n} <${email}>`;
}

const escape = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");


/**
 * Préfixe « RE: » / « TR: », sans le doubler. Sans espace avant les deux
 * points, même en français : c'est la forme que les autres logiciels
 * reconnaissent pour regrouper la conversation (relecture complète n° 24).
 */
function prefixed(prefix: "RE" | "TR", subject: string): string {
  const already = prefix === "RE" ? /^\s*(re|aw|sv)\s*:/i : /^\s*(tr|fw|fwd|wg|rv)\s*:/i;
  if (already.test(subject)) return subject;
  return prefix === "RE" ? t("RE: {subject}", { subject }) : t("TR: {subject}", { subject });
}

/**
 * Réponse : « Le …, X a écrit : » puis le message d'origine dans un bloc cité
 * (barre à gauche), comme les clients de messagerie habituels. Transfert :
 * en-tête complet du message d'origine (De, Envoyé, À, Objet).
 */
function quote(m: Message, mode: "reply" | "replyAll" | "forward"): string {
  const from = m.fromName ? `${escape(m.fromName)} &lt;${escape(m.fromEmail)}&gt;` : escape(m.fromEmail);
  if (mode !== "forward") {
    return (
      `<p><br></p>` +
      signatureBlock(m.accountId, true) +
      `<p><br></p>` +
      `<div class="quote-attribution">${t("Le {date}, {from} a écrit :", { date: escape(longDate(m.date)), from })}</div>` +
      `<blockquote type="cite" style="${QUOTE_STYLE}">${m.bodySimple}</blockquote>`
    );
  }
  return (
    `<p><br></p>` +
    signatureBlock(m.accountId, true) +
    `<p><br></p>` +
    `<hr style="border:none;border-top:1px solid #e1e1e1;margin:8px 0">` +
    `<div class="quote-header"><b>${t("De :")}</b> ${from}<br>` +
    `<b>${t("Envoyé :")}</b> ${escape(longDate(m.date))}<br>` +
    `<b>${t("À :")}</b> ${escape(m.to)}<br>` +
    `<b>${t("Objet :")}</b> ${escape(m.subject)}</div><br>` +
    `<div class="quoted-body">${m.bodySimple}</div>`
  );
}

export function newDraft(accountId: string): ComposeInit {
  return {
    key: ++counter,
    mode: "new",
    accountId,
    to: "",
    cc: "",
    subject: "",
    html: "<p><br></p>" + signatureBlock(accountId, false),
    inReplyTo: null,
    references: null,
    answeredId: null,
    attachments: [],
  };
}

/** Nouveau message ouvert par un lien `mailto:` (destinataires, objet, texte). */
export function mailtoDraft(accountId: string, link: { to: string; cc: string; subject: string; body: string }): ComposeInit {
  const init = newDraft(accountId);
  const body = link.body
    ? `<p>${escape(link.body).replace(/\r?\n/g, "<br>")}</p>`
    : "<p><br></p>";
  return { ...init, to: link.to, cc: link.cc, subject: link.subject, html: body + signatureBlock(accountId, false) };
}

export function replyDraft(mode: "reply" | "replyAll" | "forward", m: Message, accounts: Account[]): ComposeInit {
  const own = (accounts.find((a) => a.id === m.accountId)?.email ?? "").toLowerCase();
  const notMe = (a: string) => a.toLowerCase() !== own;
  const sender = m.replyTo.length > 0 ? m.replyTo : [m.fromEmail];

  let to: string[] = [];
  let cc: string[] = [];
  if (mode === "reply") {
    to = sender;
  } else if (mode === "replyAll") {
    to = [...sender, ...m.toList].filter(notMe);
    cc = m.ccList.filter(notMe).filter((a) => !to.some((x) => x.toLowerCase() === a.toLowerCase()));
    if (to.length === 0) to = sender; // réponse à son propre message
  }
  const dedupe = (l: string[]) => l.filter((a, i) => l.findIndex((b) => b.toLowerCase() === a.toLowerCase()) === i);

  const isReply = mode !== "forward";
  const references = [m.references, m.messageId].filter((s) => s && s.trim()).join(" ").trim();
  return {
    key: ++counter,
    mode,
    accountId: m.accountId,
    to: dedupe(to).join(", "),
    cc: dedupe(cc).join(", "),
    subject: prefixed(isReply ? "RE" : "TR", m.subject),
    html: quote(m, mode),
    inReplyTo: isReply && m.messageId ? m.messageId : null,
    references: isReply && references ? references : null,
    // Message d'origine : marqué « répondu », ou « transféré » ($Forwarded) pour un transfert.
    answeredId: m.id,
    // Le transfert reprend les pièces jointes, pas la réponse.
    attachments: isReply
      ? []
      : m.attachments.map((a) => ({
          key: `m${m.id}-${a.index}`,
          name: a.name,
          contentType: a.contentType,
          size: a.size,
          messageId: m.id,
          index: a.index,
        })),
  };
}

/**
 * Message rendu à la rédaction après un envoi en arrière-plan échoué (lot E1) :
 * tel qu'il était, pièces jointes comprises, avec l'erreur.
 */
export function failedInit(
  accountId: string,
  d: Draft,
  attachments: AttachmentInput[],
  answeredId: number | null,
  draftId: number | null,
  error: string,
): ComposeInit {
  return {
    key: ++counter,
    mode: answeredId === null ? "new" : "reply",
    accountId,
    to: d.to.join("; "),
    cc: d.cc.join("; "),
    bcc: d.bcc.join("; "),
    subject: d.subject,
    html: d.html || "<p><br></p>",
    inReplyTo: d.inReplyTo,
    references: d.references,
    answeredId,
    draftId,
    plainText: d.plainText,
    encrypt: d.encrypt,
    sign: d.sign,
    attachments: attachments.map((a, i) => ({
      ...a,
      key: `e${counter}-${i}`,
      // Taille d'après le base64 (fichiers choisis) ; inconnue pour une pièce reprise.
      size: a.data ? Math.floor((a.data.length * 3) / 4) : 0,
    })),
    error,
  };
}
