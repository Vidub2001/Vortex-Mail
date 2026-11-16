// Paramètres techniques des fournisseurs connus : serveurs appliqués quand on
// saisit l'adresse, et domaines des comptes Outlook.com / O365 (connexion OAuth2).
import type { Security } from "./types";

const OUTLOOK_DOMAINS = ["outlook.com", "outlook.fr", "hotmail.com", "hotmail.fr", "live.com", "live.fr", "msn.com"];

/** Serveurs connus par domaine d'adresse. */
export const PRESETS: Record<string, { imap: [string, number, Security]; smtp: [string, number, Security] }> = {
  "mailo.com": { imap: ["mail.mailo.com", 993, "sslTls"], smtp: ["mail.mailo.com", 465, "sslTls"] },
  "gmail.com": { imap: ["imap.gmail.com", 993, "sslTls"], smtp: ["smtp.gmail.com", 465, "sslTls"] },
  "googlemail.com": { imap: ["imap.gmail.com", 993, "sslTls"], smtp: ["smtp.gmail.com", 465, "sslTls"] },
  ...Object.fromEntries(
    OUTLOOK_DOMAINS.map((d) => [
      d,
      { imap: ["outlook.office365.com", 993, "sslTls"], smtp: ["smtp-mail.outlook.com", 587, "startTls"] },
    ]),
  ),
};

/** Adresse Outlook.com (connexion OAuth2 obligatoire). */
export const isOutlookDomain = (domain: string) => /^(outlook|hotmail|live|msn)\./.test(domain);
