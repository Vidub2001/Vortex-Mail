// Signature S/MIME d'un message reçu (lot D1) : lecture du résultat calculé
// par le backend (src-tauri/src/smime.rs) et textes affichés.
//
// Le nom et les adresses viennent du certificat, donc du message : ils sont
// affichés comme du texte, jamais interprétés.
import { t } from "./i18n";
import type { Message } from "./types";

/** Les quatre résultats, du meilleur au pire (RFC 8551, d'après MailCraft). */
export type Verdict = "verified" | "notVerified" | "changed" | "unchecked";

export interface SignatureInfo {
  verdict: Verdict;
  /** Pourquoi le résultat n'est pas « verified ». */
  why: string | null;
  name: string;
  addresses: string[];
  /** Empreinte SHA-256 du certificat (hexadécimal). */
  fingerprint: string;
  issuer: string;
  /** Validité du certificat, en secondes depuis 1970 (0 : inconnue). */
  notBefore: number;
  notAfter: number;
  /** Date de signature annoncée par le signataire (non vérifiée). */
  signedAt: number | null;
  /** La signature couvre tout le message. */
  whole: boolean;
  /** Certificat cherché dans une liste valable de son autorité (lot G2c). */
  revocationChecked: boolean;
}

const VERDICTS: Verdict[] = ["verified", "notVerified", "changed", "unchecked"];

/** Signature du message, ou `null` s'il n'est pas signé (ou pas encore examiné). */
export function signatureOf(m: Pick<Message, "signature">): SignatureInfo | null {
  if (!m.signature || m.signature === "?") return null;
  try {
    const s = JSON.parse(m.signature) as Partial<SignatureInfo>;
    if (!s || !VERDICTS.includes(s.verdict as Verdict)) return null;
    return {
      verdict: s.verdict as Verdict,
      why: typeof s.why === "string" ? s.why : null,
      name: typeof s.name === "string" ? s.name : "",
      addresses: Array.isArray(s.addresses) ? s.addresses.filter((a) => typeof a === "string") : [],
      fingerprint: typeof s.fingerprint === "string" ? s.fingerprint : "",
      issuer: typeof s.issuer === "string" ? s.issuer : "",
      notBefore: typeof s.notBefore === "number" ? s.notBefore : 0,
      notAfter: typeof s.notAfter === "number" ? s.notAfter : 0,
      signedAt: typeof s.signedAt === "number" ? s.signedAt : null,
      whole: s.whole !== false,
      revocationChecked: s.revocationChecked === true,
    };
  } catch {
    return null;
  }
}

/** Le message attend son premier examen (corps gardé avant le lot D1). */
export function notExamined(m: Pick<Message, "signature">): boolean {
  return m.signature === "?";
}

/** Signataire tel qu'affiché : nom et adresse du certificat. */
export function signerLabel(s: SignatureInfo): string {
  const address = s.addresses[0] ?? "";
  if (s.name && address && s.name !== address) return `${s.name} <${address}>`;
  return s.name || address || t("signataire inconnu");
}

/** Phrase principale de la barre de signature. */
export function verdictText(s: SignatureInfo): string {
  switch (s.verdict) {
    case "verified":
      return t("Signé par {who} : signature vérifiée.", { who: signerLabel(s) });
    case "notVerified":
      return t("Signé par {who}, mais le signataire n'est pas vérifié.", { who: signerLabel(s) });
    case "changed":
      return t("Ce message a été modifié après sa signature.");
    default:
      return t("La signature de ce message ne peut pas être vérifiée.");
  }
}

/** Raison d'un résultat autre que « verified » ; vide si inconnue. */
export function whyText(why: string | null): string {
  switch (why) {
    case "unknownIssuer":
      return t("Son certificat n'a pas été délivré par une autorité de confiance pour le courrier.");
    case "selfSigned":
      return t("Son certificat est signé par son propre titulaire, qui ne fait pas partie des autorités de confiance.");
    case "expired":
      return t("Son certificat, ou celui d'une autorité dont il dépend, est expiré ou pas encore valide.");
    case "notForEmail":
      return t("Son certificat n'est pas prévu pour signer du courrier.");
    case "addressMismatch":
      return t("Son certificat est celui d'une autre adresse que celle de l'expéditeur.");
    case "constrained":
      return t("Une autorité de la chaîne de certification n'autorise pas cet usage du certificat.");
    case "revoked":
      return t("Le certificat a été révoqué par son autorité : il ne doit plus servir.");
    case "noAuthorities":
      return t("Aucune autorité de confiance pour le courrier n'a été trouvée sur cet ordinateur.");
    case "authoritiesNotRead":
      return t("Les autorités de confiance n'ont pas encore été lues.");
    case "partlySigned":
      return t("Seule une partie du message est signée : le reste a pu être ajouté ensuite.");
    case "altered":
      return t("Le contenu reçu n'est pas celui qui a été signé.");
    case "weakAlgorithm":
      return t("Elle utilise un algorithme qui n'est plus sûr (MD5, SHA-1 ou clé RSA trop courte).");
    case "unknownAlgorithm":
      return t("Elle utilise un algorithme inconnu.");
    case "noPublicKey":
      return t("Le certificat du signataire n'est pas joint au message.");
    case "unreadable":
      return t("La signature est illisible.");
    case "tooMuch":
      return t("Trop de certificats à examiner : la vérification a été arrêtée.");
    default:
      return "";
  }
}

/** Ton de la barre : vert, orange ou rouge. */
export function tone(s: SignatureInfo): "good" | "warning" | "danger" {
  if (s.verdict === "verified") return "good";
  return s.verdict === "changed" ? "danger" : "warning";
}

/** Empreinte lisible : octets séparés par des deux-points, en majuscules. */
export function fingerprintText(hex: string): string {
  return (hex.replace(/[^0-9a-fA-F]/g, "").toUpperCase().match(/../g) ?? []).join(":");
}
