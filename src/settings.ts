// Réglages de « Paramètres › Courrier », dans le cache chiffré (signatures :
// audit S8, voir `secure-store.ts`), pour la fenêtre principale. Depuis le lot 9c, chaque fenêtre a son stockage : une fenêtre
// de message ou de rédaction reçoit les réglages à son ouverture
// (`window_bootstrap`) et ne voit pas ceux changés ensuite.
import { useEffect, useState } from "react";
import { secureStore } from "./secure-store";

export interface MailSettings {
  /** Signature (texte) par compte. */
  signatures: Record<string, string>;
  /** Signature aussi dans les réponses et transferts. */
  signatureOnReplies: boolean;
  /** Fréquence de la synchronisation automatique, en minutes. */
  syncMinutes: number;
  /** Avertir à l'envoi quand le message parle d'une pièce jointe absente. */
  attachmentReminder: boolean;
  /** Mots qui annoncent une pièce jointe, séparés par des virgules. */
  attachmentWords: string;
}

const KEY = "vortex.mail";
const CHANGED = "vortex-settings";

export const SYNC_CHOICES = [1, 2, 5, 10, 15, 30];

/** Mots par défaut (français, anglais, espagnol). */
export const DEFAULT_ATTACHMENT_WORDS =
  "pièce jointe, pièces jointes, PJ, P.J., ci-joint, ci-jointe, ci-joints, ci-jointes, joint, jointe, joints, jointes, " +
  "en annexe, attaché, attachée, attached, attachment, attachments, enclosed, adjunto, adjunta, adjuntos, archivo adjunto";

const DEFAULTS: MailSettings = {
  signatures: {},
  signatureOnReplies: true,
  syncMinutes: 2,
  attachmentReminder: true,
  attachmentWords: DEFAULT_ATTACHMENT_WORDS,
};

export function loadSettings(): MailSettings {
  try {
    return { ...DEFAULTS, ...(JSON.parse(secureStore.getItem(KEY) ?? "{}") as Partial<MailSettings>) };
  } catch {
    return { ...DEFAULTS };
  }
}

export function saveSettings(settings: MailSettings) {
  try {
    secureStore.setItem(KEY, JSON.stringify(settings));
  } catch {
    // stockage indisponible : réglage valable jusqu'à la fermeture
  }
  window.dispatchEvent(new Event(CHANGED));
}

/** Réglages à jour, y compris quand une autre fenêtre les modifie. */
export function useMailSettings(): MailSettings {
  const [settings, setSettings] = useState(loadSettings);
  useEffect(() => {
    const reload = () => setSettings(loadSettings());
    window.addEventListener(CHANGED, reload);
    window.addEventListener("storage", reload);
    return () => {
      window.removeEventListener(CHANGED, reload);
      window.removeEventListener("storage", reload);
    };
  }, []);
  return settings;
}

const escapeHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** Contenu HTML de la signature d'un compte (vide s'il n'en a pas). */
export function signatureContent(accountId: string): string {
  const text = (loadSettings().signatures[accountId] ?? "").trimEnd();
  return text ? text.split("\n").map((l) => escapeHtml(l) || "<br>").join("<br>") : "";
}

/** Bloc de signature à placer dans un nouveau message (repérable pour changer de compte). */
export function signatureBlock(accountId: string, reply: boolean): string {
  const s = loadSettings();
  if (reply && !s.signatureOnReplies) return "";
  const content = signatureContent(accountId);
  return content ? `<p><br></p><div data-signature="1">${content}</div>` : "";
}

// ------------------------------------------------------------------ pièce jointe oubliée

const escapeRegExp = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * Premier mot annonçant une pièce jointe dans le texte (sujet + corps sans la
 * citation ni la signature), ou null. Mots entiers, sans tenir compte de la casse.
 */
export function attachmentMention(text: string, words = loadSettings().attachmentWords): string | null {
  const list = words
    .split(",")
    .map((w) => w.trim())
    .filter(Boolean)
    // Les expressions longues d'abord : « pièce jointe » avant « jointe ».
    .sort((a, b) => b.length - a.length);
  if (list.length === 0) return null;
  // Sans « lookbehind » (anciennes versions de WebKitGTK) : le caractère précédent est capturé à part.
  const re = new RegExp(`(^|[^\\p{L}\\p{N}])(${list.map(escapeRegExp).join("|")})(?![\\p{L}\\p{N}])`, "iu");
  return re.exec(text)?.[2] ?? null;
}

/** Texte écrit par l'utilisateur : sans le message cité (réponse, transfert) ni la signature. */
export function ownText(editor: HTMLElement, plain: boolean): string {
  if (plain) {
    const lines: string[] = [];
    for (const line of editor.innerText.split("\n")) {
      // Début de la citation : « De : … » (en-tête du message d'origine) ou ligne de séparation.
      if (/^\s*(De|From)\s*:/i.test(line) || /^\s*(-{3,}|_{5,})/.test(line)) break;
      // Ligne d'attribution d'une réponse : « Le …, X a écrit : » (et ses traductions).
      if (/(a écrit|wrote|escribió)\s*:\s*$/i.test(line)) break;
      if (/^\s*>/.test(line)) continue;
      lines.push(line);
    }
    return lines.join("\n");
  }
  const copy = editor.cloneNode(true) as HTMLElement;
  // Tout ce qui suit le séparateur placé avant l'en-tête cité.
  const header = copy.querySelector(".quote-header");
  if (header) {
    let node: ChildNode | null = header.previousSibling?.nodeName === "HR" ? header.previousSibling : header;
    while (node) {
      const next: ChildNode | null = node.nextSibling;
      node.remove();
      node = next;
    }
  }
  copy.querySelectorAll("blockquote, .quoted-body, .quote-attribution, [data-signature]").forEach((n) => n.remove());
  return copy.innerText || copy.textContent || "";
}
