// Maquette JS utilisée seulement hors de Tauri (aperçu navigateur).
// Mêmes règles que le Rust : même compte = déplacement simple,
// autre compte = copie avec nouvel identifiant puis suppression de la source.
import type { Account, AccountConfig, AttachmentInput, CalEvent, Contact, Draft, DraftContent, PimCollection, Suggestion, Folder, FolderListing, FolderRole, Message, MessageSummary, MoveReport, SavedDraft, TestReport, Detected } from "./types";

const now = Math.floor(Date.now() / 1000);
const h = (hours: number) => now - hours * 3600;

const accounts: Account[] = [
  { id: "perso", name: "Gmail perso", email: "fabrice.perso@example.com", color: "#c4314b" },
  { id: "pro", name: "OVH pro", email: "contact@example.fr", color: "#0f6cbd" },
];

const roleNames: Record<FolderRole, string> = {
  inbox: "Boîte de réception",
  drafts: "Brouillons",
  sent: "Éléments envoyés",
  archive: "Archive",
  trash: "Éléments supprimés",
  junk: "Courrier indésirable",
  custom: "",
};

const folders: Folder[] = [];
function addFolder(accountId: string, path: string, role: FolderRole) {
  const parts = path.split("/");
  folders.push({
    id: `${accountId}:${path}`,
    accountId,
    name: role === "custom" ? parts[parts.length - 1] : roleNames[role],
    path,
    parentId: parts.length > 1 ? `${accountId}:${parts.slice(0, -1).join("/")}` : null,
    depth: parts.length - 1,
    role,
    unread: 0,
    total: 0,
    subscribed: true,
  });
}
for (const a of accounts) {
  addFolder(a.id, "INBOX", "inbox");
  addFolder(a.id, "Drafts", "drafts");
  addFolder(a.id, "Sent", "sent");
  addFolder(a.id, "Archive", "archive");
  addFolder(a.id, "Trash", "trash");
}
addFolder("perso", "Factures", "custom");
addFolder("pro", "Clients", "custom");
addFolder("pro", "Clients/Client A", "custom");

let nextId = 1;
const messages = new Map<number, Message>();
function add(folderId: string, fromName: string, fromEmail: string, subject: string, body: string, date: number, seen: boolean, flagged = false, attach = false) {
  const accountId = folderId.split(":")[0];
  const id = nextId++;
  messages.set(id, {
    id, folderId, accountId, fromName, fromEmail, subject,
    preview: body.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim().slice(0, 140),
    date, flags: { seen, flagged, answered: false }, hasAttachments: attach,
    to: accounts.find((a) => a.id === accountId)!.email, bodyHtml: body,
    toList: [accounts.find((a) => a.id === accountId)!.email], ccList: [], replyTo: [],
    messageId: `<demo-${id}@example.com>`, references: "",
    attachments: attach ? [{ index: 0, name: "document.pdf", contentType: "application/pdf", size: 184_320 }] : [],
    imagesAllowed: false,
    bodySimple: body,
    bodyText: body.replace(/<[^>]*>/g, ""),
  });
}
add("perso:INBOX", "Banque Exemple", "alertes@banque.example.com", "Votre relevé d'octobre est disponible", "<p>Bonjour,</p><p>Votre relevé de compte du mois d'octobre est disponible.</p>", h(2), false, false, true);
add("perso:INBOX", "Marie Dupont", "marie.dupont@example.com", "Repas de dimanche", "<p>Coucou !</p><p>On se retrouve à 12 h 30 dimanche ?</p>", h(26), true, true);
add("perso:INBOX", "Librairie en ligne", "commandes@livres.example.com", "Votre commande a été expédiée", "<p>Votre commande arrivera sous 48 h.</p>", h(75), true);
add("perso:Factures", "Opérateur Box", "facture@box.example.com", "Votre facture internet d'octobre", "<p>Facture disponible.</p>", h(400), true, false, true);
add("pro:INBOX", "Sophie Bernard", "s.bernard@client-a.example.com", "Point d'avancement – migration CI/CD", "<p>Pouvons-nous caler un point jeudi ?</p>", h(1), false, true);
add("pro:INBOX", "Julien Petit", "julien.petit@client-b.example.com", "Devis pour l'audit d'architecture", "<p>Vous trouverez ci-joint le cahier des charges.</p>", h(22), true, false, true);
add("pro:INBOX", "Thomas Leroy", "t.leroy@client-a.example.com", "RE: RE: TR: RE: Réunion de lancement du projet de refonte du système d'information – planning, budget prévisionnel et répartition des lots", "<p>Je confirme pour mardi 14 h, salle Everest. J'ai ajouté le budget prévisionnel en pièce jointe.</p><blockquote style=\"border-left:3px solid #9e9e9e;padding-left:12px;margin-left:4px\"><p>Peux-tu confirmer le créneau et le lieu ?</p></blockquote>", h(3), false, false, true);
add("perso:INBOX", "Association des parents d'élèves", "bureau@ape.example.org", "Re: Re: Re: Fwd: Organisation de la kermesse de fin d'année : besoin de bénévoles pour les stands, la buvette et le rangement", "<p>Merci à tous pour vos réponses, voici le tableau des créneaux mis à jour.</p>", h(5), false);
add("pro:INBOX", "Claire Martin", "claire.martin@client-b.example.com", "RE: RE: RE: RE: RE: Problème de connexion VPN depuis la mise à jour du poste – ticket n° 48213", "<p>Le correctif a été déployé, pouvez-vous retester ?</p>", h(30), true, true);
add("pro:INBOX", "Cabinet comptable", "compta@cabinet.example.fr", "Pièces manquantes pour la clôture", "<p>Il nous manque trois justificatifs.</p>", h(48), false);
// Expéditeur usurpé, refusé par DMARC au serveur de réception (lot G1).
add("perso:INBOX", "Banque Exemple", "securite@banque.example.com", "Action requise : vérifiez votre compte", "<p>Votre compte sera suspendu sous 24 h. Confirmez vos identifiants.</p>", h(50), true);
messages.get(nextId - 1)!.senderCheck = "dmarc";

function summary(m: Message): MessageSummary {
  const { to, bodyHtml, ...s } = m;
  void to;
  void bodyHtml;
  return s;
}
const delay = <T,>(v: T) => new Promise<T>((r) => setTimeout(() => r(structuredClone(v)), 30));

export const listAccounts = () => delay(accounts);

export const listFolders = (): Promise<FolderListing> =>
  delay({
    folders: folders.map((f) => {
      const inFolder = [...messages.values()].filter((m) => m.folderId === f.id);
      return { ...f, total: inFolder.length, unread: inFolder.filter((m) => !m.flags.seen).length };
    }),
    errors: [],
  });

export const desktopOnly = "Disponible uniquement dans l'application (npm run tauri dev).";

/** Test d'un compte : réussi avec le mot de passe « secret » (parcours de l'assistant, lot I2). */
export const testAccount = (config: AccountConfig): Promise<TestReport> => {
  const ok = config.imap.password === "secret";
  const refused = { ok: false, message: "Identifiant ou mot de passe refusé (AUTHENTICATIONFAILED)" };
  return delay({ imap: ok ? { ok: true, message: "connexion réussie" } : refused, smtp: ok ? { ok: true, message: "connexion réussie" } : refused });
};

/** Empreinte du certificat de Proton Mail Bridge de la maquette (lot I3). */
export const bridgeCertificate = () =>
  delay(Array.from({ length: 32 }, (_, i) => ((i * 37 + 11) % 256).toString(16).toUpperCase().padStart(2, "0")).join(":"));

/** Comptes ajoutés dans l'aperçu navigateur (lot I2) ; la liste des comptes de la maquette ne change pas. */
const accountConfigs: AccountConfig[] = [];

export const listAccountConfigs = () => delay(accountConfigs);

export function addAccount(config: AccountConfig): Promise<Account> {
  const account = { id: `ajout-${accountConfigs.length + 1}`, name: config.name, email: config.email, color: config.color };
  accountConfigs.push({ ...config, id: account.id, imap: { ...config.imap, password: "" }, smtp: { ...config.smtp, password: "" } });
  return delay(account);
}

/**
 * Réglages trouvés par la maquette (lot I2) : `exemple.fr` dans la base de
 * Thunderbird, `hebergeur.example` par le DNS hors du domaine, `gmail.com` par
 * Google, `proton.me` par Bridge (lot I3) ; tout autre domaine : rien trouvé.
 */
export function detectAccount(email: string): Promise<Detected> {
  const domain = email.split("@")[1]?.toLowerCase() ?? "";
  const tls = (host: string, port: number) => ({ host, port, security: "sslTls" as const });
  const base = { username: email, login: "password" as const, outsideDomain: false, helpUrl: "" };
  if (domain === "exemple.fr") {
    return delay<Detected>({ ...base, source: "ispdb", provider: "Exemple Mail", imap: tls("imap.exemple.fr", 993), smtp: tls("smtp.exemple.fr", 465) });
  }
  if (domain === "hebergeur.example") {
    return delay<Detected>({ ...base, source: "srv", provider: "", imap: tls("imap.mail-hote.example", 993), smtp: tls("smtp.mail-hote.example", 465), outsideDomain: true });
  }
  if (domain === "proton.me") {
    const local = (port: number) => ({ host: "127.0.0.1", port, security: "startTls" as const });
    return delay<Detected>({ ...base, source: "builtin", provider: "Proton Mail", login: "protonBridge", imap: local(1143), smtp: local(1025) });
  }
  if (domain === "gmail.com") {
    return delay<Detected>({ ...base, source: "builtin", provider: "Gmail", login: "google", imap: tls("imap.gmail.com", 993), smtp: tls("smtp.gmail.com", 465) });
  }
  return new Promise((_, reject) => setTimeout(() => reject(`Aucun réglage trouvé pour ${domain} : saisissez les serveurs à la main.`), 30));
}

export const listMessages = (folderId: string) =>
  delay(
    [...messages.values()]
      .filter((m) => m.folderId === folderId)
      .sort((a, b) => b.date - a.date)
      .map(summary),
  );

export function getMessage(id: number): Promise<Message> {
  const m = messages.get(id);
  return m ? delay(m) : Promise.reject(`introuvable : message #${id}`);
}

export async function setSeen(ids: number[], seen: boolean) {
  ids.forEach((id) => { const m = messages.get(id); if (m) m.flags.seen = seen; });
}

export async function setFlagged(ids: number[], flagged: boolean) {
  ids.forEach((id) => { const m = messages.get(id); if (m) m.flags.flagged = flagged; });
}

export async function moveMessages(ids: number[], targetFolderId: string): Promise<MoveReport> {
  const target = folders.find((f) => f.id === targetFolderId);
  if (!target) throw `destination invalide : ${targetFolderId}`;
  const report: MoveReport = { moved: [], failed: [], sameAccount: 0, crossAccount: 0 };
  for (const id of ids) {
    const m = messages.get(id);
    if (!m) { report.failed.push({ id, reason: `introuvable : message #${id}` }); continue; }
    if (m.folderId === target.id) continue;
    if (m.accountId === target.accountId) {
      m.folderId = target.id;
      report.moved.push({ oldId: id, newId: id });
      report.sameAccount++;
    } else {
      const newId = nextId++;
      messages.set(newId, { ...structuredClone(m), id: newId, folderId: target.id, accountId: target.accountId });
      messages.delete(id);
      report.moved.push({ oldId: id, newId });
      report.crossAccount++;
    }
  }
  return report;
}

// ---------------------------------------------------------------- envoi

// Ouvert au premier envoi seulement : un canal ouvert dès le chargement garderait
// Node actif à la fin des tests sans navigateur (npm test).
let sendChannel: BroadcastChannel | null = null;
function announce(event: unknown) {
  if (typeof BroadcastChannel === "undefined") return;
  sendChannel ??= new BroadcastChannel("vortex");
  sendChannel.postMessage(event);
}

/**
 * Envoi simulé comme par le backend (lot E1) : « envoi en cours », puis le
 * résultat, annoncés à la fenêtre principale (lot E4, tests de l'interface).
 * Une adresse qui commence par `echec@` simule un serveur qui refuse le message.
 */
export async function sendMessage(
  accountId: string,
  draft: Draft,
  answeredId: number | null,
  attachments: AttachmentInput[],
  draftId: number | null,
): Promise<void> {
  const recipients = [...draft.to, ...draft.cc, ...draft.bcc];
  if (recipients.length === 0) throw "Ajoutez au moins un destinataire.";
  announce({ type: "sending", accountId });
  const refused = recipients.find((a) => a.toLowerCase().startsWith("echec@"));
  if (refused) {
    announce({
      type: "sendFailed", error: `550 destinataire refusé : ${refused}`,
      accountId, draft, attachments, answeredId, draftId, window: "demo",
    });
    return;
  }
  announce({
    type: "sent", report: { recipients: recipients.length, savedIn: roleNames.sent, warning: null }, accountId, answeredId,
  });
}

// ---------------------------------------------------------------- brouillons et recherche

/**
 * Tests de l'interface (lot E5b) : brouillons présents « sur le serveur », gardés
 * dans le stockage partagé par les fenêtres de l'aperçu, et lenteur réglable du
 * serveur (`vortex.demo.lenteur`, en millisecondes).
 */
const SERVER_DRAFTS = "vortex.demo.brouillons";
function serverDrafts(change: (ids: number[]) => number[]) {
  try {
    const ids = JSON.parse(localStorage.getItem(SERVER_DRAFTS) ?? "[]") as number[];
    localStorage.setItem(SERVER_DRAFTS, JSON.stringify(change(ids)));
  } catch {
    // stockage indisponible : rien à noter
  }
}
function slowness(): number {
  try {
    return Number(localStorage.getItem("vortex.demo.lenteur") ?? 0) || 0;
  } catch {
    return 0;
  }
}

export async function saveDraft(accountId: string, draft: Draft, replace: number | null): Promise<SavedDraft> {
  const folder = folders.find((f) => f.accountId === accountId && f.role === "drafts");
  if (!folder) throw "Aucun dossier « Brouillons » sur ce compte.";
  const wait = slowness();
  if (wait > 0) await new Promise((r) => setTimeout(r, wait));
  if (replace !== null) messages.delete(replace);
  const account = accounts.find((a) => a.id === accountId)!;
  const id = nextId++;
  messages.set(id, {
    id, folderId: folder.id, accountId, fromName: account.name, fromEmail: account.email,
    subject: draft.subject || "(sans objet)",
    preview: draft.html.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim().slice(0, 140),
    date: Math.floor(Date.now() / 1000), flags: { seen: true, flagged: false, answered: false }, hasAttachments: false,
    to: draft.to.join(", "), bodyHtml: draft.html, toList: draft.to, ccList: draft.cc, replyTo: [],
    messageId: `<draft-${id}@example.com>`, references: draft.references ?? "", attachments: [], imagesAllowed: false,
    bodySimple: draft.html,
    bodyText: draft.html.replace(/<[^>]*>/g, ""),
  });
  drafts.set(id, { draft, accountId });
  serverDrafts((ids) => [...ids.filter((x) => x !== replace), id]);
  return { id, folderId: folder.id };
}

const drafts = new Map<number, { draft: Draft; accountId: string }>();

export async function discardDraft(id: number) {
  messages.delete(id);
  drafts.delete(id);
  serverDrafts((ids) => ids.filter((x) => x !== id));
}

export async function openDraft(id: number): Promise<DraftContent> {
  const m = messages.get(id);
  if (!m) throw `introuvable : brouillon #${id}`;
  const saved = drafts.get(id);
  return {
    id,
    accountId: m.accountId,
    to: (saved?.draft.to ?? m.toList).join("; "),
    cc: (saved?.draft.cc ?? m.ccList).join("; "),
    bcc: (saved?.draft.bcc ?? []).join("; "),
    subject: m.subject === "(sans objet)" ? "" : m.subject,
    html: m.bodyHtml,
    plainText: saved?.draft.plainText ?? false,
    inReplyTo: saved?.draft.inReplyTo ?? null,
    references: saved?.draft.references ?? null,
    attachments: [],
  };
}

export async function search(query: string): Promise<MessageSummary[]> {
  const q = query.trim().toLowerCase();
  if (q.length < 2) return [];
  return [...messages.values()]
    .filter((m) => [m.subject, m.fromName, m.fromEmail, m.preview, m.bodyHtml].some((s) => s.toLowerCase().includes(q)))
    .sort((a, b) => b.date - a.date)
    .map(summary);
}

export async function moveToRole(ids: number[], role: FolderRole): Promise<MoveReport> {
  const total: MoveReport = { moved: [], failed: [], sameAccount: 0, crossAccount: 0 };
  for (const id of ids) {
    const m = messages.get(id);
    const target = m && folders.find((f) => f.accountId === m.accountId && f.role === role);
    if (!target) { total.failed.push({ id, reason: "dossier cible introuvable" }); continue; }
    const r = await moveMessages([id], target.id);
    total.moved.push(...r.moved);
    total.failed.push(...r.failed);
    total.sameAccount += r.sameAccount;
    total.crossAccount += r.crossAccount;
  }
  return total;
}

// ---------------------------------------------------------------- contacts et agenda

const collections: PimCollection[] = [
  { account: "local", url: "local:contacts", kind: "contacts", name: "Sur cet ordinateur", color: "#6b7280" },
  { account: "local", url: "local:calendar", kind: "calendar", name: "Sur cet ordinateur", color: "#6b7280" },
  { account: "pro", url: "https://dav.example.fr/cal/pro/", kind: "calendar", name: "Agenda pro", color: "#0f6cbd" },
];

let contactList: Contact[] = [
  { id: "local|1", source: "local", uid: "1", name: "Marie Dupont", emails: ["marie.dupont@example.com"], phones: ["06 12 34 56 78"], organization: "", note: "" },
  { id: "local|2", source: "local", uid: "2", name: "Sophie Bernard", emails: ["s.bernard@client-a.example.com"], phones: ["01 23 45 67 89"], organization: "Client A", note: "Responsable du projet de migration CI/CD" },
  { id: "local|3", source: "local", uid: "3", name: "Julien Petit", emails: ["julien.petit@client-b.example.com"], phones: [], organization: "Client B", note: "" },
];

/** Date-heure locale au format iCalendar (sans fuseau). */
function wall(daysFromToday: number, hour: number, minute = 0): string {
  const d = new Date();
  d.setDate(d.getDate() + daysFromToday);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}T${p(hour)}${p(minute)}00`;
}

function day(daysFromToday: number): string {
  return wall(daysFromToday, 0).slice(0, 8);
}

const base = { location: "", description: "", duration: "", exdates: [] as string[], recurrenceId: "", organizer: "", rrule: "" };
let eventList: CalEvent[] = [
  { ...base, id: "local|e1", source: "local", calendar: "local:calendar", uid: "e1", summary: "Point d'avancement CI/CD", location: "Visio", start: { value: wall(0, 10), tzid: "Europe/Paris" }, end: { value: wall(0, 11), tzid: "Europe/Paris" }, allDay: false },
  { ...base, id: "local|e2", source: "pro", calendar: "https://dav.example.fr/cal/pro/", uid: "e2", summary: "Réunion d'équipe", start: { value: wall(-3, 9, 30), tzid: "Europe/Paris" }, end: { value: wall(-3, 10, 30), tzid: "Europe/Paris" }, allDay: false, rrule: "FREQ=WEEKLY" },
  { ...base, id: "local|e3", source: "local", calendar: "local:calendar", uid: "e3", summary: "Déjeuner avec Marie", location: "Le Bistrot", start: { value: wall(1, 12, 30), tzid: "" }, end: { value: wall(1, 14), tzid: "" }, allDay: false },
  { ...base, id: "local|e4", source: "local", calendar: "local:calendar", uid: "e4", summary: "Audit d'architecture", start: { value: wall(2, 14), tzid: "" }, end: { value: wall(2, 17), tzid: "" }, allDay: false },
  { ...base, id: "local|e5", source: "local", calendar: "local:calendar", uid: "e5", summary: "Congés", start: { value: day(3), tzid: "" }, end: { value: day(5), tzid: "" }, allDay: true },
  { ...base, id: "local|e6", source: "pro", calendar: "https://dav.example.fr/cal/pro/", uid: "e6", summary: "Appel client B", start: { value: wall(2, 15), tzid: "" }, end: { value: wall(2, 16), tzid: "" }, allDay: false },
  // Plus de 24 h, à heures fixes (corpus F4) : ne devient jamais « toute la journée ».
  { ...base, id: "local|e7", source: "local", calendar: "local:calendar", uid: "e7", summary: "Conférence DevOps", location: "Lyon", start: { value: wall(0, 9), tzid: "Europe/Paris" }, end: { value: wall(2, 17), tzid: "Europe/Paris" }, allDay: false },
];

export const pimCollections = () => delay(collections);
export const contacts = () => delay(contactList);
export const events = () => delay(eventList);

export async function saveContact(contact: Contact, collection: string): Promise<Contact> {
  const account = collections.find((c) => c.url === collection)?.account ?? "local";
  const saved = { ...contact, uid: contact.uid || String(Date.now()), source: account };
  saved.id = contact.id || `${account}|${saved.uid}`;
  contactList = [...contactList.filter((c) => c.id !== saved.id), saved];
  return saved;
}

export async function saveEvent(event: CalEvent, collection: string): Promise<CalEvent> {
  const account = collections.find((c) => c.url === collection)?.account ?? "local";
  const saved = { ...event, uid: event.uid || String(Date.now()), source: account, calendar: collection };
  saved.id = event.id || `${account}|${saved.uid}`;
  eventList = [...eventList.filter((e) => e.id !== saved.id), saved];
  return saved;
}

export async function pimDelete(id: string) {
  contactList = contactList.filter((c) => c.id !== id);
  eventList = eventList.filter((e) => e.id !== id);
}

export async function suggest(query: string): Promise<Suggestion[]> {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const out: Suggestion[] = [];
  for (const c of contactList)
    for (const e of c.emails)
      if (c.name.toLowerCase().includes(q) || e.toLowerCase().includes(q)) out.push({ name: c.name, email: e, contact: true });
  for (const m of messages.values())
    if ((m.fromName.toLowerCase().includes(q) || m.fromEmail.toLowerCase().includes(q)) && !out.some((s) => s.email === m.fromEmail))
      out.push({ name: m.fromName, email: m.fromEmail, contact: false });
  return out.slice(0, 8);
}
