import { invoke } from "@tauri-apps/api/core";
import type {
  Account,
  AccountConfig,
  AttachmentInput,
  CalEvent,
  Contact,
  CorrespondentInfo,
  Detected,
  Draft,
  DraftContent,
  FlushReport,
  FolderListing,
  FolderRole,
  ImportReport,
  Invitation,
  InvitationAnswer,
  Message,
  MessageSummary,
  MoveReport,
  NewVersion,
  RestoreReport,
  PimCollection,
  PimSyncReport,
  PrefetchReport,
  RecipientCheck,
  SavedDraft,
  SearchReport,
  Security,
  SigningAccount,
  SmimeIdentity,
  Suggestion,
  TestReport,
} from "./types";
import type { ListSync } from "./lists";
import type * as Mock from "./mock";

/**
 * Maquette de l'aperçu navigateur, chargée à sa première utilisation (audit
 * P7) : dans Tauri, elle n'est jamais appelée, donc jamais téléchargée.
 * `mock.f(…)` rend la promesse de `f` une fois le module chargé.
 */
type LazyMock = {
  [K in keyof typeof Mock as (typeof Mock)[K] extends (...args: never[]) => unknown ? K : never]: (typeof Mock)[K] extends (
    ...args: infer A
  ) => infer R
    ? (...args: A) => Promise<Awaited<R>>
    : never;
};
let mockModule: Promise<typeof Mock> | undefined;
const loadMock = (): Promise<typeof Mock> => (mockModule ??= import("./mock"));
const mock = new Proxy({} as LazyMock, {
  get:
    (_, name: string) =>
    (...args: unknown[]) =>
      loadMock().then((m) => (m as unknown as Record<string, (...a: unknown[]) => unknown>)[name](...args)),
});
/** Refus d'une commande réservée à l'application, dans l'aperçu navigateur. */
const desktopOnly = (): Promise<never> => loadMock().then((m) => Promise.reject(m.desktopOnly));

/**
 * Dans la fenêtre Tauri, on appelle le backend Rust.
 * Dans un navigateur (`npm run dev` seul), on utilise une maquette JS :
 * pratique pour travailler l'interface sans recompiler le Rust.
 */
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
// Aperçu navigateur : chargée dès le départ, pour qu'une action de
// l'utilisateur (ouvrir une rédaction) n'attende pas son téléchargement et
// garde le droit d'ouvrir une fenêtre.
if (!inTauri) void loadMock();

export const api = {
  listAccounts: (): Promise<Account[]> =>
    inTauri ? invoke("list_accounts") : mock.listAccounts(),

  /** Arborescence en cache : immédiat, même hors ligne. */
  listFolders: (): Promise<FolderListing> =>
    inTauri ? invoke("list_folders") : mock.listFolders(),

  /** Relit les dossiers sur les serveurs ; erreurs de connexion données par compte. */
  syncFolders: (): Promise<FolderListing> =>
    inTauri ? invoke("sync_folders") : mock.listFolders(),

  /** Messages en cache : immédiat, même hors ligne. */
  listMessages: (folderId: string): Promise<MessageSummary[]> =>
    inTauri ? invoke("list_messages", { folderId }) : mock.listMessages(folderId),

  /** Synchronise le dossier avec son serveur et renvoie la liste à jour. */
  syncMessages: (folderId: string): Promise<MessageSummary[]> =>
    inTauri ? invoke("sync_messages", { folderId }) : mock.listMessages(folderId),

  /** Plusieurs dossiers d'un coup (vue unifiée) : comptes synchronisés en parallèle. */
  syncMessagesMany: (folderIds: string[]): Promise<{ folderId: string; messages: MessageSummary[]; error: string | null }[]> =>
    inTauri
      ? invoke("sync_messages_many", { folderIds })
      : Promise.all(folderIds.map(async (folderId) => ({ folderId, messages: await mock.listMessages(folderId), error: null }))),

  /**
   * Comme `syncMessagesMany`, sans renvoyer une liste inchangée depuis la
   * version `known` (audit P1) : `messages` vaut alors `null`.
   */
  syncChangedLists: (folderIds: string[], known: (number | null)[]): Promise<ListSync[]> =>
    inTauri
      ? invoke("sync_messages_many", { folderIds, known })
      : Promise.all(
          folderIds.map(async (folderId) => ({ folderId, messages: await mock.listMessages(folderId), revision: 0, uidValidity: 1, error: null })),
        ),

  getMessage: (id: number): Promise<Message> =>
    inTauri ? invoke("get_message", { id }) : mock.getMessage(id),

  /** Corps nettoyé et texte d'un corps rendu par `getMessage` (audit P7). */
  bodyViews: (html: string): Promise<{ simple: string; text: string }> =>
    inTauri ? invoke("body_views", { html }) : mock.bodyViews(html),

  setSeen: (ids: number[], seen: boolean): Promise<void> =>
    inTauri ? invoke("set_seen", { ids, seen }) : mock.setSeen(ids, seen),

  setFlagged: (ids: number[], flagged: boolean): Promise<void> =>
    inTauri ? invoke("set_flagged", { ids, flagged }) : mock.setFlagged(ids, flagged),

  moveMessages: (ids: number[], targetFolderId: string): Promise<MoveReport> =>
    inTauri
      ? invoke("move_messages", { ids, targetFolderId })
      : mock.moveMessages(ids, targetFolderId),

  moveToRole: (ids: number[], role: FolderRole): Promise<MoveReport> =>
    inTauri ? invoke("move_to_role", { ids, role }) : mock.moveToRole(ids, role),

  /** Une étape du téléchargement hors connexion ; `resync` : resynchroniser les dossiers. */
  prefetchStep: (resync: boolean): Promise<PrefetchReport> =>
    inTauri ? invoke("prefetch_step", { resync }) : Promise.resolve({ fetched: 0, remaining: 0, done: true }),

  /** Télécharge et mémorise les images distantes du message ; renvoie le message à jour. */
  showImages: (id: number): Promise<Message> =>
    inTauri ? invoke("show_images", { id }) : desktopOnly(),

  /** Signature S/MIME examinée de nouveau (lot D1) ; renvoie le message à jour. */
  checkSignature: (id: number): Promise<Message> =>
    inTauri ? invoke("check_signature", { id }) : desktopOnly(),

  /** Certificat de signature S/MIME d'un compte (lot D2), ou `null`. */
  smimeIdentity: (accountId: string): Promise<SmimeIdentity | null> =>
    inTauri ? invoke("smime_identity", { accountId }) : Promise.resolve(null),
  /** Importe un fichier PKCS #12 (`.p12`, `.pfx`), donné en base64. */
  smimeImport: (accountId: string, data: string, password: string): Promise<SmimeIdentity> =>
    inTauri ? invoke("smime_import", { accountId, data, password }) : desktopOnly(),
  smimeRemove: (accountId: string): Promise<void> =>
    inTauri ? invoke("smime_remove", { accountId }) : desktopOnly(),
  smimeSignByDefault: (accountId: string, on: boolean): Promise<void> =>
    inTauri ? invoke("smime_sign_by_default", { accountId, on }) : desktopOnly(),
  /** Comptes qui peuvent signer (fenêtre de rédaction). */
  smimeSigners: (): Promise<SigningAccount[]> => (inTauri ? invoke("smime_signers") : Promise.resolve([])),
  /** Adresses sans certificat pour leur écrire chiffré (lot D3b). */
  smimeMissing: (addresses: string[]): Promise<RecipientCheck> =>
    inTauri ? invoke("smime_missing", { addresses }) : Promise.resolve({ missing: addresses, unverified: [] }),
  /** Certificats des correspondants (lot D3b). */
  smimeCorrespondents: (): Promise<CorrespondentInfo[]> => (inTauri ? invoke("smime_correspondents") : Promise.resolve([])),
  smimeForgetCorrespondent: (address: string): Promise<void> =>
    inTauri ? invoke("smime_forget_correspondent", { address }) : desktopOnly(),

  /** Nouveau dossier (sous `parentId` s'il est donné) ; renvoie l'arborescence à jour. */
  createFolder: (accountId: string, parentId: string | null, name: string): Promise<FolderListing> =>
    inTauri ? invoke("create_folder", { accountId, parentId, name }) : desktopOnly(),

  /** Renomme un dossier personnel ; renvoie l'arborescence à jour. */
  renameFolder: (folderId: string, name: string): Promise<FolderListing> =>
    inTauri ? invoke("rename_folder", { folderId, name }) : desktopOnly(),

  /** Déplace un dossier personnel sous `parentId` (même compte), ou au premier niveau (`null`). */
  moveFolder: (folderId: string, parentId: string | null): Promise<FolderListing> =>
    inTauri ? invoke("move_folder", { folderId, parentId }) : desktopOnly(),

  deleteFolder: (folderId: string): Promise<FolderListing> =>
    inTauri ? invoke("delete_folder", { folderId }) : desktopOnly(),

  /** Enregistre des messages en `.eml` dans Téléchargements ; chemins écrits (lot E8a). */
  exportEml: (ids: number[]): Promise<string[]> => (inTauri ? invoke("export_eml", { ids }) : desktopOnly()),

  /** Enregistre tout un dossier en `.mbox` dans Téléchargements (lot E8a). */
  exportMbox: (folderId: string): Promise<{ path: string; count: number }> =>
    inTauri ? invoke("export_mbox", { folderId }) : desktopOnly(),

  /**
   * Dépose dans un dossier les messages d'un `.eml` ou d'un `.mbox` (lot E8a),
   * envoyés en octets (audit P7) ; le dossier passe dans un en-tête.
   */
  importMessages: (folderId: string, data: Uint8Array): Promise<ImportReport> =>
    inTauri
      ? invoke("import_messages", data, { headers: { "X-Vortex-Folder": encodeURIComponent(folderId) } })
      : desktopOnly(),

  /** Enregistre un agenda (.ics) ou un carnet (.vcf) dans Téléchargements (lot E8b). */
  exportPim: (collection: string): Promise<{ path: string; count: number }> =>
    inTauri ? invoke("export_pim", { collection }) : desktopOnly(),

  /** Importe un fichier .ics dans un agenda, ou .vcf dans un carnet (lot E8b). */
  importPim: (collection: string, content: string): Promise<ImportReport> =>
    inTauri ? invoke("import_pim", { collection, content }) : desktopOnly(),

  /** Vide définitivement la corbeille ou le courrier indésirable ; nombre de messages supprimés. */
  emptyFolder: (folderId: string): Promise<number> =>
    inTauri ? invoke("empty_folder", { folderId }) : desktopOnly(),

  /** Remet des messages d'« Éléments supprimés » dans leur dossier d'origine (sinon la boîte de réception). */
  restoreMessages: (ids: number[]): Promise<RestoreReport> =>
    inTauri ? invoke("restore_messages", { ids }) : mock.restoreMessages(ids),

  /** Supprime définitivement des messages de la corbeille ou du courrier indésirable ; nombre supprimé. */
  deleteForever: (ids: number[]): Promise<number> =>
    inTauri ? invoke("delete_forever", { ids }) : desktopOnly(),

  /**
   * Envoi en arrière-plan (lot E1) : rend la main une fois le message vérifié ;
   * le résultat arrive à la fenêtre principale (événements `sent` ou `sendFailed`).
   * `draftId` : brouillon à supprimer après l'envoi.
   */
  sendMessage: (
    accountId: string,
    draft: Draft,
    answeredId: number | null,
    attachments: AttachmentInput[],
    draftId: number | null = null,
  ): Promise<void> =>
    inTauri
      ? invoke("send_message", { accountId, draft, answeredId, attachments, draftId })
      : mock.sendMessage(accountId, draft, answeredId, attachments, draftId),

  /** Enregistre le brouillon dans « Brouillons » (remplace `replace`). */
  saveDraft: (
    accountId: string,
    draft: Draft,
    attachments: AttachmentInput[],
    replace: number | null,
    /** Brouillon chiffré remplacé en clair : seulement si l'utilisateur a désactivé « Chiffrer » (audit S4). */
    allowPlain = false,
  ): Promise<SavedDraft> =>
    inTauri
      ? invoke("save_draft", { accountId, draft, attachments, replace, allowPlain })
      : mock.saveDraft(accountId, draft, replace),

  discardDraft: (id: number): Promise<void> =>
    inTauri ? invoke("discard_draft", { id }) : mock.discardDraft(id),

  openDraft: (id: number): Promise<DraftContent> =>
    inTauri ? invoke("open_draft", { id }) : mock.openDraft(id),

  /** Recherche immédiate dans le cache (hors connexion aussi). */
  searchLocal: (query: string): Promise<MessageSummary[]> =>
    inTauri ? invoke("search_local", { query }) : mock.search(query),

  /** Recherche sur les serveurs, dans tous les dossiers de tous les comptes. */
  searchMessages: (query: string): Promise<SearchReport> =>
    inTauri ? invoke("search_messages", { query }) : mock.search(query).then((results) => ({ results, errors: [], unfinished: 0 })),

  /** Rejoue les actions faites hors connexion et reprend les déplacements interrompus. */
  flushPending: (): Promise<FlushReport> =>
    inTauri ? invoke("flush_pending") : Promise.resolve({ done: 0, remaining: 0 }),

  // ------------------------------------------------------------ contacts et agenda

  /** Relit carnets et agendas sur les serveurs (CardDAV/CalDAV). */
  pimSync: (): Promise<PimSyncReport> =>
    inTauri ? invoke("pim_sync") : Promise.resolve({ contacts: 0, events: 0, errors: [] }),

  pimCollections: (): Promise<PimCollection[]> =>
    inTauri ? invoke("pim_collections") : mock.pimCollections(),

  contacts: (): Promise<Contact[]> => (inTauri ? invoke("contacts_list") : mock.contacts()),

  saveContact: (contact: Contact, collection: string): Promise<Contact> =>
    inTauri ? invoke("contact_save", { contact, collection }) : mock.saveContact(contact, collection),

  events: (): Promise<CalEvent[]> => (inTauri ? invoke("events_list") : mock.events()),

  saveEvent: (event: CalEvent, collection: string, raw: string | null = null): Promise<CalEvent> =>
    inTauri ? invoke("event_save", { event, collection, raw }) : mock.saveEvent(event, collection),

  /** Supprime un contact, ou un rendez-vous (toute la série). */
  pimDelete: (id: string): Promise<void> => (inTauri ? invoke("pim_delete", { id }) : mock.pimDelete(id)),

  /** Adresses proposées pendant la saisie d'un destinataire. */
  suggestAddresses: (query: string): Promise<Suggestion[]> =>
    inTauri ? invoke("address_suggest", { query }) : mock.suggest(query),

  /** Ne plus proposer une adresse collectée (lot J13) ; un contact reste proposé. */
  forgetAddress: (email: string): Promise<void> =>
    inTauri ? invoke("forget_address", { email }) : mock.forgetAddress(email),

  /** Rendez-vous d'une invitation reçue (pièce jointe .ics). */
  readInvitation: (id: number, index: number, accountId?: string): Promise<Invitation> =>
    inTauri ? invoke("read_invitation", { id, index, accountId: accountId ?? null }) : desktopOnly(),

  /**
   * Réponse à une invitation (lot H1) : `send` l'envoie à l'organisateur.
   * Rend l'invitation où la ligne du compte porte la réponse, pour l'agenda.
   */
  answerInvitation: (
    id: number,
    index: number,
    accountId: string,
    answer: InvitationAnswer,
    send: boolean,
    when: string,
    asAccount?: string,
  ): Promise<{ raw: string }> =>
    inTauri
      ? invoke("answer_invitation", { id, index, accountId, reply: { answer, send, when, asAccount: asAccount ?? null } })
      : desktopOnly(),

  /** Source d'un rendez-vous ou d'un contact, telle que le serveur l'a donnée. */
  pimSource: (id: string): Promise<string> => (inTauri ? invoke("pim_source", { id }) : Promise.resolve("")),

  /**
   * Réponse à une invitation déposée dans l'agenda (lot H1b) ; `id` : le
   * rendez-vous en cache ; `asAccount` : compte invité qui répond.
   */
  answerEvent: (id: string, answer: InvitationAnswer, send: boolean, when: string, asAccount?: string): Promise<void> =>
    inTauri ? invoke("answer_event", { id, reply: { answer, send, when, asAccount: asAccount ?? null } }) : desktopOnly(),

  /** Messages concernés par des actions en attente du réseau. */
  pendingCount: (): Promise<number> => (inTauri ? invoke("pending_count") : Promise.resolve(0)),

  /** Ouvre la pièce jointe avec l'application par défaut du système. */
  openAttachment: (id: number, index: number): Promise<string> =>
    inTauri ? invoke("open_attachment", { id, index }) : desktopOnly(),

  /** Enregistre la pièce jointe dans Téléchargements ; renvoie le chemin. */
  saveAttachment: (id: number, index: number): Promise<string> =>
    inTauri ? invoke("save_attachment", { id, index }) : desktopOnly(),

  /** Connexion Outlook.com dans le navigateur ; se termine au retour (5 min au plus). */
  oauthSignIn: (email: string, clientId: string): Promise<{ calendar: boolean }> =>
    inTauri ? invoke("oauth_sign_in", { email, clientId }) : desktopOnly(),

  /**
   * Sauvegarde chiffrée de la configuration ; renvoie le chemin du fichier et
   * les comptes dont le mot de passe manque (absent de la sauvegarde).
   */
  exportBackup: (password: string, settings: string): Promise<{ path: string; missing: string[] }> =>
    inTauri ? invoke("export_backup", { password, settings }) : desktopOnly(),
  inspectBackup: (content: string, password: string): Promise<BackupInfo> =>
    inTauri ? invoke("inspect_backup", { content, password }) : desktopOnly(),
  /** Remplace la configuration ; renvoie les réglages de l'interface à remettre en place. */
  importBackup: (content: string, password: string): Promise<string> =>
    inTauri ? invoke("import_backup", { content, password }) : desktopOnly(),
  restartApp: (): Promise<void> => (inTauri ? invoke("restart_app") : Promise.resolve(window.location.reload())),
  o365Status: (email: string): Promise<boolean> => (inTauri ? invoke("o365_status", { email }) : Promise.resolve(false)),
  o365SignOut: (accountId: string, email: string): Promise<void> =>
    inTauri ? invoke("o365_sign_out", { accountId, email }) : desktopOnly(),
  /** Connexion Google ; `mail` : autoriser aussi le courrier (compte réglé en OAuth2). */
  googleSignIn: (email: string, clientId: string, clientSecret: string, mail = false): Promise<void> =>
    inTauri ? invoke("google_sign_in", { email, clientId, clientSecret, mail }) : desktopOnly(),
  googleStatus: (email: string): Promise<{ signedIn: boolean; hasClient: boolean }> =>
    inTauri ? invoke("google_status", { email }) : Promise.resolve({ signedIn: false, hasClient: false }),
  googleSignOut: (accountId: string, email: string): Promise<void> =>
    inTauri ? invoke("google_sign_out", { accountId, email }) : desktopOnly(),
  oauthStatus: (email: string): Promise<{ signedIn: boolean; hasClientId: boolean }> =>
    inTauri ? invoke("oauth_status", { email }) : Promise.resolve({ signedIn: false, hasClientId: false }),

  /** Ouvre une adresse web dans le navigateur du système. */
  /** Mode portable : données à côté de l'exécutable, coffre protégé par un mot de passe maître. */
  portableStatus: (): Promise<{ portable: boolean; hasVault: boolean; unlocked: boolean }> =>
    inTauri ? invoke("portable_status") : Promise.resolve({ portable: false, hasVault: false, unlocked: true }),
  portableUnlock: (password: string): Promise<void> => (inTauri ? invoke("portable_unlock", { password }) : Promise.resolve()),
  portableChangePassword: (old: string, next: string): Promise<void> =>
    inTauri ? invoke("portable_change_password", { old, new: next }) : Promise.resolve(),
  /** Fenêtre secondaire (message, rédaction), créée par l'application. */
  /** `sources` : messages dont une fenêtre de rédaction peut reprendre les pièces jointes (transfert). */
  openWindow: (
    label: string,
    url: string,
    title: string,
    width: number,
    height: number,
    sources: number[] = [],
    init: string | null = null,
    settings: string | null = null,
  ): Promise<void> => invoke("open_window", { label, url, title, width, height, sources, init, settings }),
  /** Fenêtre secondaire : réglages et brouillon de départ confiés à son ouverture. */
  windowBootstrap: (): Promise<{ settings: string; init: string | null }> => invoke("window_bootstrap"),
  /** Fenêtre secondaire : prévient la fenêtre principale (par le backend). */
  notifyMain: (event: unknown): Promise<void> => invoke("notify_main", { event }),
  /** Copie de secours d'un brouillon (`null` : la retirer), gardée dans le cache chiffré ; `key` : brouillon de la fenêtre principale. */
  saveDraftBackup: (backup: string | null, key?: string): Promise<void> => invoke("save_draft_backup", { backup, key: key ?? null }),
  /** Fenêtre principale : copies de secours gardées par le backend. */
  draftBackups: (): Promise<[string, string][]> => (inTauri ? invoke("draft_backups") : Promise.resolve([])),
  clearDraftBackup: (key: string): Promise<void> => (inTauri ? invoke("clear_draft_backup", { key }) : Promise.resolve()),
  /** Connexions Google / Outlook.com à refaire (expirées ou révoquées). */
  /** Connexions à refaire ; `scope` : ce que sert le jeton (courrier, agenda et contacts, ou les deux). */
  authPending: (): Promise<{ provider: "google" | "o365"; user: string; scope?: "mail" | "pim" | "all" }[]> =>
    inTauri ? invoke("auth_pending") : Promise.resolve([]),
  /** Langue de l'interface, pour les notifications produites par l'application. */
  setLanguage: (language: string): Promise<void> =>
    inTauri ? invoke("set_language", { language }) : Promise.resolve(),
  /** Rappel de rendez-vous : notification du bureau. */
  showReminder: (summary: string, body: string): Promise<void> =>
    inTauri ? invoke("show_reminder", { summary, body }) : Promise.resolve(),
  /** Nouveaux messages restés dans une boîte de réception après les règles : notification du bureau (lot B4). */
  notifyNewMail: (summary: string, body: string): Promise<void> =>
    inTauri ? invoke("notify_new_mail", { summary, body }) : Promise.resolve(),
  /** Fermer la fenêtre principale la cache dans la zone de notification (lot B3). */
  setKeepInTray: (keep: boolean): Promise<void> => (inTauri ? invoke("set_keep_in_tray", { keep }) : Promise.resolve()),
  /** Règles et signatures du cache chiffré (audit S8) : (clé, valeur). */
  prefsGet: (): Promise<[string, string][]> => (inTauri ? invoke("prefs_get") : Promise.resolve([])),
  prefsSet: (key: string, value: string | null): Promise<void> => (inTauri ? invoke("prefs_set", { key, value }) : Promise.resolve()),
  /** Nouvelle version publiée sur GitHub (lot K1) ; null : à jour. */
  checkUpdate: (): Promise<NewVersion | null> => (inTauri ? invoke("check_update") : Promise.resolve(null)),
  openUrl: (url: string): Promise<void> =>
    inTauri ? invoke("open_url", { url }) : Promise.resolve(void window.open(url, "_blank", "noopener")),

  listAccountConfigs: (): Promise<AccountConfig[]> =>
    inTauri ? invoke("list_account_configs") : mock.listAccountConfigs(),

  /** Réglages d'un compte trouvés d'après son adresse, sans mot de passe (lot I1). */
  detectAccount: (email: string): Promise<Detected> =>
    inTauri ? invoke("detect_account", { email }) : mock.detectAccount(email),

  /** Empreinte du certificat de Proton Mail Bridge sur cet ordinateur, à confirmer avant de l'épingler (lot I3). */
  bridgeCertificate: (port: number, security: Security): Promise<string> =>
    inTauri ? invoke("bridge_certificate", { port, security }) : mock.bridgeCertificate(),

  testAccount: (config: AccountConfig): Promise<TestReport> =>
    inTauri ? invoke("test_account", { config }) : mock.testAccount(config),

  addAccount: (config: AccountConfig): Promise<Account> =>
    inTauri ? invoke("add_account", { config }) : mock.addAccount(config),

  /** Mot de passe vide = inchangé. */
  updateAccount: (config: AccountConfig): Promise<Account> =>
    inTauri ? invoke("update_account", { config }) : desktopOnly(),

  removeAccount: (id: string): Promise<void> =>
    inTauri ? invoke("remove_account", { id }) : desktopOnly(),
};

export const runningInTauri = inTauri;

/** Contenu d'une sauvegarde, avant import. */
export interface BackupInfo {
  created: number;
  accounts: { name: string; email: string }[];
  localItems: number;
}
