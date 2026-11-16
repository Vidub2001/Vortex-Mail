// Miroir des types Rust de mail-core (sérialisés en camelCase).

export type FolderRole = "inbox" | "drafts" | "sent" | "archive" | "trash" | "junk" | "custom";

export interface Account {
  id: string;
  name: string;
  email: string;
  color: string;
}

export interface Folder {
  id: string;
  accountId: string;
  name: string;
  path: string;
  parentId: string | null;
  depth: number;
  role: FolderRole;
  unread: number;
  total: number;
  /** Abonné sur le serveur ; sinon visible seulement en vue « Comptes séparés ». */
  subscribed: boolean;
}

export interface Flags {
  seen: boolean;
  flagged: boolean;
  answered: boolean;
}

export interface MessageSummary {
  id: number;
  folderId: string;
  accountId: string;
  fromName: string;
  fromEmail: string;
  subject: string;
  preview: string;
  /** Timestamp Unix en secondes. */
  date: number;
  flags: Flags;
  hasAttachments: boolean;
  /** UID IMAP dans son dossier (0 : inconnu). */
  uid?: number;
}

export interface Message extends MessageSummary {
  to: string;
  bodyHtml: string;
  /** Adresses seules, pour Répondre / Répondre à tous. */
  toList: string[];
  ccList: string[];
  replyTo: string[];
  messageId: string;
  references: string;
  attachments: AttachmentInfo[];
  /** Images distantes autorisées (déjà intégrées au message en cache). */
  imagesAllowed: boolean;
  /** Corps nettoyé par le backend (liste blanche) : citation, brouillons. */
  bodySimple: string;
  /** Corps en texte brut (mode d'affichage « Texte »). */
  bodyText: string;
  /**
   * Signature S/MIME vérifiée par le backend, en JSON (lot D1, voir
   * `src/signature.ts`) ; vide si le message n'est pas signé, « ? » s'il n'a
   * pas encore été examiné.
   */
  signature?: string;
  /**
   * Message chiffré en S/MIME (lot D3a) : vide s'il ne l'est pas, « ok » une
   * fois déchiffré en mémoire, sinon la raison (traduite) pour laquelle il ne
   * l'a pas été.
   */
  encryption?: string;
  /**
   * Expéditeur que le serveur de réception n'a pas pu authentifier (lot G1) :
   * vide, « dmarc » (le domaine refuse ce message) ou « spf » (serveur d'envoi
   * non autorisé, sans signature valide).
   */
  senderCheck?: string;
}

/** Avancement du téléchargement pour la lecture hors connexion. */
export interface PrefetchReport {
  fetched: number;
  remaining: number;
  done: boolean;
}

export interface AttachmentInfo {
  /** Position dans le message, pour le téléchargement. */
  index: number;
  name: string;
  contentType: string;
  /** Octets. */
  size: number;
}

/** Pièce jointe envoyée : fichier choisi (base64) ou pièce d'un message (transfert). */
export interface AttachmentInput {
  name: string;
  contentType: string;
  data?: string;
  messageId?: number;
  index?: number;
}

/** Message rédigé, tel qu'envoyé au backend. */
export interface Draft {
  fromName: string;
  fromEmail: string;
  to: string[];
  cc: string[];
  bcc: string[];
  subject: string;
  html: string;
  inReplyTo: string | null;
  references: string | null;
  /** Texte brut : envoyé sans partie HTML. */
  plainText?: boolean;
  /** Clé d'envoi tirée une fois par fenêtre : même `Message-ID` à chaque essai. */
  sendKey?: string;
  /** Signé en S/MIME à l'envoi (lot D2). */
  sign?: boolean;
  /** Chiffré en S/MIME à l'envoi (lot D3b). */
  encrypt?: boolean;
}

/** Certificat d'un correspondant, gardé sur ses messages signés (lot D3b). */
export interface CorrespondentInfo {
  address: string;
  name: string;
  issuer: string;
  fingerprint: string;
  notAfter: number;
  /** Venu d'une autorité de confiance. */
  verified: boolean;
  /** Dernière fois vu sur un message signé (secondes depuis 1970). */
  seen: number;
}

/** Certificat de signature S/MIME d'un compte (lot D2). */
export interface SmimeIdentity {
  name: string;
  addresses: string[];
  issuer: string;
  fingerprint: string;
  /** Validité, en secondes depuis 1970. */
  notBefore: number;
  notAfter: number;
  /** « Signer » enfoncé à l'ouverture d'une rédaction. */
  signByDefault: boolean;
}

/** Compte qui peut signer, pour la rédaction. */
export interface SigningAccount {
  accountId: string;
  signByDefault: boolean;
}

export interface SendReport {
  recipients: number;
  /** Dossier où la copie a été déposée. */
  savedIn: string | null;
  warning: string | null;
}

export interface MoveReport {
  moved: { oldId: number; newId: number }[];
  failed: { id: number; reason: string }[];
  sameAccount: number;
  crossAccount: number;
  /** Hors connexion : déplacements mis en attente, faits au retour du réseau. */
  queued?: number;
}

/** Brouillon enregistré dans « Brouillons ». */
export interface SavedDraft {
  id: number;
  folderId: string;
}

/** Brouillon repris pour continuer sa rédaction. */
export interface DraftContent {
  id: number;
  accountId: string;
  to: string;
  cc: string;
  bcc: string;
  subject: string;
  html: string;
  plainText: boolean;
  inReplyTo: string | null;
  references: string | null;
  attachments: { name: string; contentType: string; size: number; data: string }[];
  /** Brouillon chiffré : « Chiffrer » enfoncé à la reprise (lot D4). */
  encrypt?: boolean;
}

/** Destinataires d'un message à chiffrer (lot D4). */
export interface RecipientCheck {
  /** Sans certificat : l'envoi chiffré est refusé. */
  missing: string[];
  /** Certificat d'une autorité inconnue. */
  unverified: string[];
}

export interface SearchReport {
  results: MessageSummary[];
  errors: AccountError[];
}

export interface FlushReport {
  done: number;
  remaining: number;
}

/** Nouveaux messages signalés par le serveur (IMAP IDLE). */
export interface MailChanged {
  accountId: string;
  folderId: string;
}

// ---------------------------------------------------------------- comptes

/** Type de connexion sécurisée. */
export type Security = "auto" | "sslTls" | "startTls" | "none";
/** « normal » = mot de passe normal (LOGIN) ; « plain » = AUTHENTICATE PLAIN. */
export type AuthMethod = "normal" | "plain" | "oauth2";

export interface ServerConfig {
  host: string;
  port: number;
  security: Security;
  auth: AuthMethod;
  username: string;
  password: string;
}

export interface AccountConfig {
  id: string;
  name: string;
  email: string;
  /** Nom vu par les destinataires (« De : »). */
  displayName: string;
  color: string;
  imap: ServerConfig;
  smtp: ServerConfig;
  smtpSameCredentials: boolean;
  /** ID d'application Outlook.com (comptes OAuth2). */
  oauthClientId: string;
  /** Contacts et agenda (CardDAV/CalDAV) : vide = recherche automatique, « - » = désactivé. */
  davUrl?: string;
  /** Contacts (CardDAV) s'ils sont sur un autre serveur que l'agenda ; vide = même serveur. */
  cardUrl?: string;
}

// ---------------------------------------------------------------- contacts et agenda

/** Carnet d'adresses ou agenda (`account` = « local » : sur cet ordinateur). */
export interface PimCollection {
  account: string;
  url: string;
  kind: "contacts" | "calendar";
  name: string;
  color: string;
}

export interface Contact {
  id: string;
  source: string;
  uid: string;
  /** Nom affiché (FN). */
  name: string;
  /** Prénom et nom de famille (N). */
  givenName?: string;
  familyName?: string;
  emails: string[];
  phones: string[];
  organization: string;
  note: string;
}

/** Date (`20261103`) ou date-heure (`20261103T140000`, `…Z`) iCalendar, avec son fuseau. */
export interface When {
  value: string;
  tzid: string;
}

export interface CalEvent {
  id: string;
  source: string;
  calendar: string;
  uid: string;
  summary: string;
  location: string;
  description: string;
  start: When;
  end: When;
  duration: string;
  allDay: boolean;
  rrule: string;
  exdates: string[];
  /** Occurrence modifiée d'une série (même uid). */
  recurrenceId: string;
  organizer: string;
  /** Rappel en minutes avant le début ; null ou absent : aucun. */
  reminder?: number | null;
  /** Version fixée par l'organisateur (SEQUENCE). */
  sequence?: number;
  /** STATUS en majuscules (CONFIRMED, TENTATIVE, CANCELLED), vide s'il n'y en a pas. */
  status?: string;
  /** Invités et leur réponse (lot H1), lus seulement. */
  attendees?: Attendee[];
  /** Adresse de l'organisateur (`organizer` porte son nom quand il en a un). */
  organizerEmail?: string;
}

/** Invité d'un rendez-vous. */
export interface Attendee {
  email: string;
  /** Nom affiché, vide s'il n'y en a pas. */
  name: string;
  /** PARTSTAT en majuscules (ACCEPTED, DECLINED, TENTATIVE, NEEDS-ACTION…). */
  partstat: string;
}

export interface Suggestion {
  name: string;
  email: string;
  contact: boolean;
}

export interface PimSyncReport {
  contacts: number;
  events: number;
  errors: AccountError[];
}

/** Réponse à une invitation (lot H1). */
export type InvitationAnswer = "accepted" | "tentative" | "declined";

export interface Invitation {
  events: CalEvent[];
  raw: string;
  /** METHOD du calendrier (REQUEST, CANCEL, REPLY…), vide s'il n'y en a pas. */
  method?: string;
  /**
   * Comptes qui peuvent répondre : celui qui a reçu l'invitation d'abord, puis
   * les autres comptes invités (lot H1b).
   */
  addresses?: MyAddress[];
}

/** Bilan d'un import de messages (lot E8a). */
export interface ImportReport {
  imported: number;
  failed: number;
  error: string | null;
  /** Arrêté après plusieurs refus de suite (serveur injoignable…). */
  stopped: boolean;
}

/** Compte qui peut répondre à une invitation, et sa réponse déjà donnée (lot H1). */
export interface MyAddress {
  account: string;
  email: string;
  name: string;
  answer: InvitationAnswer | null;
}

export interface AccountError {
  accountId: string;
  message: string;
}

export interface FolderListing {
  folders: Folder[];
  errors: AccountError[];
}

export interface CheckResult {
  ok: boolean;
  message: string;
}

export interface TestReport {
  imap: CheckResult;
  smtp: CheckResult;
}
