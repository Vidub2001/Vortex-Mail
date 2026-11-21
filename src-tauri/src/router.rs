//! Le routeur relie les comptes IMAP configurés au cache local et au trait
//! `MailBackend` : les opérations de `mail-core` (dont le déplacement entre
//! comptes) fonctionnent ainsi entre n'importe quels comptes.
//!
//! Lecture : le cache (`cache.rs`) sert l'interface sans réseau.
//! Écriture : chaque action réussie sur un serveur est répercutée dans le cache.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use imap_lite::{FetchItem, ListEntry, MailboxStatus, SearchKey, SelectInfo, UidSet};
use mail_core::{
    Account, FailedMove, Flags, Folder, FolderId, FolderRole, Journal, JournalAccess, MailBackend, MailError, Message, MessageId,
    MessageSummary, MoveReport, RawMessage, SharedJournal,
};
use serde::Serialize;

use crate::cache::{self, BodyExtra, Cache, Header, ImapRef, PendingAction, PendingSent};
use crate::config::AccountConfig;
use crate::mimeparse;
use crate::smime;
use crate::transport::{self, Session};
use mail_core::compose::{self, Draft, OutgoingAttachment};
use mail_core::search::SearchQuery;
use serde::Deserialize;

/// Messages récents téléchargés à la première synchronisation d'un dossier.
const LIST_LIMIT: u32 = 200;

/// Octets du début du texte lus pour l'aperçu (`BODY.PEEK[TEXT]<0.2048>`).
const PREVIEW_BYTES: usize = 2048;

/// En-têtes + début du texte : de quoi afficher la liste avec un aperçu.
const LIST_QUERY: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE \
    BODY.PEEK[HEADER.FIELDS (FROM TO SUBJECT DATE CONTENT-TYPE CONTENT-TRANSFER-ENCODING MIME-VERSION)] \
    BODY.PEEK[TEXT]<0.2048>)";

const FULL_QUERY: &str = "(UID FLAGS INTERNALDATE BODY.PEEK[])";
/// Type d'un message, pour savoir s'il est signé en S/MIME (lot D1).
const TYPE_QUERY: &str = "(UID BODY.PEEK[HEADER.FIELDS (CONTENT-TYPE)])";

/// Comme `LIST_QUERY`, avec le `Message-ID` (dédoublonnage des résultats de recherche).
const SEARCH_QUERY: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE \
    BODY.PEEK[HEADER.FIELDS (FROM TO SUBJECT DATE MESSAGE-ID CONTENT-TYPE CONTENT-TRANSFER-ENCODING MIME-VERSION)] \
    BODY.PEEK[TEXT]<0.2048>)";

/// Résultats de recherche : au plus tant par dossier, et au total.
const SEARCH_PER_FOLDER: usize = 60;
const SEARCH_MAX: usize = 300;

/// Erreur due au réseau (serveur injoignable, connexion perdue), et non à un
/// refus du serveur : l'action peut être mise en attente puis rejouée.
pub(crate) fn is_network_error(message: &str) -> bool {
    let m = message.to_lowercase();
    // Refus du serveur (`NO`, `BAD`) ou dossier renuméroté : jamais une panne,
    // même si le texte du serveur contient « timed out » ou « network ».
    // Sinon l'action resterait en attente et bloquerait les suivantes
    // (relecture complète n° 32).
    let refused = [
        "refusé par le serveur",
        "refused by the server",
        "rechazado por el servidor",
        "rejetée par le serveur",
        "rejected by the server",
        "comando rechazado",
        "renuméroté par le serveur",
        "renumbered by the server",
        "ha renumerado",
    ];
    if refused.iter().any(|k| m.contains(k)) {
        return false;
    }
    // Messages du transport (transport.rs) dans les trois langues de l'interface,
    // et erreurs système (souvent en anglais).
    [
        "» introuvable",
        "” not found",
        "” no encontrado",
        "connexion à",
        "could not connect",
        "no se puede conectar",
        "erreur réseau",
        "network error",
        "error de red",
        "connexion fermée",
        "connection closed",
        "conexión cerrada",
        "timed out",
        "délai",
        "network",
        "unreachable",
        "failed to lookup",
        "résolution",
        "connection reset",
        "connection refused",
        "broken pipe",
    ]
    .iter()
    .any(|k| m.contains(k))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountError {
    pub account_id: String,
    pub message: String,
}

/// Connexion utilisée : chaque compte en a deux, pour qu'une synchronisation
/// ne fasse jamais attendre une action de l'utilisateur (et inversement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lane {
    /// Synchronisations, compléments, recherche.
    Sync,
    /// Ouvrir, marquer, déplacer, envoyer, brouillons, dossiers.
    User,
}

#[derive(Default)]
struct AccountState {
    folders: Vec<Folder>,
    /// Gmail : chemin de « Tous les messages » (`\All`), affiché comme Archive
    /// mais limité aux messages archivés (hors réception, envoyés, brouillons).
    gmail_all: Option<String>,
}

/// Un compte : sa configuration, son arborescence et ses deux connexions,
/// chacune sous son propre verrou (les comptes sont indépendants entre eux).
pub(crate) struct ImapAccount {
    pub config: AccountConfig,
    sync: Mutex<Option<Session>>,
    user: Mutex<Option<Session>>,
    state: Mutex<AccountState>,
}

/// Verrou tolérant : une panique dans une opération ne bloque pas les suivantes.
///
/// Écart voulu à la règle « verrou empoisonné = arrêt » (lot 10) : arrêter
/// ferait d'une panique un plantage à chaque démarrage. Ce n'est sûr que pour
/// des données remplacées d'un bloc (`AccountState`, tour de préchargement) ;
/// les connexions IMAP, elles, sont fermées après une panique (`ImapAccount::run`).
/// Règle : sous ces verrous, aucune modification en plusieurs étapes.
fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl ImapAccount {
    fn new(config: AccountConfig) -> Self {
        Self { config, sync: Mutex::new(None), user: Mutex::new(None), state: Mutex::new(AccountState::default()) }
    }

    /// Exécute une opération sur une connexion, en se (re)connectant au besoin.
    /// Une connexion perdue (mise en veille, coupure) est retentée une fois.
    fn run<T>(&self, lane: Lane, op: impl FnMut(&mut Session) -> imap_lite::Result<T>) -> Result<T, String> {
        self.run_tries(lane, 2, op)
    }

    /// Comme [`ImapAccount::run`], avec `tries` essais au plus (1 : jamais
    /// refaite). La recherche sur les serveurs n'en fait qu'un : un dossier
    /// qui dépasse le délai de lecture n'est pas recherché une seconde fois
    /// (lot J14).
    fn run_tries<T>(&self, lane: Lane, tries: u32, mut op: impl FnMut(&mut Session) -> imap_lite::Result<T>) -> Result<T, String> {
        let slot = match lane {
            Lane::Sync => &self.sync,
            Lane::User => &self.user,
        };
        // Une panique a interrompu une commande : la réponse du serveur est
        // peut-être restée à moitié lue, et la commande suivante la prendrait
        // pour la sienne (comme en Y7). On repart d'une connexion neuve.
        let mut session = slot.lock().unwrap_or_else(|poisoned| {
            let mut session = poisoned.into_inner();
            *session = None;
            slot.clear_poison();
            session
        });
        let mut attempt = 0;
        loop {
            attempt += 1;
            if session.is_none() {
                *session = Some(open_session(&self.config.imap)?);
            }
            let result = op(session.as_mut().expect("session ouverte juste avant"));
            // Alertes du serveur reçues pendant l'opération (connexion comprise) :
            // montrées à l'utilisateur (relecture complète n° 22).
            if let Some(s) = session.as_mut() {
                for text in s.take_alerts() {
                    crate::auth::server_alert(&self.config.email, &text);
                }
            }
            match result {
                Ok(value) => return Ok(value),
                Err(e) if e.is_connection_lost() => {
                    *session = None;
                    if attempt >= tries {
                        return Err(e.to_string());
                    }
                }
                // Réponse incompréhensible : la suite de la réponse est peut-être
                // restée dans le tampon, et la commande suivante la prendrait pour
                // la sienne. On repart d'une connexion neuve (relecture Y7).
                Err(e @ imap_lite::Error::Protocol(_)) => {
                    *session = None;
                    return Err(e.to_string());
                }
                Err(e) => return Err(e.to_string()),
            }
        }
    }

    /// Ferme les deux connexions (attend la fin d'une opération en cours).
    fn close(&self) {
        for lane in [&self.sync, &self.user] {
            if let Some(s) = guard(lane).take() {
                let _ = s.logout();
            }
        }
    }

    fn folders(&self) -> Vec<Folder> {
        guard(&self.state).folders.clone()
    }

    /// Un dossier, sans copier toute l'arborescence (audit P4).
    fn find_folder(&self, matches: impl Fn(&Folder) -> bool) -> Option<Folder> {
        guard(&self.state).folders.iter().find(|f| matches(f)).cloned()
    }

    fn is_gmail_all(&self, mailbox: &str) -> bool {
        guard(&self.state).gmail_all.as_deref() == Some(mailbox)
    }
}

/// Nouvelle connexion IMAP authentifiée.
#[cfg(not(test))]
fn open_session(config: &crate::config::ServerConfig) -> Result<Session, String> {
    Ok(transport::open_imap(config)?.0)
}

#[cfg(test)]
thread_local! {
    /// Tests (lot E5b) : serveur joué en mémoire à la place du réseau.
    static TEST_SERVER: std::cell::RefCell<Option<Box<dyn Fn() -> Session>>> = std::cell::RefCell::new(None);
}

/// Nouvelle connexion IMAP : le serveur joué du test s'il y en a un.
#[cfg(test)]
fn open_session(config: &crate::config::ServerConfig) -> Result<Session, String> {
    match TEST_SERVER.with(|server| server.borrow().as_ref().map(|open| open())) {
        Some(session) => Ok(session),
        None => Ok(transport::open_imap(config)?.0),
    }
}

/// Sélectionne le dossier seulement s'il ne l'est pas déjà.
/// `APPEND` rejouable sans doublon. `ImapAccount::run` refait l'opération sur
/// une nouvelle connexion quand la précédente est coupée ; si la coupure a eu
/// lieu après que le serveur a accepté le message (mais avant sa réponse), un
/// second `APPEND` le déposerait deux fois (relecture Y8). À la nouvelle
/// tentative, on cherche donc d'abord le message par son `Message-ID`.
fn append_once(
    s: &mut Session,
    attempts: &mut u32,
    mailbox: &str,
    flags: &[String],
    date: Option<i64>,
    raw: &[u8],
) -> imap_lite::Result<Option<u32>> {
    *attempts += 1;
    if *attempts > 1 {
        if let Some(message_id) = compose::header_value(raw, "Message-ID") {
            ensure_selected(s, mailbox, None)?;
            if let Some(&uid) = s.uid_search("HEADER Message-ID", &message_id)?.last() {
                return Ok(Some(uid));
            }
        }
    }
    s.append(mailbox, flags, date, raw)
}

/// Élément `FETCH` qui porte le message complet de `uid`. Un serveur peut
/// glisser avant lui une réponse de drapeaux seuls (changement venu d'un autre
/// appareil, QRESYNC) ou rendre `BODY[] NIL` : un message vide déposé ailleurs
/// remplacerait l'original, qui serait ensuite supprimé (audit B5).
fn complete_item(items: Vec<FetchItem>, uid: u32) -> Result<FetchItem, MailError> {
    let mut announced = false;
    for item in items {
        if item.uid() != Some(uid) {
            continue;
        }
        if item.body("BODY[]").is_some_and(|body| !body.is_empty()) {
            return Ok(item);
        }
        announced = true;
    }
    if announced {
        Err(MailError::Backend(tr!("contenu du message non reçu du serveur").into()))
    } else {
        Err(MailError::NotFound(tr!("message supprimé entre-temps sur le serveur").into()))
    }
}

/// Déplacement dans un compte rejouable sans doublon (audit B1).
/// `ImapAccount::run` refait l'opération sur une nouvelle connexion quand la
/// précédente est coupée. Sans l'extension `MOVE`, `uid_move` fait `COPY`, puis
/// `STORE \Deleted` et `EXPUNGE` : une coupure après la copie la ferait refaire.
/// Au nouvel essai, les messages dont le `Message-ID` est déjà dans la cible ne
/// sont donc que retirés de la source ; ceux sans `Message-ID` sont recopiés.
fn move_once(s: &mut Session, attempts: &mut u32, mailbox: &str, known: Option<u32>, uids: &[u32], target: &str) -> imap_lite::Result<()> {
    *attempts += 1;
    ensure_selected(s, mailbox, known)?;
    if *attempts == 1 || s.has_capability("MOVE") {
        return s.uid_move(uids, target);
    }
    let message_ids = fetch_message_ids(s, uids)?;
    ensure_selected(s, target, None)?;
    let mut copied = Vec::new();
    for (uid, message_id) in &message_ids {
        if !s.uid_search("HEADER Message-ID", message_id)?.is_empty() {
            copied.push(*uid);
        }
    }
    ensure_selected(s, mailbox, known)?;
    let remaining: Vec<u32> = uids.iter().copied().filter(|uid| !copied.contains(uid)).collect();
    if !remaining.is_empty() {
        s.uid_move(&remaining, target)?;
    }
    if !copied.is_empty() {
        s.uid_store(&copied, "+FLAGS.SILENT (\\Deleted)")?;
        s.uid_expunge(&copied)?;
    }
    Ok(())
}

/// Messages disparus du serveur : parmi ceux lus dans le cache **au début**
/// de la synchronisation (`cached`), ceux que le serveur n'a plus. Un message
/// ajouté au cache entre-temps par une autre action (brouillon enregistré,
/// résultat de recherche ouvert) n'est jamais retiré (audit B2).
fn missing_since(cached: &[u32], present: &[u32]) -> Vec<u32> {
    let present: HashSet<u32> = present.iter().copied().collect();
    cached.iter().copied().filter(|uid| !present.contains(uid)).collect()
}

/// Renommage rejouable (audit B8) : `ImapAccount::run` refait l'opération
/// après une coupure ; si le premier `RENAME` avait abouti sur le serveur,
/// le second est refusé. Au nouvel essai, un refus alors que la cible existe
/// et que l'ancien nom a disparu veut dire que c'est fait.
fn rename_once(s: &mut Session, attempts: &mut u32, from: &str, to: &str) -> imap_lite::Result<()> {
    *attempts += 1;
    match s.rename(from, to) {
        Err(imap_lite::Error::No(_)) if *attempts > 1 && s.status(to).is_ok() && s.status(from).is_err() => Ok(()),
        result => result,
    }
}

/// Premier passage complet sur un dossier ? Oui s'il a été renuméroté, ou s'il
/// n'a jamais été synchronisé (pas d'UIDVALIDITY en cache), même si quelques
/// messages y sont déjà (brouillon enregistré, résultat de recherche ouvert) :
/// sinon, seuls les messages plus récents qu'eux seraient listés (relecture Y3).
/// Un serveur qui ne donne pas d'UIDVALIDITY garde la synchronisation habituelle.
fn full_sync(cached: Option<u32>, server: Option<u32>) -> bool {
    match (cached, server) {
        (Some(known), Some(current)) => known != current,
        (None, Some(_)) => true,
        (_, None) => false,
    }
}

/// Nombre de messages à la fin du dossier sélectionné (`exists` messages)
/// déjà tous en cache (`cached` : UID triés). On part du nombre de messages
/// en cache, puis on le réduit tant que le message à cette position laisse
/// au-dessus de lui moins de messages en cache qu'attendu.
fn cached_tail(s: &mut Session, exists: u32, cached: &[u32]) -> imap_lite::Result<u32> {
    let mut tail = u32::try_from(cached.len()).unwrap_or(u32::MAX).min(exists);
    for _ in 0..8 {
        if tail == 0 {
            break;
        }
        let position = exists - tail + 1;
        let Some(uid) = s.fetch(&position.to_string(), "(UID)")?.iter().find_map(FetchItem::uid) else { break };
        let above = u32::try_from(cached.len() - cached.partition_point(|&u| u < uid)).unwrap_or(u32::MAX);
        if above >= tail {
            break;
        }
        tail = above;
    }
    Ok(tail)
}

/// Sélectionne `mailbox` au besoin, puis vérifie que ses UID désignent
/// toujours les messages connus du cache : si le serveur a renuméroté le
/// dossier (`UIDVALIDITY` différente de `known`), une action par UID toucherait
/// d'autres messages ; elle est refusée (RFC 3501 §2.3.1.1, relecture complète
/// n° 2). `None` : rien de connu (recherche par `Message-ID`, dépôt).
fn ensure_selected(s: &mut Session, mailbox: &str, known: Option<u32>) -> imap_lite::Result<()> {
    if s.selected() != Some(mailbox) {
        s.select(mailbox)?;
    }
    match (known, s.selected_uid_validity()) {
        (Some(known), Some(current)) if known != current => Err(imap_lite::Error::No(tr!(
            "Le dossier {mailbox} a été renuméroté par le serveur : action annulée, sa liste va être relue.",
            mailbox = mailbox
        ))),
        _ => Ok(()),
    }
}

fn flags_from(item: &FetchItem) -> Flags {
    Flags { seen: item.has_flag("\\Seen"), flagged: item.has_flag("\\Flagged"), answered: item.has_flag("\\Answered") }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Comment rafraîchir les messages déjà en cache (RFC 7162).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlagPlan {
    /// Tout relire : `UID FETCH min:* (UID FLAGS)`, les absents sont supprimés.
    Full,
    /// HIGHESTMODSEQ et nombre de messages inchangés : rien n'a bougé.
    Unchanged,
    /// QRESYNC : changements et suppressions en une commande (`CHANGEDSINCE … VANISHED`).
    Qresync { since: u64 },
    /// CONDSTORE seul : changements par `CHANGEDSINCE` (si le MODSEQ a bougé),
    /// suppressions par `UID SEARCH UID min:*` (liste d'UID, sans les états).
    Condstore { since: u64, fetch_flags: bool },
}

/// Choisit la synchronisation des messages connus. L'incrémental n'est utilisé
/// que s'il est sûr : serveur CONDSTORE, MODSEQ mémorisé pour le même
/// UIDVALIDITY, dossier sans `NOMODSEQ`, et MODSEQ qui n'a pas reculé.
fn flag_plan(
    cached_validity: Option<u32>,
    stored: Option<(u64, Option<u32>)>,
    info: &SelectInfo,
    condstore: bool,
    qresync: bool,
) -> FlagPlan {
    let Some((since, stored_exists)) = stored else { return FlagPlan::Full };
    let Some(now) = info.highest_modseq else { return FlagPlan::Full };
    if !condstore || info.no_modseq || cached_validity.is_none() || cached_validity != info.uid_validity || now < since {
        return FlagPlan::Full;
    }
    // Un nouveau message augmente le MODSEQ : à MODSEQ égal, seul un EXPUNGE
    // (qui, sans QRESYNC, ne le change pas forcément) ferait baisser EXISTS.
    if now == since && stored_exists == Some(info.exists) {
        return FlagPlan::Unchanged;
    }
    if qresync {
        FlagPlan::Qresync { since }
    } else {
        FlagPlan::Condstore { since, fetch_flags: now != since }
    }
}

/// État des messages déjà en cache, relu sur le serveur.
enum Refresh {
    /// Rien à relire (cache vide, ou UIDVALIDITY changé).
    Nothing,
    /// États de tous les messages à partir du plus ancien en cache.
    Full(Vec<FetchItem>),
    /// Seulement ce qui a changé.
    Delta { changed: Vec<FetchItem>, removed: Removed },
}

enum Removed {
    /// UID supprimés (QRESYNC).
    Vanished(UidSet),
    /// UID encore présents à partir du plus ancien en cache (CONDSTORE seul).
    Present(Vec<u32>),
}

/// Synchronise un dossier : nouveaux messages, états lu/suivi, suppressions.
fn sync_mailbox(account: &ImapAccount, cache: &Mutex<Cache>, mailbox: &str) -> Result<(), MailError> {
    let account_id = account.config.id.clone();
    let mailbox = mailbox.to_string();
    if account.is_gmail_all(&mailbox) {
        return sync_gmail_archive(account, cache, &mailbox);
    }
    let (cached_validity, stored_modseq, cached_uids) = {
        let c = lock_cache(cache);
        (
            c.uid_validity(&account_id, &mailbox).map_err(backend)?,
            c.modseq_state(&account_id, &mailbox).map_err(backend)?,
            c.cached_uids(&account_id, &mailbox).map_err(backend)?,
        )
    };

    let (info, validity_changed, new_items, refresh, condstore) = account
        .run(Lane::Sync, |s| {
            // QRESYNC doit être activé avant toute sélection ; sinon CONDSTORE suffit.
            s.enable_sync_extensions()?;
            let info = s.select(&mailbox)?;
            let validity_changed = matches!((cached_validity, info.uid_validity), (Some(a), Some(b)) if a != b);
            let first_sync = full_sync(cached_validity, info.uid_validity);
            let known: &[u32] = if first_sync { &[] } else { &cached_uids };

            // 1. Messages plus récents que le dernier connu (ou les derniers, au premier passage).
            let new_items = if info.exists == 0 {
                Vec::new()
            } else if let Some(&max) = known.last() {
                if info.uid_next.is_some_and(|next| next <= max.saturating_add(1)) {
                    // UIDNEXT n'a pas dépassé le dernier connu : rien de nouveau.
                    Vec::new()
                } else {
                    // « n:* » renvoie toujours au moins le dernier message : on filtre.
                    s.uid_fetch_set(&format!("{}:*", max.saturating_add(1)), LIST_QUERY)?
                        .into_iter()
                        .filter(|i| i.uid().is_some_and(|u| u > max))
                        .collect()
                }
            } else {
                let start = info.exists.saturating_sub(LIST_LIMIT - 1).max(1);
                s.fetch(&format!("{start}:*"), LIST_QUERY)?
            };

            // 2. État actuel des messages déjà en cache (lu, suivi, encore présents ?).
            let refresh = match known.first() {
                None => Refresh::Nothing,
                Some(_) if info.exists == 0 => Refresh::Full(Vec::new()),
                Some(&min) => {
                    let range = format!("{min}:*");
                    match flag_plan(cached_validity, stored_modseq, &info, s.condstore(), s.qresync()) {
                        FlagPlan::Full => Refresh::Full(s.uid_fetch_set(&range, "(UID FLAGS)")?),
                        FlagPlan::Unchanged => Refresh::Delta { changed: Vec::new(), removed: Removed::Vanished(UidSet::default()) },
                        FlagPlan::Qresync { since } => {
                            let changes = s.uid_fetch_changed(&range, "(UID FLAGS)", since)?;
                            Refresh::Delta { changed: changes.items, removed: Removed::Vanished(changes.vanished) }
                        }
                        FlagPlan::Condstore { since, fetch_flags } => {
                            let changed = if fetch_flags { s.uid_fetch_changed(&range, "(UID FLAGS)", since)?.items } else { Vec::new() };
                            let present = s.uid_search_raw(&format!("UID {range}"))?;
                            Refresh::Delta { changed, removed: Removed::Present(present) }
                        }
                    }
                }
            };
            Ok((info, validity_changed, new_items, refresh, s.condstore()))
        })
        .map_err(backend)?;

    // Analyse des en-têtes avant de prendre le verrou du cache.
    let headers: Vec<Header> = new_items.iter().filter_map(header_from).collect();
    let item_flags = |items: &[FetchItem]| -> Vec<(u32, Flags)> { items.iter().filter_map(|i| Some((i.uid()?, flags_from(i)))).collect() };
    // Une seule transaction (audit P7) : une synchronisation interrompue ne
    // laisse plus le cache à moitié écrit.
    lock_cache(cache)
        .atomically(|c| {
            if validity_changed {
                c.reset_folder(&account_id, &mailbox)?;
            }
            if let Some(v) = info.uid_validity {
                c.set_uid_validity(&account_id, &mailbox, v)?;
            }
            c.upsert_headers(&account_id, &mailbox, &headers)?;
            match refresh {
                Refresh::Nothing => {}
                Refresh::Full(known_items) => {
                    if !cached_uids.is_empty() {
                        let present: Vec<u32> = known_items.iter().filter_map(FetchItem::uid).collect();
                        c.delete_messages(&account_id, &mailbox, &missing_since(&cached_uids, &present))?;
                        c.update_flags(&account_id, &mailbox, &item_flags(&known_items))?;
                    }
                }
                Refresh::Delta { changed, removed } => {
                    // Seuls les messages connus de cette synchronisation peuvent être retirés :
                    // jamais un message ajouté entre-temps au cache par une autre action.
                    let candidates = cached_uids.iter().copied().chain(new_items.iter().filter_map(FetchItem::uid));
                    let gone: Vec<u32> = match removed {
                        Removed::Vanished(set) => candidates.filter(|u| set.contains(*u)).collect(),
                        Removed::Present(present) => {
                            let present: std::collections::HashSet<u32> = present.into_iter().collect();
                            candidates.filter(|u| !present.contains(u)).collect()
                        }
                    };
                    c.delete_messages(&account_id, &mailbox, &gone)?;
                    c.update_flags(&account_id, &mailbox, &item_flags(&changed))?;
                }
            }
            // En dernier, une fois le cache à jour : point de départ de la prochaine fois.
            let modseq = info.highest_modseq.filter(|_| condstore && !info.no_modseq);
            c.set_modseq_state(&account_id, &mailbox, modseq, info.exists)
        })
        .map_err(backend)
}

/// Archive Gmail : « Tous les messages » sans ce qui est encore dans la
/// réception, les envoyés ou les brouillons — sinon tout y figurerait en
/// double. La liste est recalculée à chaque passage (`X-GM-RAW`), car un
/// message archivé dans Gmail garde son UID dans ce dossier.
fn sync_gmail_archive(account: &ImapAccount, cache: &Mutex<Cache>, mailbox: &str) -> Result<(), MailError> {
    const ARCHIVED: &str = "X-GM-RAW \"-in:inbox -in:sent -in:drafts -in:chats\"";
    const FLAGS_LIMIT: usize = 2000;
    let account_id = account.config.id.clone();
    let account_id = account_id.as_str();
    let (cached_validity, cached) = {
        let c = lock_cache(cache);
        (c.uid_validity(account_id, mailbox).map_err(backend)?, c.cached_uids(account_id, mailbox).map_err(backend)?)
    };
    let target = mailbox.to_string();
    let (info, validity_changed, archived, new_items, flag_items) = account
        .run(Lane::Sync, |s| {
            let info = s.select(&target)?;
            let validity_changed = matches!((cached_validity, info.uid_validity), (Some(a), Some(b)) if a != b);
            let known: std::collections::HashSet<u32> =
                if validity_changed { HashSet::default() } else { cached.iter().copied().collect() };
            let archived = s.uid_search_raw(ARCHIVED)?;
            // Les plus récents d'abord ; le reste arrive aux passages suivants.
            let missing: Vec<u32> = archived
                .iter()
                .rev()
                .filter(|u| !known.contains(u))
                .take(usize::try_from(LIST_LIMIT).unwrap_or(usize::MAX))
                .copied()
                .collect();
            let new_items = if missing.is_empty() { Vec::new() } else { s.uid_fetch(&missing, LIST_QUERY)? };
            let keep: Vec<u32> = archived.iter().rev().filter(|u| known.contains(u)).take(FLAGS_LIMIT).copied().collect();
            let flag_items = if keep.is_empty() { Vec::new() } else { s.uid_fetch(&keep, "(UID FLAGS)")? };
            Ok((info, validity_changed, archived, new_items, flag_items))
        })
        .map_err(backend)?;

    let headers: Vec<Header> = new_items.iter().filter_map(header_from).collect();
    // Une seule transaction, comme `sync_mailbox` (audit P7).
    lock_cache(cache)
        .atomically(|c| {
            if validity_changed {
                c.reset_folder(account_id, mailbox)?;
            }
            if let Some(v) = info.uid_validity {
                c.set_uid_validity(account_id, mailbox, v)?;
            }
            c.upsert_headers(account_id, mailbox, &headers)?;
            if !validity_changed {
                // Revenu dans la réception (ou supprimé) : retiré de l'archive.
                let archived: std::collections::HashSet<u32> = archived.into_iter().collect();
                let gone: Vec<u32> = cached.iter().filter(|u| !archived.contains(u)).copied().collect();
                c.delete_messages(account_id, mailbox, &gone)?;
                let flags: Vec<(u32, Flags)> = flag_items.iter().filter_map(|i| Some((i.uid()?, flags_from(i)))).collect();
                c.update_flags(account_id, mailbox, &flags)?;
            }
            // Liste recalculée à chaque passage : pas de synchronisation incrémentale ici.
            c.set_modseq_state(account_id, mailbox, None, info.exists)
        })
        .map_err(backend)
}

/// Arborescence d'un compte relue sur le serveur et enregistrée dans le cache.
fn refresh_account_folders(account: &ImapAccount, cache: &Mutex<Cache>) -> Option<AccountError> {
    let id = account.config.id.clone();
    let result = account.run(Lane::Sync, |s| {
        // Abonnements : si le serveur refuse LSUB, tout est considéré comme abonné.
        let subscribed = match s.lsub() {
            Ok(list) => Some(list.into_iter().map(|e| e.name).collect::<Vec<_>>()),
            Err(e) if e.is_connection_lost() => return Err(e),
            Err(_) => None,
        };
        let entries = s.list()?;
        // Compteurs de tous les dossiers en un seul aller-retour.
        let names: Vec<String> = entries.iter().filter(|e| e.selectable()).map(|e| e.name.clone()).collect();
        let mut statuses = s.status_many(&names)?.into_iter();
        let out: Vec<_> = entries
            .into_iter()
            .map(|entry| {
                let status = if entry.selectable() { statuses.next().flatten() } else { None };
                (entry, status)
            })
            .collect();
        let gmail = s.has_capability("X-GM-EXT-1");
        Ok((out, subscribed, gmail))
    });
    match result {
        Ok((entries, subscribed, gmail)) => {
            let gmail_all = gmail.then(|| entries.iter().find(|(e, _)| e.has_attribute("\\All")).map(|(e, _)| e.name.clone())).flatten();
            let mut folders = build_folders(&id, entries);
            if let Some(all) = &gmail_all {
                // Ses non-lus sont ceux de la réception : pas de compteur trompeur.
                if let Some(f) = folders.iter_mut().find(|f| &f.path == all) {
                    f.unread = 0;
                }
            }
            if let Some(names) = subscribed {
                mark_subscriptions(&mut folders, &names);
            }
            let saved = lock_cache(cache).save_folders(&id, &folders);
            *guard(&account.state) = AccountState { folders, gmail_all };
            saved.err().map(|message| AccountError { account_id: id, message })
        }
        Err(message) => Some(AccountError { account_id: id, message }),
    }
}

/// Gmail : « Tous les messages » (`\All`) est le seul dossier de rôle Archive.
fn gmail_archive(config: &AccountConfig, folders: &[Folder]) -> Option<String> {
    crate::google::is_google_host(&config.imap.host)
        .then(|| folders.iter().find(|f| f.role == FolderRole::Archive).map(|f| f.path.clone()))
        .flatten()
}

/// Rôle d'un dossier : attributs SPECIAL-USE, sinon noms usuels.
/// Rôle annoncé par le serveur : `INBOX`, ou attribut SPECIAL-USE (RFC 6154).
fn role_by_attribute(entry: &ListEntry) -> Option<FolderRole> {
    if entry.name.eq_ignore_ascii_case("INBOX") {
        return Some(FolderRole::Inbox);
    }
    for (attr, role) in [
        ("\\Sent", FolderRole::Sent),
        ("\\Drafts", FolderRole::Drafts),
        ("\\Trash", FolderRole::Trash),
        ("\\Junk", FolderRole::Junk),
        ("\\Archive", FolderRole::Archive),
        // Gmail : « Tous les messages » sert d'archive (comme dans Thunderbird).
        ("\\All", FolderRole::Archive),
    ] {
        if entry.has_attribute(attr) {
            return Some(role);
        }
    }
    None
}

/// Rôle deviné d'après le nom, pour un serveur sans SPECIAL-USE : seulement
/// au premier niveau ou juste sous `INBOX` (un `Clients/Archive` reste un
/// dossier ordinaire).
fn role_by_name(entry: &ListEntry) -> Option<FolderRole> {
    if entry.parent().is_some_and(|p| !p.eq_ignore_ascii_case("INBOX")) {
        return None;
    }
    let role = match entry.leaf_name().to_lowercase().as_str() {
        "sent" | "sent items" | "sent messages" | "sent mail" | "envoyés" | "messages envoyés" | "éléments envoyés" => {
            FolderRole::Sent
        }
        "drafts" | "draft" | "brouillons" => FolderRole::Drafts,
        "trash" | "deleted items" | "deleted messages" | "corbeille" | "éléments supprimés" => FolderRole::Trash,
        "junk" | "spam" | "junk e-mail" | "courrier indésirable" | "indésirables" => FolderRole::Junk,
        "archive" | "archives" => FolderRole::Archive,
        _ => return None,
    };
    Some(role)
}

fn role_rank(role: FolderRole) -> u8 {
    match role {
        FolderRole::Inbox => 0,
        FolderRole::Drafts => 1,
        FolderRole::Sent => 2,
        FolderRole::Archive => 3,
        FolderRole::Trash => 4,
        FolderRole::Junk => 5,
        FolderRole::Custom => 6,
    }
}

/// Nom d'un dossier spécial dans la langue de l'interface ; `None` pour un dossier personnel.
/// Appliqué à la lecture du cache aussi (lot E3) : le nom suit la langue choisie.
pub(crate) fn role_name(role: FolderRole) -> Option<&'static str> {
    Some(match role {
        FolderRole::Inbox => tr!("Boîte de réception"),
        FolderRole::Drafts => tr!("Brouillons"),
        FolderRole::Sent => tr!("Éléments envoyés"),
        FolderRole::Archive => tr!("Archive"),
        FolderRole::Trash => tr!("Éléments supprimés"),
        FolderRole::Junk => tr!("Courrier indésirable"),
        FolderRole::Custom => return None,
    })
}

/// Construit l'arborescence affichable à partir de `LIST` + `STATUS`.
/// Abonnements : la réception et les dossiers spéciaux restent toujours visibles.
fn mark_subscriptions(folders: &mut [Folder], subscribed: &[String]) {
    for f in folders.iter_mut() {
        f.subscribed = f.role != FolderRole::Custom || f.path.eq_ignore_ascii_case("INBOX") || subscribed.contains(&f.path);
    }
}

/// Niveaux de dossiers gardés, au plus (audit S7) : un serveur qui annonce
/// des milliers de niveaux ne fait pas déborder la pile de l'arborescence.
const MAX_FOLDER_DEPTH: u32 = 64;

fn build_folders(account_id: &str, entries: Vec<(ListEntry, Option<MailboxStatus>)>) -> Vec<Folder> {
    fn visit(
        parent: Option<&str>,
        depth: u32,
        nodes: &[(ListEntry, Option<MailboxStatus>, FolderRole)],
        parent_of: &dyn Fn(&ListEntry) -> Option<String>,
        account_id: &str,
        out: &mut Vec<Folder>,
    ) {
        if depth >= MAX_FOLDER_DEPTH {
            return;
        }
        for (entry, status, role) in nodes {
            if parent_of(entry).as_deref() != parent {
                continue;
            }
            let st = status.clone().unwrap_or_default();
            out.push(Folder {
                id: format!("{account_id}:{}", entry.name),
                account_id: account_id.to_string(),
                name: role_name(*role).map_or_else(|| entry.leaf_name(), str::to_string),
                path: entry.name.clone(),
                parent_id: parent.map(|p| format!("{account_id}:{p}")),
                depth,
                role: *role,
                unread: st.unseen,
                total: st.messages,
                subscribed: true,
            });
            visit(Some(&entry.name), depth + 1, nodes, parent_of, account_id, out);
        }
    }

    // Un seul dossier par rôle spécial. Les attributs du serveur passent
    // d'abord, quel que soit l'ordre du `LIST` ; le nom ne sert que pour les
    // rôles restés libres (relecture complète n° 6 : un dossier personnel
    // « Trash » listé avant le dossier `\Trash` ne lui prend plus son rôle).
    let mut taken: Vec<FolderRole> = Vec::new();
    let mut roles: Vec<FolderRole> = entries
        .iter()
        .map(|(e, _)| match role_by_attribute(e) {
            Some(role) if !taken.contains(&role) => {
                taken.push(role);
                role
            }
            _ => FolderRole::Custom,
        })
        .collect();
    for ((e, _), role) in entries.iter().zip(roles.iter_mut()) {
        if *role != FolderRole::Custom || role_by_attribute(e).is_some() {
            continue;
        }
        if let Some(by_name) = role_by_name(e).filter(|r| !taken.contains(r)) {
            taken.push(by_name);
            *role = by_name;
        }
    }
    let mut nodes: Vec<(ListEntry, Option<MailboxStatus>, FolderRole)> =
        entries.into_iter().zip(roles).map(|((e, st), role)| (e, st, role)).collect();
    // Clé calculée une fois par dossier : le nom décodé coûte une allocation.
    nodes.sort_by_cached_key(|n| (role_rank(n.2), n.0.decoded_name().to_lowercase()));

    let names: Vec<String> = nodes.iter().map(|n| n.0.name.clone()).collect();
    let parent_of = |e: &ListEntry| e.parent().filter(|p| names.contains(p));

    // Parcours en profondeur : chaque parent suivi de ses enfants.
    let mut out = Vec::with_capacity(nodes.len());
    visit(None, 0, &nodes, &parent_of, account_id, &mut out);
    out
}

/// Verrou du cache ; un verrou « empoisonné » par une panique reste utilisable.
///
/// Sûr parce que toute écriture dans SQLite passe par une transaction, annulée
/// si une panique l'interrompt, et que les tables de numéros en mémoire
/// (`Cache::id_for`) changent en instructions qui ne paniquent pas. Règle : pas
/// de modification en plusieurs étapes hors d'une transaction (lot 10).
///
/// Avec `VORTEX_MESURE` (lot E2), une attente de 50 ms ou plus est notée, avec
/// l'endroit qui attendait.
#[track_caller]
pub(crate) fn lock_cache(cache: &Mutex<Cache>) -> MutexGuard<'_, Cache> {
    if !crate::diag::measuring() {
        return cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    let caller = std::panic::Location::caller();
    let since = std::time::Instant::now();
    let guard = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let waited = since.elapsed().as_millis();
    if waited >= 50 {
        crate::diag::step(&format_args!("cache attendu {waited} ms ({}:{})", caller.file(), caller.line()));
    }
    guard
}

/// Début du texte lu pour l'aperçu, coupé à la dernière fin de ligne quand
/// il a été tronqué (pas de ligne base64 coupée au milieu).
fn preview_sample(text: &[u8]) -> &[u8] {
    if text.len() < PREVIEW_BYTES {
        return text;
    }
    text.iter().rposition(|&b| b == b'\n').map_or(text, |end| &text[..=end])
}

/// Décode une partie restée en base64 : `mail-parser` ne décode pas une
/// partie dont la fin manque (début du message seulement), et l'aperçu
/// affichait le base64 brut (« `DQoNCl9fX19`… »).
fn decoded_part(text: &mut Option<String>) {
    if let Some(decoded) = text.as_deref().and_then(mimeparse::decode_if_base64) {
        *text = Some(decoded);
    }
}

/// Ferme les parties multiples d'un début de message coupé (lot H1) : sans
/// leur délimiteur final, l'aperçu d'une réponse d'invitation, dont le
/// calendrier dépasse les 2 048 octets lus, montrait les délimiteurs et les
/// en-têtes des parties (« `--=_vortex_… Content-Type…` »).
fn close_multiparts(sample: &mut Vec<u8>) {
    let text = String::from_utf8_lossy(sample).into_owned();
    let lower = text.to_ascii_lowercase();
    let mut boundaries: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(at) = lower.get(from..).and_then(|rest| rest.find("boundary=")) {
        let start = from + at + "boundary=".len();
        let rest = text.get(start..).unwrap_or_default();
        let value = match rest.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next().unwrap_or_default(),
            None => rest.split(|c: char| c == ';' || c.is_whitespace()).next().unwrap_or_default(),
        };
        if !value.is_empty() && value.len() <= 200 && !boundaries.iter().any(|b| b == value) {
            boundaries.push(value.to_string());
        }
        from = start;
    }
    for boundary in boundaries.iter().rev() {
        sample.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    }
}

/// Message décodé pour l'aperçu de la liste, à partir de son début seulement.
fn parse_for_preview(sample: &[u8]) -> mimeparse::Parsed {
    let mut sample = sample.to_vec();
    close_multiparts(&mut sample);
    let mut parsed = mimeparse::parse(&sample);
    decoded_part(&mut parsed.text);
    decoded_part(&mut parsed.html);
    parsed
}

/// En-têtes lus par `HEADER.FIELDS`, toujours suivis de leur ligne vide
/// (lot O). Sans elle, le début du texte (délimiteur et en-têtes de la
/// première partie) était lu comme la suite des en-têtes du message, et
/// l'aperçu devenait « test --=_vortex_… Content-Type: text/html… ».
fn header_block(headers: &[u8]) -> Vec<u8> {
    let mut block = headers.to_vec();
    if !(block.ends_with(b"\r\n\r\n") || block.ends_with(b"\n\n")) {
        if !block.is_empty() && !block.ends_with(b"\n") {
            block.extend_from_slice(b"\r\n");
        }
        block.extend_from_slice(b"\r\n");
    }
    block
}

/// En-tête à mettre en cache, depuis une réponse FETCH de la liste.
fn header_from(item: &FetchItem) -> Option<Header> {
    let uid = item.uid()?;
    let headers = item.body("BODY[HEADER").unwrap_or_default();
    let mut sample = header_block(headers);
    if let Some(text) = item.body("BODY[TEXT]") {
        sample.extend_from_slice(preview_sample(text));
    }
    let parsed = parse_for_preview(&sample);
    Some(Header {
        uid,
        from_name: parsed.from_name.clone(),
        from_email: parsed.from_email.clone(),
        subject: parsed.subject.clone(),
        preview: mimeparse::preview(&parsed),
        date: item.internal_date().or(parsed.date).unwrap_or(0),
        flags: flags_from(item),
        has_attachments: mimeparse::looks_like_attachment(headers),
    })
}

fn backend(e: String) -> MailError {
    if is_network_error(&e) {
        MailError::Network(e)
    } else {
        MailError::Backend(e)
    }
}

/// Taille totale maximale des pièces jointes d'un envoi.
pub(crate) const MAX_ATTACHMENTS: usize = 25 * 1024 * 1024;

/// Pièce jointe à envoyer : fichier choisi (base64) ou pièce d'un message existant (transfert).
/// Sérialisée pour rendre un message à la rédaction après un envoi échoué (lot E1).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInput {
    pub name: String,
    #[serde(default)]
    pub content_type: String,
    pub data: Option<String>,
    pub message_id: Option<MessageId>,
    pub index: Option<u32>,
}

/// Compte rendu d'un envoi.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendReport {
    pub recipients: u32,
    /// Dossier où la copie a été déposée.
    pub saved_in: Option<String>,
    pub warning: Option<String>,
}

/// Partagé entre toutes les commandes sans verrou global : la liste des
/// comptes n'est verrouillée que le temps d'en copier les références, et
/// chaque compte porte ses propres verrous (voir `ImapAccount`).
pub struct Router {
    accounts: RwLock<Vec<Arc<ImapAccount>>>,
    cache: Arc<Mutex<Cache>>,
    /// Téléchargement en arrière-plan pour la lecture hors connexion.
    prefetch: Mutex<Option<Prefetch>>,
    /// Dernier message entier lu sur le serveur (audit P5).
    last_raw: Mutex<Option<LastRaw>>,
}

/// Plus gros message gardé en mémoire après sa lecture (audit P5).
const LAST_RAW_MAX: usize = 30 << 20;

/// Dernier message entier lu sur le serveur, gardé pour ses pièces jointes
/// (audit P5) : « Tout enregistrer » sur un message de 20 Mo à cinq pièces ne
/// télécharge plus 100 Mo. Un UID ne désigne qu'un seul message tant que
/// l'`UIDVALIDITY` du dossier ne change pas : il fait partie de la clé, et un
/// message dont le dossier n'a pas d'`UIDVALIDITY` connue n'est pas gardé.
struct LastRaw {
    account: String,
    mailbox: String,
    uid: u32,
    validity: u32,
    raw: Arc<[u8]>,
}

impl LastRaw {
    fn is(&self, r: &ImapRef, validity: u32) -> bool {
        self.account == r.account && self.mailbox == r.mailbox && self.uid == r.uid && self.validity == validity
    }
}

/// Ne prend aucun verrou (un `Debug` ne doit pas pouvoir bloquer).
impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router").finish_non_exhaustive()
    }
}

/// Tour de téléchargement : dossiers restants, déjà synchronisés, déjà complétés.
struct Prefetch {
    queue: std::collections::VecDeque<FolderId>,
    synced: std::collections::HashSet<FolderId>,
    backfilled: std::collections::HashSet<FolderId>,
    resync: bool,
    /// Messages sans corps par dossier : comptés au début du tour, puis
    /// recomptés seulement pour le dossier de chaque étape (audit P2).
    counts: std::collections::HashMap<FolderId, u32>,
}

impl Prefetch {
    fn remaining(&self) -> u32 {
        self.counts.values().fold(0, |total, n| total.saturating_add(*n))
    }
}

/// Remplacement d'un brouillon chiffré par une version en clair (audit S4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlainReplacement {
    /// Refusé : le chiffrement a disparu sans que l'utilisateur le demande
    /// (certificat devenu indisponible, autre compte d'envoi).
    Refused,
    /// Demandé : l'utilisateur a désactivé « Chiffrer » dans la rédaction.
    Confirmed,
}

/// Brouillon enregistré dans « Brouillons ».
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedDraft {
    pub id: MessageId,
    pub folder_id: FolderId,
}

/// Pièce jointe d'un brouillon repris, avec son contenu (base64).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftAttachment {
    pub name: String,
    pub content_type: String,
    pub size: u64,
    pub data: String,
}

/// Contenu d'un brouillon à reprendre dans la fenêtre de rédaction.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftContent {
    pub id: MessageId,
    pub account_id: String,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub html: String,
    pub plain_text: bool,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub attachments: Vec<DraftAttachment>,
    /// Brouillon chiffré : la rédaction reprend avec « Chiffrer » enfoncé (lot D4).
    pub encrypt: bool,
}

/// Résultats d'une recherche sur les serveurs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchReport {
    pub results: Vec<MessageSummary>,
    pub errors: Vec<AccountError>,
    /// Dossiers non parcourus : la recherche s'est arrêtée au bout de
    /// [`SEARCH_BUDGET`] (lot J14), les résultats peuvent être incomplets.
    pub unfinished: usize,
}

/// Durée d'une recherche sur les serveurs, au plus (lot J14, décision du
/// 19 novembre 2026 : 15 s d'abord, puis 30 s après essai, « raisonnable »). Les résultats trouvés
/// sont alors rendus, et les dossiers restants comptés.
const SEARCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// Rang d'un dossier dans une recherche sur les serveurs : les plus utiles
/// d'abord, pour que des résultats partiels soient les bons.
fn search_rank(role: FolderRole) -> u8 {
    match role {
        FolderRole::Inbox => 0,
        FolderRole::Sent => 1,
        FolderRole::Archive => 2,
        FolderRole::Drafts => 3,
        _ => 4,
    }
}

/// Étape de la recherche d'un compte, envoyée à [`Router::search`].
enum SearchStep {
    /// Dossiers à parcourir pour ce compte.
    Planned(usize),
    /// Un dossier parcouru : ses messages trouvés (compte, dossier, rôle), s'il y en a.
    Searched(Option<(String, String, FolderRole, Vec<FetchItem>)>),
    /// Compte injoignable : ses dossiers restants ne sont pas comptés comme non parcourus.
    Failed { account_id: String, message: String, remaining: usize },
    /// Ce compte a fini (ou le temps est écoulé).
    Done,
}

/// Bilan d'un import de messages (lot E8a).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Messages déposés dans le dossier.
    pub imported: u32,
    /// Messages refusés par le serveur.
    pub failed: u32,
    /// Raison du dernier refus.
    pub error: Option<String>,
    /// Import arrêté après plusieurs refus de suite (serveur injoignable…).
    pub stopped: bool,
}

/// Bilan de « Restaurer » (messages remis hors d'« Éléments supprimés »).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    /// Déplacements faits, mis en attente ou en échec.
    pub report: MoveReport,
    /// Messages sans dossier d'origine connu, remis dans la boîte de réception.
    pub to_inbox: u32,
}

/// Messages d'un même dossier d'« Éléments supprimés » à restaurer.
struct TrashGroup {
    account: String,
    mailbox: String,
    /// (identifiant, UID dans ce dossier).
    members: Vec<(MessageId, u32)>,
}

/// Messages à remettre dans un même dossier par « Restaurer ».
struct RestoreTarget {
    folder: FolderId,
    ids: Vec<MessageId>,
    /// Notes à oublier une fois restaurés : (`Message-ID`, dossier).
    notes: Vec<(String, String)>,
}

/// `Message-ID` des messages `uids` du dossier sélectionné : (UID, `Message-ID`) ;
/// les messages qui n'en ont pas sont absents de la liste.
fn fetch_message_ids(s: &mut Session, uids: &[u32]) -> imap_lite::Result<Vec<(u32, String)>> {
    let items = s.uid_fetch(uids, "(UID BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])")?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let uid = item.uid()?;
            let message_id = compose::header_value(item.body("BODY[HEADER")?, "Message-ID")?;
            let message_id = message_id.trim();
            (uids.contains(&uid) && !message_id.is_empty()).then(|| (uid, message_id.to_string()))
        })
        .collect())
}

/// Bilan d'une reprise des actions en attente.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlushReport {
    /// Actions (ou déplacements interrompus) menées à bien.
    pub done: u32,
    /// Messages encore en attente (réseau toujours absent).
    pub remaining: u32,
}

/// Corps HTML d'un document complet : ce qui est entre `<body …>` et `</body>`.
fn body_fragment(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<body").and_then(|i| lower[i..].find('>').map(|j| i + j + 1));
    let end = lower.rfind("</body>");
    match (start, end) {
        (Some(s), Some(e)) if s <= e => html[s..e].trim().to_string(),
        _ => html.to_string(),
    }
}

/// `a@x, b@y` → `a@x; b@y` (format du champ de saisie).
fn address_field(raw: &[u8], header: &str) -> String {
    compose::header_value(raw, header).map_or_default(|v| compose::parse_address_list(&v).join("; "))
}

/// Avancement du mode hors connexion.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefetchReport {
    pub fetched: u32,
    /// Messages connus dont le contenu reste à télécharger.
    pub remaining: u32,
    pub done: bool,
}

/// Messages téléchargés par étape (garde le routeur disponible pour l'interface).
const PREFETCH_BATCH: usize = 10;

/// Contenu d'un message brut : analyse, HTML affichable, informations de
/// réponse et signature S/MIME vérifiée maintenant (lot D1). Un message signé
/// « opaque » s'affiche par le contenu que porte sa signature.
fn body_record(raw: &[u8]) -> (mimeparse::Parsed, String, BodyExtra) {
    // Message chiffré (lot D3a) : rien en clair dans le cache, seulement le
    // message chiffré, déchiffré en mémoire à chaque ouverture (`Router::open`).
    let encrypted = smime::is_encrypted(raw);
    let opened = if encrypted { None } else { smime::opaque_content(raw) };
    let mut parsed = mimeparse::parse(opened.as_deref().unwrap_or(raw));
    let signature = if encrypted { cache::ENCRYPTED.to_string() } else { smime::column(raw, now()) };
    if encrypted {
        parsed.attachments.clear();
        parsed.text = None;
        parsed.html = None;
    } else if signature != cache::UNSIGNED {
        // Signature S/MIME montrée par sa barre : sa partie `smime.p7s` n'est pas
        // une pièce jointe (les autres gardent leur numéro).
        parsed.attachments.retain(|a| !smime::is_signature_part(&a.content_type));
    }
    let html = if encrypted { String::new() } else { mimeparse::display_html(&parsed) };
    let addresses = |name: &str| compose::header_value(raw, name).map_or_default(|v| compose::parse_address_list(&v));
    let extra = BodyExtra {
        to_list: addresses("To"),
        cc_list: addresses("Cc"),
        reply_to: addresses("Reply-To"),
        message_id: compose::header_value(raw, "Message-ID").unwrap_or_default(),
        // Sans `References`, l'`In-Reply-To` du message tient lieu de fil
        // (RFC 5322 §3.6.4 ; relecture complète n° 13).
        references: compose::header_value(raw, "References")
            .filter(|r| !r.trim().is_empty())
            .or_else(|| compose::header_value(raw, "In-Reply-To"))
            .unwrap_or_default(),
        attachments: parsed.attachments.clone(),
        images_allowed: false,
        signature,
        ciphertext: if encrypted { raw.to_vec() } else { Vec::new() },
        sender_check: crate::authres::verdict(raw).to_owned(),
    };
    (parsed, html, extra)
}

impl Router {
    /// Les arborescences sont reprises du cache : affichage immédiat au démarrage.
    pub fn new(configs: Vec<AccountConfig>, cache: Arc<Mutex<Cache>>) -> Self {
        let accounts = configs.into_iter().map(|c| Arc::new(ImapAccount::new(c))).collect();
        let router = Self { accounts: RwLock::new(accounts), cache, prefetch: Mutex::new(None), last_raw: Mutex::new(None) };
        router.reload_folders();
        router.publish_accounts();
        router
    }

    /// Arborescences relues dans le cache. Le routeur d'arrière-plan (lecture
    /// hors connexion) s'en sert : c'est le routeur principal qui les met à jour.
    pub fn reload_folders(&self) {
        let c = lock_cache(&self.cache);
        for a in self.all_accounts() {
            let folders = c.folders(&a.config.id).unwrap_or_default();
            let mut st = guard(&a.state);
            if st.gmail_all.is_none() {
                st.gmail_all = gmail_archive(&a.config, &folders);
            }
            st.folders = folders;
        }
    }

    /// Les comptes, copiés (références) : la liste n'est verrouillée qu'un instant.
    fn all_accounts(&self) -> Vec<Arc<ImapAccount>> {
        self.accounts.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// `UIDVALIDITY` du dossier selon le cache : celle des UID qu'il mémorise.
    fn known_validity(&self, account: &str, mailbox: &str) -> Option<u32> {
        self.cache().uid_validity(account, mailbox).ok().flatten()
    }

    fn cache(&self) -> MutexGuard<'_, Cache> {
        lock_cache(&self.cache)
    }

    /// Recopie la liste des comptes dans le cache, lisible sans ce verrou-ci.
    fn publish_accounts(&self) {
        if let Err(e) = self.cache().set_accounts(&self.accounts()) {
            crate::diag::report(&e);
        }
    }

    // ------------------------------------------------------------ comptes

    pub fn configs(&self) -> Vec<AccountConfig> {
        self.all_accounts().iter().map(|a| a.config.clone()).collect()
    }

    pub fn config(&self, id: &str) -> Option<AccountConfig> {
        self.all_accounts().iter().find(|a| a.config.id == id).map(|a| a.config.clone())
    }

    pub fn add_account(&self, config: AccountConfig) {
        self.accounts.write().unwrap_or_else(std::sync::PoisonError::into_inner).push(Arc::new(ImapAccount::new(config)));
        self.publish_accounts();
    }

    /// Remplace la configuration d'un compte. Le serveur a pu changer : le
    /// cache du compte est vidé et sera reconstruit à la synchronisation.
    pub fn update_account(&self, config: AccountConfig) -> bool {
        let id = config.id.clone();
        let (old, server_changed) = {
            let mut list = self.accounts.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(slot) = list.iter_mut().find(|a| a.config.id == id) else {
                return false;
            };
            let server_changed = slot.config.imap.host != config.imap.host || slot.config.imap.username != config.imap.username;
            let fresh = ImapAccount::new(config);
            if !server_changed {
                let st = guard(&slot.state);
                *guard(&fresh.state) = AccountState { folders: st.folders.clone(), gmail_all: st.gmail_all.clone() };
            }
            (std::mem::replace(slot, Arc::new(fresh)), server_changed)
        };
        if server_changed {
            if let Err(e) = self.cache().delete_account(&id) {
                crate::diag::report(&e);
            }
            *guard(&self.last_raw) = None;
        }
        self.publish_accounts();
        // Anciennes connexions fermées une fois leurs opérations terminées.
        old.close();
        true
    }

    pub fn remove_account(&self, id: &str) -> bool {
        let old = {
            let mut list = self.accounts.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(pos) = list.iter().position(|a| a.config.id == id) else {
                return false;
            };
            list.remove(pos)
        };
        if let Err(e) = self.cache().delete_account(id) {
            crate::diag::report(&e);
        }
        *guard(&self.last_raw) = None;
        self.publish_accounts();
        old.close();
        true
    }

    pub fn accounts(&self) -> Vec<Account> {
        self.all_accounts()
            .iter()
            .map(|a| Account {
                id: a.config.id.clone(),
                name: a.config.name.clone(),
                email: a.config.email.clone(),
                color: a.config.color.clone(),
            })
            .collect()
    }

    fn imap_account(&self, account_id: &str) -> Result<Arc<ImapAccount>, MailError> {
        self.all_accounts()
            .into_iter()
            .find(|a| a.config.id == account_id)
            .ok_or_else(|| MailError::NotFound(tr!("compte {account}", account = account_id)))
    }

    fn split_folder(folder_id: &str) -> Result<(String, String), MailError> {
        folder_id
            .split_once(':')
            .map(|(a, m)| (a.to_string(), m.to_string()))
            .ok_or_else(|| MailError::NotFound(tr!("dossier {folder}", folder = folder_id)))
    }

    fn imap_ref(&self, id: MessageId) -> Result<ImapRef, MailError> {
        self.cache().imap_ref(id).ok_or_else(|| MailError::NotFound(tr!("message #{id}", id = id)))
    }

    // ------------------------------------------------------------ synchronisation

    /// Relit l'arborescence de chaque compte (LIST + STATUS) et l'enregistre.
    /// Les erreurs sont rendues par compte, sans bloquer les autres.
    pub fn refresh_folders(&self) -> Vec<AccountError> {
        let cache = Arc::clone(&self.cache);
        // Chaque compte a sa propre connexion : ils sont interrogés en parallèle.
        let accounts = self.all_accounts();
        std::thread::scope(|scope| {
            let handles: Vec<_> = accounts
                .iter()
                .map(|account| {
                    let cache = Arc::clone(&cache);
                    scope.spawn(move || refresh_account_folders(account, &cache))
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().unwrap_or(None)).collect()
        })
    }

    /// Synchronise un dossier : nouveaux messages, états lu/suivi, suppressions.
    pub fn sync_folder(&self, folder_id: &str) -> Result<(), MailError> {
        let (account_id, mailbox) = Self::split_folder(folder_id)?;
        let cache = Arc::clone(&self.cache);
        let account = self.imap_account(&account_id)?;
        sync_mailbox(&account, &cache, &mailbox)
    }

    /// Synchronise plusieurs dossiers (vue unifiée) : un fil par compte, les
    /// comptes en parallèle. Résultats dans l'ordre des dossiers demandés.
    pub fn sync_many(&self, folder_ids: &[String]) -> Vec<Result<(), MailError>> {
        let cache = Arc::clone(&self.cache);
        let mut results: Vec<Option<Result<(), MailError>>> = folder_ids.iter().map(|_| None).collect();
        let mut wanted: Vec<(usize, String, String)> = Vec::new();
        for (i, id) in folder_ids.iter().enumerate() {
            match Self::split_folder(id) {
                Ok((account, mailbox)) => wanted.push((i, account, mailbox)),
                Err(e) => results[i] = Some(Err(e)),
            }
        }
        let accounts = self.all_accounts();
        let done: Vec<(usize, Result<(), MailError>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = accounts
                .iter()
                .filter_map(|account| {
                    let mine: Vec<(usize, String)> =
                        wanted.iter().filter(|(_, a, _)| *a == account.config.id).map(|(i, _, m)| (*i, m.clone())).collect();
                    if mine.is_empty() {
                        return None;
                    }
                    let cache = Arc::clone(&cache);
                    Some(scope.spawn(move || mine.into_iter().map(|(i, m)| (i, sync_mailbox(account, &cache, &m))).collect::<Vec<_>>()))
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        });
        for (i, r) in done {
            results[i] = Some(r);
        }
        results
            .into_iter()
            .zip(folder_ids)
            .map(|(r, id)| r.unwrap_or_else(|| Err(MailError::NotFound(tr!("compte du dossier {folder}", folder = id)))))
            .collect()
    }

    // ------------------------------------------------------------ lecture d'un message

    fn fetch_full(&self, r: &ImapRef) -> Result<FetchItem, MailError> {
        let (mailbox, uid) = (r.mailbox.clone(), r.uid);
        let known = self.known_validity(&r.account, &r.mailbox);
        let items = self
            .imap_account(&r.account)?
            .run(Lane::User, |s| {
                ensure_selected(s, &mailbox, known)?;
                s.uid_fetch(&[uid], FULL_QUERY)
            })
            .map_err(backend)?;
        complete_item(items, uid)
    }

    /// Message entier : celui gardé en mémoire s'il s'agit du même (audit P5),
    /// sinon relu sur le serveur, puis gardé.
    fn full_raw(&self, r: &ImapRef) -> Result<Arc<[u8]>, MailError> {
        let validity = self.known_validity(&r.account, &r.mailbox);
        if let Some(validity) = validity {
            if let Some(last) = guard(&self.last_raw).as_ref().filter(|last| last.is(r, validity)) {
                return Ok(Arc::clone(&last.raw));
            }
        }
        let item = self.fetch_full(r)?;
        let raw: Arc<[u8]> = Arc::from(item.body("BODY[]").unwrap_or_default());
        self.remember_raw(r, validity, &raw);
        Ok(raw)
    }

    /// Garde `raw` comme dernier message lu, s'il n'est pas trop gros et que
    /// l'`UIDVALIDITY` de son dossier est connue (audit P5).
    fn remember_raw(&self, r: &ImapRef, validity: Option<u32>, raw: &Arc<[u8]>) {
        let Some(validity) = validity else { return };
        if raw.len() > LAST_RAW_MAX {
            return;
        }
        let last = LastRaw { account: r.account.clone(), mailbox: r.mailbox.clone(), uid: r.uid, validity, raw: Arc::clone(raw) };
        *guard(&self.last_raw) = Some(last);
    }

    /// Télécharge un message absent du cache, l'affiche et le garde pour la suite.
    pub fn message(&self, id: MessageId) -> Result<Message, MailError> {
        let r = self.imap_ref(id)?;
        let validity = self.known_validity(&r.account, &r.mailbox);
        let item = self.fetch_full(&r)?;
        let raw = item.body("BODY[]").unwrap_or_default();
        // Ses pièces jointes s'ouvrent ensuite sans le relire (audit P5).
        self.remember_raw(&r, validity, &Arc::from(raw));
        let (parsed, html, extra) = body_record(raw);

        let summary = MessageSummary {
            id,
            folder_id: format!("{}:{}", r.account, r.mailbox),
            account_id: r.account.clone(),
            from_name: parsed.from_name.clone(),
            from_email: parsed.from_email.clone(),
            subject: parsed.subject.clone(),
            preview: mimeparse::preview(&parsed),
            date: item.internal_date().or(parsed.date).unwrap_or(0),
            flags: flags_from(&item),
            has_attachments: !parsed.attachments.is_empty(),
            uid: r.uid,
        };
        let mut c = self.cache();
        if let Err(e) = c.save_body(&r.account, &r.mailbox, r.uid, &parsed.to, &html, &extra) {
            crate::diag::report(&e);
        }
        let cached = c.header(&r.account, &r.mailbox, r.uid).ok().flatten();
        // L'aperçu de la liste vient d'un début de message ; le message
        // entier, une fois ouvert, donne le bon.
        if let Some(mut header) = cached.filter(|h| h.preview != summary.preview) {
            header.preview.clone_from(&summary.preview);
            let _ = c.upsert_headers(&r.account, &r.mailbox, &[header]);
        } else if c.header(&r.account, &r.mailbox, r.uid).ok().flatten().is_none() {
            let header = Header {
                uid: r.uid,
                from_name: summary.from_name.clone(),
                from_email: summary.from_email.clone(),
                subject: summary.subject.clone(),
                preview: summary.preview.clone(),
                date: summary.date,
                flags: summary.flags,
                has_attachments: summary.has_attachments,
            };
            let _ = c.upsert_headers(&r.account, &r.mailbox, &[header]);
        }
        Ok(Message {
            summary,
            to: parsed.to,
            body_html: html,
            to_list: extra.to_list,
            cc_list: extra.cc_list,
            reply_to: extra.reply_to,
            message_id: extra.message_id,
            references: extra.references,
            attachments: extra.attachments,
            images_allowed: false,
            body_simple: String::new(),
            body_text: String::new(),
            signature: if extra.signature == cache::UNSIGNED { String::new() } else { extra.signature },
            encryption: String::new(),
            sender_check: extra.sender_check,
        })
    }

    /// Message prêt à montrer : un message chiffré (lot D3a) est déchiffré ici,
    /// en mémoire, depuis le chiffré gardé dans le cache (ou relu sur le
    /// serveur) ; son contenu n'est jamais écrit dans le cache. Les autres
    /// sont rendus tels quels.
    pub fn open(&self, id: MessageId, mut message: Message) -> Message {
        if message.signature != cache::ENCRYPTED {
            return message;
        }
        message.signature = String::new();
        let raw = match self.cache().ciphertext(id) {
            Ok(Some(raw)) => raw,
            _ => match self.imap_ref(id).and_then(|r| self.fetch_full(&r)) {
                Ok(item) => item.body("BODY[]").unwrap_or_default().to_vec(),
                Err(e) => {
                    message.encryption = e.to_string();
                    return message;
                }
            },
        };
        match smime::decrypt(&self.configs(), &message.summary.account_id, &raw) {
            Ok(shown) => {
                let (parsed, html, extra) = body_record(&shown);
                message.body_html = html;
                message.attachments = extra.attachments;
                message.signature = if extra.signature == cache::UNSIGNED { String::new() } else { extra.signature };
                if message.summary.preview.is_empty() {
                    message.summary.preview = mimeparse::preview(&parsed);
                }
                message.encryption = smime::DECRYPTED.to_string();
            }
            Err(e) => message.encryption = e,
        }
        message
    }

    /// Signature S/MIME d'un message déjà en cache, examinée de nouveau (lot D1) :
    /// « Vérifier de nouveau », ou premier examen d'un corps gardé avant le lot
    /// D1. Seul un message annoncé signé ou chiffré est téléchargé de nouveau ;
    /// au premier examen, son corps est refait (contenu d'une signature opaque).
    ///
    /// # Errors
    ///
    /// Message inconnu ou disparu du serveur, serveur injoignable, cache illisible.
    pub fn check_signature(&self, id: MessageId) -> Result<(), MailError> {
        let r = self.imap_ref(id)?;
        let (mailbox, uid) = (r.mailbox.clone(), r.uid);
        let known = self.known_validity(&r.account, &r.mailbox);
        let items = self
            .imap_account(&r.account)?
            .run(Lane::User, |s| {
                ensure_selected(s, &mailbox, known)?;
                s.uid_fetch(&[uid], TYPE_QUERY)
            })
            .map_err(backend)?;
        let item = items
            .iter()
            .find(|i| i.uid() == Some(uid))
            .ok_or_else(|| MailError::NotFound(tr!("message supprimé entre-temps sur le serveur").into()))?;
        if !smime::announced(item.body("BODY[HEADER").unwrap_or_default()) {
            return self.cache().set_signature(id, cache::UNSIGNED).map_err(MailError::Backend);
        }
        let first = self.cache().message(id).map_err(MailError::Backend)?.is_none_or(|m| m.signature == cache::NOT_EXAMINED);
        let item = self.fetch_full(&r)?;
        let raw = item.body("BODY[]").unwrap_or_default();
        // Message chiffré : gardé chiffré (sa signature, à l'intérieur, se lit à
        // l'ouverture).
        if first || smime::is_encrypted(raw) {
            let (parsed, html, extra) = body_record(raw);
            self.cache().save_body(&r.account, &r.mailbox, r.uid, &parsed.to, &html, &extra).map_err(MailError::Backend)
        } else {
            // Corps déjà fait au lot D1 : on garde les images autorisées.
            self.cache().set_signature(id, &smime::column(raw, now())).map_err(MailError::Backend)
        }
    }

    // ------------------------------------------------------------ hors connexion

    /// Dossiers lisibles hors connexion : tous sauf corbeille et indésirables,
    /// la réception d'abord.
    fn offline_folders(&self) -> Vec<Folder> {
        let mut folders: Vec<Folder> = self
            .all_accounts()
            .iter()
            .flat_map(|a| a.folders())
            .filter(|f| !matches!(f.role, FolderRole::Trash | FolderRole::Junk) && f.total > 0)
            .collect();
        folders.sort_by_key(|f| role_rank(f.role));
        folders
    }

    /// Messages du dossier `folder_id` encore sans corps (à télécharger : un
    /// compte sans lecture hors connexion n'en a aucun, lot J11b).
    fn count_without_body(&self, folder_id: &str) -> u32 {
        let Ok((account_id, mailbox)) = Self::split_folder(folder_id) else { return 0 };
        if !self.offline_bodies(&account_id) {
            return 0;
        }
        self.cache().count_without_body(&account_id, &mailbox).unwrap_or(0)
    }

    /// Le contenu des messages du compte est-il téléchargé d'avance (lot J11b) ?
    fn offline_bodies(&self, account_id: &str) -> bool {
        self.imap_account(account_id).is_ok_and(|a| a.config.offline_bodies)
    }

    /// Reste à télécharger après une étape sur `folder_id` : seul ce dossier a
    /// pu changer, les autres gardent leur compte (audit P2 : chaque étape
    /// recomptait tous les dossiers, verrou du cache tenu).
    fn remaining_after(&self, st: &mut Prefetch, folder_id: &str) -> u32 {
        st.counts.insert(folder_id.to_string(), self.count_without_body(folder_id));
        st.remaining()
    }

    /// Complète les en-têtes plus anciens que ceux en cache (par tranches).
    /// Renvoie `true` s'il en reste.
    fn backfill_folder(&self, folder_id: &str, chunk: u32) -> Result<bool, MailError> {
        let (account_id, mailbox) = Self::split_folder(folder_id)?;
        // Archive Gmail : complétée par chaque synchronisation (voir `sync_gmail_archive`).
        if self.imap_account(&account_id)?.is_gmail_all(&mailbox) {
            return Ok(false);
        }
        let cached = self.cache().cached_uids(&account_id, &mailbox).map_err(backend)?;
        if cached.is_empty() {
            return Ok(false);
        }
        let target = mailbox.clone();
        let fetched = self
            .imap_account(&account_id)?
            .run(Lane::Sync, |s| {
                let info = s.select(&target)?;
                // Messages en cache qui forment la fin du dossier : un ancien
                // message isolé (ouvert depuis une recherche) n'en fait pas
                // partie, sinon la tranche suivante commencerait trop bas et
                // un message ne serait jamais listé (relecture complète n° 17).
                let older = info.exists.saturating_sub(cached_tail(s, info.exists, &cached)?);
                if older == 0 {
                    return Ok(None);
                }
                let start = older.saturating_sub(chunk - 1).max(1);
                Ok(Some((start, s.fetch(&format!("{start}:{older}"), LIST_QUERY)?)))
            })
            .map_err(backend)?;
        let Some((start, items)) = fetched else { return Ok(false) };
        let headers: Vec<Header> =
            items.iter().filter(|i| i.uid().is_some_and(|u| cached.binary_search(&u).is_err())).filter_map(header_from).collect();
        if headers.is_empty() {
            return Ok(false);
        }
        self.cache().upsert_headers(&account_id, &mailbox, &headers).map_err(backend)?;
        Ok(start > 1)
    }

    /// Une étape du téléchargement hors connexion : au plus une opération réseau
    /// (synchroniser un dossier, compléter ses en-têtes, ou récupérer quelques corps).
    /// `resync` : resynchroniser chaque dossier pendant ce tour.
    pub fn prefetch_step(&self, resync: bool) -> Result<PrefetchReport, MailError> {
        // Un tour à la fois (routeur d'arrière-plan : rien d'autre n'attend ce verrou).
        let mut tour = guard(&self.prefetch);
        if tour.is_none() {
            self.reload_folders();
            let queue: std::collections::VecDeque<FolderId> = self.offline_folders().into_iter().map(|f| f.id).collect();
            let counts = queue.iter().map(|id| (id.clone(), self.count_without_body(id))).collect();
            *tour = Some(Prefetch { queue, synced: HashSet::default(), backfilled: HashSet::default(), resync, counts });
        }
        loop {
            let st = tour.as_mut().expect("tour en cours");
            let Some(folder_id) = st.queue.front().cloned() else {
                let remaining = st.remaining();
                *tour = None;
                return Ok(PrefetchReport { fetched: 0, remaining, done: true });
            };
            let do_sync = st.resync && !st.synced.contains(&folder_id);
            let do_backfill = st.resync && !do_sync && !st.backfilled.contains(&folder_id);
            if do_sync {
                st.synced.insert(folder_id.clone());
                if let Err(e) = self.sync_folder(&folder_id) {
                    crate::diag::report(&format_args!("hors connexion, {folder_id} : {e}"));
                    st.queue.pop_front();
                }
                return Ok(PrefetchReport { fetched: 0, remaining: self.remaining_after(st, &folder_id), done: false });
            }
            if do_backfill {
                match self.backfill_folder(&folder_id, LIST_LIMIT) {
                    Ok(true) => {}
                    Ok(false) => {
                        st.backfilled.insert(folder_id.clone());
                    }
                    Err(e) => {
                        crate::diag::report(&format_args!("hors connexion, {folder_id} : {e}"));
                        st.backfilled.insert(folder_id.clone());
                    }
                }
                return Ok(PrefetchReport { fetched: 0, remaining: self.remaining_after(st, &folder_id), done: false });
            }

            let (account_id, mailbox) = Self::split_folder(&folder_id)?;
            // Compte sans lecture hors connexion : en-têtes seulement (lot J11b).
            if !self.offline_bodies(&account_id) {
                st.queue.pop_front();
                continue;
            }
            let uids = self.cache().uids_without_body(&account_id, &mailbox, PREFETCH_BATCH).map_err(backend)?;
            if uids.is_empty() {
                st.queue.pop_front();
                continue;
            }
            let target = mailbox.clone();
            let known = self.known_validity(&account_id, &mailbox);
            let items = match self.imap_account(&account_id)?.run(Lane::Sync, |s| {
                ensure_selected(s, &target, known)?;
                s.uid_fetch(&uids, FULL_QUERY)
            }) {
                Ok(items) => items,
                Err(e) => {
                    // Réseau coupé : on s'arrête, l'interface relancera plus tard.
                    *tour = None;
                    return Err(backend(e));
                }
            };
            // Analyse des messages hors verrou : l'affichage n'attend pas. Un corps
            // vide n'est pas gardé : le message sera relu au passage suivant (audit B5).
            let records: Vec<_> = items
                .iter()
                .filter_map(|item| Some((item.uid()?, body_record(item.body("BODY[]").filter(|body| !body.is_empty())?))))
                .collect();
            let mut fetched = 0;
            {
                let mut c = self.cache();
                for (uid, (parsed, html, extra)) in &records {
                    if c.save_body(&account_id, &mailbox, *uid, &parsed.to, html, extra).is_ok() {
                        fetched += 1;
                    }
                }
            }
            let Some(st) = tour.as_mut() else { return Ok(PrefetchReport { fetched, remaining: 0, done: false }) };
            // Messages disparus du serveur entre-temps : on passe au dossier suivant.
            if fetched == 0 {
                st.queue.pop_front();
            }
            return Ok(PrefetchReport { fetched, remaining: self.remaining_after(st, &folder_id), done: false });
        }
    }

    /// Contenu d'une pièce jointe, lu dans le dernier message gardé en mémoire
    /// ou relu sur le serveur (audit P5).
    pub fn attachment(&self, id: MessageId, index: u32) -> Result<mimeparse::AttachmentData, MailError> {
        let r = self.imap_ref(id)?;
        let whole = self.full_raw(&r)?;
        let raw: &[u8] = &whole;
        // Message chiffré (lot D3a) : pièces jointes lues dans le contenu déchiffré,
        // en mémoire.
        let decrypted = if smime::is_encrypted(raw) {
            Some(smime::decrypt(&self.configs(), &r.account, raw).map_err(MailError::InvalidTarget)?)
        } else {
            None
        };
        let raw = decrypted.as_deref().unwrap_or(raw);
        // Pièces jointes d'un message signé « opaque » : dans sa signature (lot D1).
        let opened = smime::opaque_content(raw);
        mimeparse::attachment(opened.as_deref().unwrap_or(raw), index)
            .ok_or_else(|| MailError::NotFound(tr!("pièce jointe n° {index}", index = index)))
    }

    /// Pièce jointe prête à partir ; `input` est seulement lu (le message entier
    /// reste disponible pour être rendu à la rédaction si l'envoi échoue, lot E5).
    fn resolve_attachment(&self, input: &AttachmentInput) -> Result<OutgoingAttachment, MailError> {
        match (&input.data, input.message_id, input.index) {
            (Some(data), _, _) => Ok(OutgoingAttachment {
                data: compose::base64_decode(data).map_err(|e| MailError::InvalidTarget(tr!("{name} : {e}", name = input.name, e = e)))?,
                name: input.name.clone(),
                content_type: input.content_type.clone(),
            }),
            (None, Some(id), Some(index)) => {
                let a = self.attachment(id, index)?;
                Ok(OutgoingAttachment { name: a.name, content_type: a.content_type, data: a.data })
            }
            _ => Err(MailError::InvalidTarget(tr!("pièce jointe incomplète : {name}", name = input.name))),
        }
    }

    // ------------------------------------------------------------ dossiers

    /// Crée un dossier (dans `parent_path` s'il est donné), s'y abonne et
    /// renvoie son identifiant.
    pub fn create_folder(&self, account_id: &str, parent_path: Option<&str>, name: &str) -> Result<FolderId, MailError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(MailError::InvalidTarget(tr!("Nom de dossier vide.").into()));
        }
        let delimiter = self.imap_account(account_id)?.run(Lane::User, imap_lite::Client::delimiter).map_err(backend)?;
        if name.contains(delimiter) {
            return Err(MailError::InvalidTarget(tr!("Le nom ne peut pas contenir « {delimiter} ».", delimiter = delimiter)));
        }
        let leaf = imap_lite::utf7::encode(name);
        let path = match parent_path {
            Some(parent) => format!("{parent}{delimiter}{leaf}"),
            None => leaf,
        };
        let target = path.clone();
        self.imap_account(account_id)?
            .run(Lane::User, |s| {
                s.create(&target)?;
                // Abonné, pour qu'il apparaisse aussi en vue unifiée.
                let _ = s.subscribe(&target);
                Ok(())
            })
            .map_err(backend)?;
        Ok(format!("{account_id}:{path}"))
    }

    /// Renomme un dossier personnel ; renvoie son nouvel identifiant.
    pub fn rename_folder(&self, folder_id: &str, name: &str) -> Result<FolderId, MailError> {
        let (account_id, path) = Self::split_folder(folder_id)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(MailError::InvalidTarget(tr!("Nom de dossier vide.").into()));
        }
        self.check_custom(folder_id)?;
        let delimiter = self.imap_account(&account_id)?.run(Lane::User, imap_lite::Client::delimiter).map_err(backend)?;
        if name.contains(delimiter) {
            return Err(MailError::InvalidTarget(tr!("Le nom ne peut pas contenir « {delimiter} ».", delimiter = delimiter)));
        }
        let leaf = imap_lite::utf7::encode(name);
        let new_path = match path.rsplit_once(delimiter) {
            Some((parent, _)) => format!("{parent}{delimiter}{leaf}"),
            None => leaf,
        };
        self.rename_path(&account_id, &path, &new_path, delimiter)
    }

    /// Déplace un dossier personnel sous `new_parent` (chemin), ou au premier niveau.
    pub fn move_folder(&self, folder_id: &str, new_parent: Option<&str>) -> Result<FolderId, MailError> {
        let (account_id, path) = Self::split_folder(folder_id)?;
        self.check_custom(folder_id)?;
        let delimiter = self.imap_account(&account_id)?.run(Lane::User, imap_lite::Client::delimiter).map_err(backend)?;
        if let Some(parent) = new_parent {
            if parent == path || parent.starts_with(&format!("{path}{delimiter}")) {
                return Err(MailError::InvalidTarget(tr!("Un dossier ne peut pas aller dans lui-même.").into()));
            }
        }
        let leaf = path.rsplit_once(delimiter).map_or_else(|| path.clone(), |(_, l)| l.to_string());
        let new_path = match new_parent {
            Some(parent) => format!("{parent}{delimiter}{leaf}"),
            None => leaf,
        };
        if new_path == path {
            return Ok(folder_id.to_string());
        }
        self.rename_path(&account_id, &path, &new_path, delimiter)
    }

    /// Seuls les dossiers personnels se renomment ou se déplacent.
    fn check_custom(&self, folder_id: &str) -> Result<(), MailError> {
        let folder = self.folder(folder_id).ok_or_else(|| MailError::NotFound(folder_id.into()))?;
        if folder.role != FolderRole::Custom || folder.path.eq_ignore_ascii_case("INBOX") {
            return Err(MailError::InvalidTarget(
                tr!("Les dossiers spéciaux (réception, envoyés…) ne peuvent pas être renommés ni déplacés.").into(),
            ));
        }
        Ok(())
    }

    /// `RENAME` sur le serveur (sous-dossiers compris), abonnements et cache mis à jour.
    fn rename_path(&self, account_id: &str, old: &str, new: &str, delimiter: char) -> Result<FolderId, MailError> {
        let prefix = format!("{old}{delimiter}");
        let children: Vec<String> =
            self.imap_account(account_id)?.folders().iter().filter(|f| f.path.starts_with(&prefix)).map(|f| f.path.clone()).collect();
        let (from, to) = (old.to_string(), new.to_string());
        let mut attempts = 0;
        self.imap_account(account_id)?
            .run(Lane::User, |s| {
                // Un serveur peut refuser de renommer le dossier ouvert.
                if s.selected().is_some_and(|m| m == from || m.starts_with(&prefix)) {
                    s.select("INBOX")?;
                }
                rename_once(s, &mut attempts, &from, &to)?;
                // Abonnements : sous le nouveau nom, pour rester visible (vue unifiée, Thunderbird…).
                for path in std::iter::once(&from).chain(children.iter()) {
                    let moved = format!("{to}{}", &path[from.len()..]);
                    let _ = s.unsubscribe(path);
                    let _ = s.subscribe(&moved);
                }
                Ok(())
            })
            .map_err(backend)?;
        if let Err(e) = self.cache().rename_path(account_id, old, new, delimiter) {
            crate::diag::report(&e);
        }
        Ok(format!("{account_id}:{new}"))
    }

    /// Supprime un dossier personnel vide de sous-dossiers (ses messages avec).
    pub fn delete_folder(&self, folder_id: &str) -> Result<(), MailError> {
        let (account_id, path) = Self::split_folder(folder_id)?;
        let folder = self.folder(folder_id).ok_or_else(|| MailError::NotFound(folder_id.into()))?;
        if folder.role != FolderRole::Custom || path.eq_ignore_ascii_case("INBOX") {
            return Err(MailError::InvalidTarget(tr!("Les dossiers spéciaux ne peuvent pas être supprimés.").into()));
        }
        let has_children = self.imap_account(&account_id)?.folders().iter().any(|f| f.parent_id.as_deref() == Some(folder_id));
        if has_children {
            return Err(MailError::InvalidTarget(tr!("Supprimez d'abord ses sous-dossiers.").into()));
        }
        let target = path.clone();
        self.imap_account(&account_id)?
            .run(Lane::User, |s| {
                // Un serveur peut refuser de supprimer le dossier ouvert.
                if s.selected() == Some(target.as_str()) {
                    s.select("INBOX")?;
                }
                let _ = s.unsubscribe(&target);
                s.delete(&target)
            })
            .map_err(backend)?;
        if let Err(e) = self.cache().reset_folder(&account_id, &path) {
            crate::diag::report(&e);
        }
        Ok(())
    }

    /// Vide définitivement « Éléments supprimés » ou « Courrier indésirable ».
    pub fn empty_folder(&self, folder_id: &str) -> Result<u32, MailError> {
        let (account_id, path) = Self::split_folder(folder_id)?;
        let role = self.folder(folder_id).map(|f| f.role);
        if !matches!(role, Some(FolderRole::Trash | FolderRole::Junk)) {
            return Err(MailError::InvalidTarget(
                tr!("Seuls les éléments supprimés et le courrier indésirable peuvent être vidés.").into(),
            ));
        }
        let target = path.clone();
        let removed = self.imap_account(&account_id)?.run(Lane::User, |s| s.empty_mailbox(&target)).map_err(backend)?;
        if let Err(e) = self.cache().reset_folder(&account_id, &path) {
            crate::diag::report(&e);
        }
        Ok(removed)
    }

    /// Écrit tous les messages d'un dossier dans `out`, au format mbox (lot
    /// E8a), du plus ancien au plus récent, par tranches de 16 Mo ou de 50
    /// messages au plus (audit P7 : 50 messages entiers pouvaient faire des
    /// centaines de Mo en mémoire) : la connexion est rendue entre deux
    /// tranches, les autres actions ne l'attendent pas. Renvoie le nombre de
    /// messages écrits.
    ///
    /// # Errors
    /// Dossier inconnu, serveur injoignable, ou écriture du fichier impossible.
    pub fn export_folder(&self, folder_id: &str, out: &mut impl std::io::Write) -> Result<u32, MailError> {
        let (account_id, mailbox) = Self::split_folder(folder_id)?;
        let account = self.imap_account(&account_id)?;
        let known = self.known_validity(&account_id, &mailbox);
        let sizes: Vec<(u32, u32)> = account
            .run(Lane::User, |s| {
                ensure_selected(s, &mailbox, known)?;
                let uids = s.uid_search_raw("ALL")?;
                // Tailles seulement : quelques octets par message.
                if uids.is_empty() {
                    Ok(Vec::new())
                } else {
                    s.uid_fetch(&uids, "(UID RFC822.SIZE)")
                }
            })
            .map_err(backend)?
            .iter()
            .filter_map(|item| Some((item.uid()?, item.size().unwrap_or(0))))
            .collect();
        let mut written = 0u32;
        for chunk in export_chunks(sizes, EXPORT_CHUNK_BYTES, EXPORT_CHUNK_COUNT) {
            let mut items = account
                .run(Lane::User, |s| {
                    ensure_selected(s, &mailbox, known)?;
                    s.uid_fetch(&chunk, "(UID INTERNALDATE BODY.PEEK[])")
                })
                .map_err(backend)?;
            items.sort_by_key(FetchItem::uid);
            for item in &items {
                let raw = item.body("BODY[]").unwrap_or_default();
                let sender = envelope_sender(raw);
                mail_core::mbox::write_message(out, raw, &sender, item.internal_date().unwrap_or_else(now))
                    .map_err(|e| MailError::Backend(tr!("Écriture du fichier impossible : {e}", e = e)))?;
                written += 1;
            }
        }
        Ok(written)
    }

    /// Dépose dans un dossier les messages d'un fichier `.eml` ou `.mbox` (lot
    /// E8a), marqués lus, à leur date d'envoi (`Date`, sinon maintenant). Un
    /// message refusé n'arrête pas les suivants ; trois refus de suite, si.
    ///
    /// # Errors
    /// Fichier sans message, ou dossier inconnu.
    pub fn import_messages(&self, folder_id: &str, data: &[u8]) -> Result<ImportReport, MailError> {
        let messages = mail_core::mbox::split(data);
        if messages.is_empty() {
            return Err(MailError::InvalidTarget(tr!("Aucun message dans ce fichier.").into()));
        }
        Self::split_folder(folder_id)?;
        let mut report = ImportReport::default();
        let mut refused_in_a_row = 0;
        for bytes in messages {
            let internal_date = crate::mimeparse::parse(&bytes).date.unwrap_or_else(now);
            let raw = RawMessage { bytes, flags: Flags { seen: true, flagged: false, answered: false }, internal_date };
            match self.append(folder_id, &raw) {
                Ok(_) => {
                    report.imported += 1;
                    refused_in_a_row = 0;
                }
                Err(e) => {
                    report.failed += 1;
                    report.error = Some(e.to_string());
                    refused_in_a_row += 1;
                    if refused_in_a_row >= 3 {
                        report.stopped = true;
                        break;
                    }
                }
            }
        }
        Ok(report)
    }

    /// Remet des messages d'« Éléments supprimés » dans le dossier d'où Vortex
    /// Mail les avait supprimés ; sans dossier d'origine connu (supprimés
    /// ailleurs, note trop ancienne, dossier disparu), dans la boîte de
    /// réception de leur compte. Les `Message-ID` sont lus sur le serveur :
    /// hors connexion, les messages du dossier sont rendus en échec (rien
    /// n'est deviné) ; une coupure pendant les déplacements les met en attente
    /// comme les autres. Plusieurs messages au même `Message-ID` (message
    /// envoyé à soi-même) retournent chacun dans un dossier noté différent.
    ///
    /// # Errors
    ///
    /// Message hors d'« Éléments supprimés » ([`MailError::InvalidTarget`]) ou
    /// introuvable ; rien n'est alors déplacé.
    pub fn restore(&self, journal: &Mutex<Journal>, ids: &[MessageId]) -> Result<RestoreReport, MailError> {
        let mut sources: Vec<TrashGroup> = Vec::new();
        for &id in ids {
            let r = self.imap_ref(id)?;
            let role = self.folder(&format!("{}:{}", r.account, r.mailbox)).map(|f| f.role);
            if role != Some(FolderRole::Trash) {
                return Err(MailError::InvalidTarget(tr!("Seuls les éléments supprimés peuvent être restaurés.").into()));
            }
            match sources.iter_mut().find(|g| g.account == r.account && g.mailbox == r.mailbox) {
                Some(g) => g.members.push((id, r.uid)),
                None => sources.push(TrashGroup { account: r.account, mailbox: r.mailbox, members: vec![(id, r.uid)] }),
            }
        }
        let mut out = RestoreReport::default();
        for TrashGroup { account, mailbox, members } in sources {
            let uids: Vec<u32> = members.iter().map(|m| m.1).collect();
            let known = self.known_validity(&account, &mailbox);
            let found = self.imap_account(&account).map_err(|e| e.to_string()).and_then(|a| {
                a.run(Lane::User, |s| {
                    ensure_selected(s, &mailbox, known)?;
                    fetch_message_ids(s, &uids)
                })
            });
            // Serveur injoignable : ces messages restent où ils sont, les autres comptes continuent.
            let found = match found {
                Ok(found) => found,
                Err(reason) => {
                    out.report.failed.extend(members.iter().map(|&(id, _)| FailedMove { id, reason: reason.clone() }));
                    continue;
                }
            };
            let inbox = self.folder_by_role(&account, FolderRole::Inbox).unwrap_or_else(|| format!("{account}:INBOX"));
            let mut targets: Vec<RestoreTarget> = Vec::new();
            // Notes déjà prises dans ce passage : deux copies au même `Message-ID`
            // retournent chacune dans son dossier.
            let mut taken: Vec<(String, String)> = Vec::new();
            for (id, uid) in members {
                let message_id = found.iter().find(|f| f.0 == uid).map(|f| f.1.clone());
                let origin = message_id.as_ref().and_then(|m| {
                    let notes = self.cache().trash_origins(&account, m).unwrap_or_default();
                    let usable: Vec<String> = notes
                        .into_iter()
                        .filter(|path| self.folder(&format!("{account}:{path}")).is_some_and(|f| f.role != FolderRole::Trash))
                        .collect();
                    let path = usable.iter().find(|p| !taken.iter().any(|t| &t.0 == m && &t.1 == *p)).or(usable.first())?.clone();
                    taken.push((m.clone(), path.clone()));
                    Some((m.clone(), path))
                });
                let folder = if let Some((_, path)) = &origin {
                    format!("{account}:{path}")
                } else {
                    out.to_inbox = out.to_inbox.saturating_add(1);
                    inbox.clone()
                };
                match targets.iter_mut().find(|t| t.folder == folder) {
                    Some(t) => {
                        t.ids.push(id);
                        t.notes.extend(origin);
                    }
                    None => targets.push(RestoreTarget { folder, ids: vec![id], notes: origin.into_iter().collect() }),
                }
            }
            for RestoreTarget { folder, ids: restored, notes } in targets {
                let report = self.move_or_queue(journal, &restored, Some(folder.as_str()), None)?;
                // Note gardée en cas d'échec : un nouvel essai retrouvera le dossier.
                let forgotten = if report.failed.is_empty() { self.cache().trash_origins_forget(&account, &notes) } else { Ok(()) };
                if let Err(e) = forgotten {
                    crate::diag::report(&e);
                }
                let all = &mut out.report;
                all.same_account = all.same_account.saturating_add(report.same_account);
                all.cross_account = all.cross_account.saturating_add(report.cross_account);
                all.queued = all.queued.saturating_add(report.queued);
                all.moved.extend(report.moved);
                all.failed.extend(report.failed);
            }
        }
        Ok(out)
    }

    /// Supprime définitivement des messages de la corbeille ou du courrier
    /// indésirable (`\Deleted` puis `UID EXPUNGE`), comme « Supprimer » dans
    /// ces dossiers chez Thunderbird (relecture complète n° 40). Renvoie le
    /// nombre de messages supprimés ; les autres dossiers sont refusés.
    pub fn delete_forever(&self, ids: &[MessageId]) -> Result<u32, MailError> {
        let groups = self.by_folder(ids)?;
        for (account, mailbox, _) in &groups {
            let role = self.folder(&format!("{account}:{mailbox}")).map(|f| f.role);
            if !matches!(role, Some(FolderRole::Trash | FolderRole::Junk)) {
                return Err(MailError::InvalidTarget(
                    tr!("Seuls les éléments supprimés et le courrier indésirable peuvent être supprimés définitivement.").into(),
                ));
            }
        }
        // Une commande par dossier (découpée par `imap-lite`), et non plus un
        // `STORE` et un `EXPUNGE` par message (audit P4).
        let mut removed = 0u32;
        for (account, mailbox, uids) in groups {
            let known = self.known_validity(&account, &mailbox);
            self.imap_account(&account)?
                .run(Lane::User, |s| {
                    ensure_selected(s, &mailbox, known)?;
                    s.uid_store(&uids, "+FLAGS.SILENT (\\Deleted)")?;
                    s.uid_expunge(&uids)
                })
                .map_err(backend)?;
            self.cache().delete_messages(&account, &mailbox, &uids).map_err(backend)?;
            removed = removed.saturating_add(u32::try_from(uids.len()).unwrap_or(u32::MAX));
        }
        Ok(removed)
    }

    /// Messages regroupés par compte et dossier, dans l'ordre de `ids`.
    fn by_folder(&self, ids: &[MessageId]) -> Result<Vec<(String, String, Vec<u32>)>, MailError> {
        let mut groups: Vec<(String, String, Vec<u32>)> = Vec::new();
        let mut index: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
        for &id in ids {
            let r = self.imap_ref(id)?;
            let key = (r.account.clone(), r.mailbox.clone());
            if let Some(group) = index.get(&key).and_then(|&i| groups.get_mut(i)) {
                group.2.push(r.uid);
            } else {
                index.insert(key, groups.len());
                groups.push((r.account, r.mailbox, vec![r.uid]));
            }
        }
        Ok(groups)
    }

    // ------------------------------------------------------------ envoi

    /// Envoie un message depuis un compte, puis en dépose une copie dans ses
    /// « Éléments envoyés ». Un échec de cette copie n'annule pas l'envoi.
    ///
    /// `draft` et `attachments` sont seulement lus : en cas d'échec, l'appelant
    /// les rend tels quels à la rédaction, sans en avoir gardé de copie (lot E5).
    pub fn send(
        &self,
        account_id: &str,
        draft: &Draft,
        answered: Option<MessageId>,
        attachments: &[AttachmentInput],
        draft_id: Option<MessageId>,
    ) -> Result<SendReport, MailError> {
        let mut draft = draft.clone();
        if let Some(id) = draft_id {
            self.ensure_draft(id)?;
        }
        let config = self.config(account_id).ok_or_else(|| MailError::NotFound(tr!("compte {account}", account = account_id)))?;
        draft.from_email.clone_from(&config.email);
        draft.from_name.clone_from(&config.display_name);
        draft.validate().map_err(MailError::InvalidTarget)?;
        // Domaines internationaux en Punycode (relecture complète n° 15) ;
        // partie locale accentuée gardée pour SMTPUTF8 (lot H2).
        draft.wire_addresses();
        for input in attachments {
            let file = self.resolve_attachment(input)?;
            draft.attachments.push(file);
        }
        let total: usize = draft.attachments.iter().map(|a| a.data.len()).sum();
        if total > MAX_ATTACHMENTS {
            return Err(MailError::InvalidTarget(tr!(
                "Pièces jointes trop volumineuses : {size} Mo (limite {limit} Mo).",
                size = total.div_ceil(1024 * 1024),
                limit = MAX_ATTACHMENTS / (1024 * 1024)
            )));
        }

        // Même clé d'envoi (nouvel essai depuis la même fenêtre) : même
        // `Message-ID`, pour que le doublon éventuel soit reconnu (n° 9).
        let random = match compose::send_key_bytes(&draft.send_key) {
            Some(key) => key,
            None => crate::oauth::random_bytes::<16>().map_err(MailError::Backend)?,
        };
        let message_id = compose::new_message_id(&config.email, &random);
        let date = now();
        draft.utc_offset = crate::tz::local_offset(date);
        // Copie gardée dans « Éléments envoyés » : avec ses copies cachées,
        // pour que l'expéditeur sache à qui il a écrit (relecture complète n° 28).
        // Message signé (lot D2) : une seule signature pour les deux, la copie
        // cachée restant hors de la partie signée.
        let (raw, kept) = if draft.encrypt {
            // Chiffré (lot D3b), signé d'abord si demandé : pour chaque destinataire,
            // copies cachées comprises, et pour l'expéditeur (sa copie).
            let recipients = draft.recipients();
            let mut sign = |content: &[u8]| crate::smime::sign(&config, content, date);
            let signer: Option<compose::Signer<'_, String>> = if draft.sign { Some(&mut sign) } else { None };
            compose::build_encrypted(&draft, date, &message_id, signer, |content| {
                crate::smime::encrypt(&config, content, &recipients, date)
            })
            .map_err(MailError::InvalidTarget)?
        } else if draft.sign {
            compose::build_signed(&draft, date, &message_id, |content| crate::smime::sign(&config, content, date))
                .map_err(MailError::InvalidTarget)?
        } else {
            (compose::build(&draft, date, &message_id), compose::build_draft(&draft, date, &message_id))
        };
        let recipients = draft.recipients();
        let smtp = config.effective_smtp();
        // Outlook.com a coupé l'envoi SMTP de certaines boîtes (comptes récents) :
        // le même message part alors par l'API, Cci compris dans ses en-têtes
        // (le service s'en sert comme destinataires et ne les transmet pas).
        let by_api = match transport::send_smtp(&smtp, &config.email, &recipients, &compose::smtp_data(&raw)) {
            Ok(_) => false,
            Err(e) if transport::smtp_disabled_by_service(&smtp, &e) => {
                let token = crate::oauth::send_token(smtp.username.trim()).map_err(backend)?;
                crate::o365::send_mime(&token, &kept).map_err(backend)?;
                true
            }
            Err(e) => return Err(backend(e)),
        };

        let mut report = SendReport { recipients: u32::try_from(recipients.len()).unwrap_or(u32::MAX), saved_in: None, warning: None };
        // Outlook.com range lui-même dans « Éléments envoyés » ce qui part par
        // son API comme par son serveur SMTP : une copie de plus ferait doublon
        // (relecture complète n° 28).
        let service_keeps_copy = by_api || transport::is_microsoft_host(&smtp.host);
        match self.folder_by_role(account_id, FolderRole::Sent) {
            Some(folder) if service_keeps_copy => report.saved_in = self.folder(&folder).map(|f| f.name),
            Some(folder) => {
                let (_, mailbox) = Self::split_folder(&folder)?;
                let flags = vec!["\\Seen".to_string()];
                let mut attempts = 0;
                let saved =
                    self.imap_account(account_id)?.run(Lane::User, |s| append_once(s, &mut attempts, &mailbox, &flags, Some(date), &kept));
                match saved {
                    Ok(_) => report.saved_in = self.folder(&folder).map(|f| f.name),
                    // Serveur IMAP injoignable : la copie attend le retour du réseau
                    // au lieu d'être perdue (relecture complète n° 11).
                    Err(e) if is_network_error(&e) => {
                        let copy = PendingSent { id: 0, account: account_id.to_string(), mailbox, date, raw: kept };
                        report.warning = Some(match self.cache().sent_push(&copy) {
                            Ok(()) => tr!("Envoyé ; la copie dans « Éléments envoyés » sera déposée au retour du réseau.").into(),
                            Err(e) => tr!("Envoyé, mais copie dans « Éléments envoyés » impossible : {e}", e = e),
                        });
                    }
                    Err(e) => report.warning = Some(tr!("Envoyé, mais copie dans « Éléments envoyés » impossible : {e}", e = e)),
                }
            }
            None => {
                report.warning = Some(tr!("Envoyé ; aucun dossier « Éléments envoyés » trouvé pour la copie.").into());
            }
        }
        if let Some(id) = answered {
            // Marqueur « répondu », ou « transféré » (`$Forwarded`, RFC 5788)
            // pour un message sans `In-Reply-To` : utile mais pas essentiel.
            let flag = if draft.in_reply_to.as_deref().is_some_and(|p| !p.trim().is_empty()) { "\\Answered" } else { "$Forwarded" };
            let _ = self.set_flag(&[id], flag, true);
        }
        if let Some(id) = draft_id {
            // Le brouillon a été envoyé : il quitte « Brouillons ».
            if let Err(e) = self.remove(id) {
                crate::diag::report(&format_args!("brouillon envoyé mais non supprimé : {e}"));
            }
        }
        Ok(report)
    }

    // ------------------------------------------------------------ brouillons

    /// Enregistre un brouillon dans « Brouillons » (drapeaux `\Draft \Seen`) et
    /// supprime la version précédente (`replace`). Renvoie le nouvel identifiant.
    pub fn save_draft(
        &self,
        account_id: &str,
        mut draft: Draft,
        attachments: Vec<AttachmentInput>,
        replace: Option<MessageId>,
        plain: PlainReplacement,
    ) -> Result<SavedDraft, MailError> {
        if let Some(old) = replace {
            self.ensure_draft(old)?;
            let encrypted = self.cache().ciphertext(old).map_err(MailError::Backend)?.is_some_and(|bytes| !bytes.is_empty());
            if encrypted && !draft.encrypt && plain == PlainReplacement::Refused {
                return Err(MailError::InvalidTarget(
                    tr!("Ce brouillon est chiffré : il n'est pas remplacé par une version en clair. Désactivez « Chiffrer » pour l'enregistrer sans chiffrement.").into(),
                ));
            }
        }
        let config = self.config(account_id).ok_or_else(|| MailError::NotFound(tr!("compte {account}", account = account_id)))?;
        let folder = self
            .folder_by_role(account_id, FolderRole::Drafts)
            .ok_or_else(|| MailError::NotFound(tr!("Aucun dossier « Brouillons » sur ce compte.").into()))?;
        let (_, mailbox) = Self::split_folder(&folder)?;
        draft.from_email.clone_from(&config.email);
        draft.from_name.clone_from(&config.display_name);
        for input in attachments {
            let file = self.resolve_attachment(&input)?;
            draft.attachments.push(file);
        }

        let random = crate::oauth::random_bytes::<16>().map_err(MailError::Backend)?;
        let message_id = compose::new_message_id(&config.email, &random);
        let date = now();
        draft.wire_addresses();
        draft.utc_offset = crate::tz::local_offset(date);
        // Brouillon d'un message à chiffrer (lot D4) : chiffré pour l'expéditeur
        // seul, comme sa copie « Éléments envoyés » ; jamais en clair sur le serveur.
        let raw = if draft.encrypt {
            compose::build_encrypted(&draft, date, &message_id, None, |content| crate::smime::encrypt(&config, content, &[], date))
                .map_err(MailError::InvalidTarget)?
                .1
        } else {
            compose::build_draft(&draft, date, &message_id)
        };
        let flags = vec!["\\Draft".to_string(), "\\Seen".to_string()];
        let target = mailbox.clone();
        let mut attempts = 0;
        let uid = self
            .imap_account(account_id)?
            .run(Lane::User, |s| {
                let uid = append_once(s, &mut attempts, &target, &flags, Some(date), &raw)?;
                // Serveur sans UIDPLUS : on retrouve le brouillon par son Message-ID.
                if let Some(uid) = uid {
                    Ok(Some(uid))
                } else {
                    s.select(&target)?;
                    Ok(s.uid_search("HEADER Message-ID", &message_id)?.last().copied())
                }
            })
            .map_err(backend)?;

        // L'ancienne version n'est supprimée qu'une fois la nouvelle déposée.
        if let Some(old) = replace {
            if let Err(e) = self.remove(old) {
                crate::diag::report(&format_args!("ancienne version du brouillon non supprimée : {e}"));
            }
        }

        let id = match uid {
            Some(uid) => {
                let header = Header {
                    uid,
                    from_name: if config.display_name.is_empty() { config.email.clone() } else { config.display_name.clone() },
                    from_email: config.email.clone(),
                    subject: draft.subject.trim().to_string(),
                    // Pas d'aperçu en clair pour un brouillon chiffré.
                    preview: if draft.encrypt { String::new() } else { mail_core::mime::preview(&draft.html, 140) },
                    date,
                    flags: Flags { seen: true, flagged: false, answered: false },
                    has_attachments: !draft.attachments.is_empty(),
                };
                let mut c = self.cache();
                c.upsert_headers(account_id, &mailbox, &[header]).map_err(backend)?;
                c.id_for(ImapRef { account: account_id.to_string(), mailbox, uid })
            }
            None => self.cache().fresh_id(),
        };
        Ok(SavedDraft { id, folder_id: folder })
    }

    /// Refuse un identifiant qui n'est pas un brouillon : la rédaction ne peut
    /// ni lire, ni remplacer, ni supprimer un autre message, même si sa fenêtre
    /// était compromise (relecture du 10 novembre 2026).
    fn ensure_draft(&self, id: MessageId) -> Result<ImapRef, MailError> {
        let r = self.imap_ref(id)?;
        let folder = format!("{}:{}", r.account, r.mailbox);
        if self.folder_by_role(&r.account, FolderRole::Drafts).as_deref() == Some(folder.as_str()) {
            Ok(r)
        } else {
            Err(MailError::InvalidTarget(tr!("le message n° {id} n'est pas un brouillon", id = id)))
        }
    }

    /// Supprime définitivement un brouillon (bouton « Abandonner »).
    pub fn discard_draft(&self, id: MessageId) -> Result<(), MailError> {
        self.ensure_draft(id)?;
        self.remove(id)
    }

    /// Contenu d'un brouillon, pour reprendre sa rédaction.
    pub fn draft_content(&self, id: MessageId) -> Result<DraftContent, MailError> {
        let r = self.ensure_draft(id)?;
        let item = self.fetch_full(&r)?;
        let fetched = item.body("BODY[]").unwrap_or_default();
        // Brouillon chiffré (lot D4) : déchiffré en mémoire pour reprendre la rédaction.
        let encrypt = smime::is_encrypted(fetched);
        let decrypted =
            if encrypt { Some(smime::decrypt(&self.configs(), &r.account, fetched).map_err(MailError::InvalidTarget)?) } else { None };
        let raw = decrypted.as_deref().unwrap_or(fetched);
        let parsed = mimeparse::parse(raw);
        let plain_text = !String::from_utf8_lossy(raw).to_ascii_lowercase().contains("text/html");
        let html = if plain_text {
            let text = parsed.text.clone().unwrap_or_default();
            text.lines().fold(String::new(), |mut html, l| {
                let escaped = l.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
                let _ = write!(html, "<div>{}</div>", if escaped.is_empty() { "<br>" } else { &escaped });
                html
            })
        } else {
            body_fragment(parsed.html.as_deref().unwrap_or_default())
        };
        let attachments = parsed
            .attachments
            .iter()
            .filter_map(|a| mimeparse::attachment(raw, a.index))
            .map(|a| DraftAttachment {
                size: u64::try_from(a.data.len()).unwrap_or(u64::MAX),
                data: compose::base64(&a.data),
                name: a.name,
                content_type: a.content_type,
            })
            .collect();
        let subject = parsed.subject;
        Ok(DraftContent {
            id,
            account_id: r.account,
            to: address_field(raw, "To"),
            cc: address_field(raw, "Cc"),
            bcc: address_field(raw, "Bcc"),
            subject,
            html,
            plain_text,
            in_reply_to: compose::header_value(raw, "In-Reply-To"),
            references: compose::header_value(raw, "References"),
            attachments,
            encrypt,
        })
    }

    // ------------------------------------------------------------ recherche

    /// Recherche dans tous les dossiers de tous les comptes (sauf corbeille et
    /// indésirables), sur les serveurs (`UID SEARCH`, critères de
    /// [`SearchQuery`], lot E7), en [`SEARCH_BUDGET`] au plus. Les comptes
    /// injoignables sont signalés sans bloquer les autres.
    pub fn search(&self, query: &SearchQuery) -> SearchReport {
        self.search_within(query, SEARCH_BUDGET)
    }

    /// Recherche sur les serveurs en `budget` au plus (lot J14). Chaque compte
    /// cherche dans son propre fil, dossier par dossier, les plus utiles
    /// d'abord ; la connexion de synchronisation est rendue entre deux
    /// dossiers (la synchronisation n'attend plus la fin de la recherche), et
    /// un dossier trop lent n'est pas refait. Passé `budget`, les résultats
    /// reçus sont rendus ; les fils finissent leur dossier en cours, puis
    /// s'arrêtent.
    fn search_within(&self, query: &SearchQuery, budget: std::time::Duration) -> SearchReport {
        use std::sync::mpsc;
        use std::time::Instant;
        let mut report = SearchReport { results: Vec::new(), errors: Vec::new(), unfinished: 0 };
        if query.is_empty() {
            return report;
        }
        let keys = Arc::new(imap_search_keys(query));
        let deadline = Instant::now() + budget;
        let accounts = self.all_accounts();
        let (tx, rx) = mpsc::channel::<SearchStep>();
        for account in &accounts {
            let (account, keys, tx) = (Arc::clone(account), Arc::clone(&keys), tx.clone());
            std::thread::spawn(move || search_account(&account, &keys, deadline, &tx));
        }
        drop(tx);
        // (compte, dossier, rôle, en-tête, Message-ID)
        let mut found: Vec<(String, String, FolderRole, Header, String)> = Vec::new();
        let (mut planned, mut searched, mut abandoned, mut running) = (0, 0, 0, accounts.len());
        while running > 0 {
            // Temps écoulé : les dossiers en cours et suivants restent non parcourus.
            let Ok(step) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) else { break };
            match step {
                SearchStep::Planned(n) => planned += n,
                SearchStep::Searched(hit) => {
                    searched += 1;
                    let Some((account_id, path, role, items)) = hit else { continue };
                    for item in &items {
                        let Some(header) = header_from(item) else { continue };
                        // Pas de critère IMAP pour les pièces jointes : tri sur l'en-tête lu.
                        if query.attachment && !header.has_attachments {
                            continue;
                        }
                        let mid = item.body("BODY[HEADER").and_then(|h| compose::header_value(h, "Message-ID")).unwrap_or_default();
                        found.push((account_id.clone(), path.clone(), role, header, mid));
                    }
                }
                SearchStep::Failed { account_id, message, remaining } => {
                    abandoned += remaining;
                    report.errors.push(AccountError { account_id, message });
                }
                SearchStep::Done => running -= 1,
            }
        }
        report.unfinished = planned.saturating_sub(searched + abandoned);
        // Un même message dans plusieurs dossiers (« Tous les messages » de Gmail) :
        // on garde son dossier le plus parlant.
        found.sort_by_key(|(_, _, role, _, _)| matches!(role, FolderRole::Archive));
        let mut seen = std::collections::HashSet::new();
        found.retain(|(account, _, _, _, mid)| mid.is_empty() || seen.insert((account.clone(), mid.clone())));
        found.sort_by_key(|a| std::cmp::Reverse(a.3.date));
        found.truncate(SEARCH_MAX);
        let mut c = self.cache();
        report.results = found.into_iter().map(|(account, path, _, header, _)| c.summary(&account, &path, header)).collect();
        report
    }

    // ------------------------------------------------------------ hors connexion : actions

    /// Déplace des messages (ou les envoie au dossier d'un rôle). Hors connexion,
    /// les déplacements impossibles sont mis en attente : ils disparaissent tout
    /// de suite de leur dossier et seront faits au retour du réseau.
    pub fn move_or_queue(
        &self,
        journal: &Mutex<Journal>,
        ids: &[MessageId],
        target: Option<&str>,
        role: Option<FolderRole>,
    ) -> Result<MoveReport, MailError> {
        // Journal partagé, verrouillé un instant à chaque étape : un long
        // déplacement entre comptes ne bloque plus les autres (audit P4).
        let mut shared = SharedJournal(journal);
        let mut report = match (target, role) {
            (Some(target), _) => mail_core::move_messages(&mut self.backend(), &mut shared, ids, target)?,
            (None, Some(role)) => mail_core::move_to_role(&mut self.backend(), &mut shared, ids, role)?,
            (None, None) => return Err(MailError::InvalidTarget(tr!("destination absente").into())),
        };
        let failed = std::mem::take(&mut report.failed);
        for f in failed {
            if !is_network_error(&f.reason) {
                report.failed.push(f);
                continue;
            }
            let Ok(r) = self.imap_ref(f.id) else {
                report.failed.push(f);
                continue;
            };
            // Copie déjà faite, suppression de la source en échec : le journal
            // s'en charge (relecture Y2) ; surtout pas d'action « déplacer » en plus.
            let journaled = shared.is_moving(f.id);
            if !journaled {
                let destination = match (target, role) {
                    (Some(t), _) => Some(t.to_string()),
                    (None, Some(role)) => self.folder_by_role(&r.account, role),
                    _ => None,
                };
                let Some(destination) = destination else {
                    report.failed.push(f);
                    continue;
                };
                let action = PendingAction {
                    id: 0,
                    kind: "move".into(),
                    account: r.account.clone(),
                    mailbox: r.mailbox.clone(),
                    uids: vec![r.uid],
                    arg: destination,
                    value: true,
                };
                self.cache().pending_push(&action).map_err(backend)?;
            }
            if !journaled {
                // Retiré de son dossier dès maintenant (le serveur suivra).
                let _ = self.cache().delete_message(&r.account, &r.mailbox, r.uid);
            }
            report.queued += 1;
        }
        Ok(report)
    }

    /// Relit l'en-tête de messages retirés d'avance du cache (déplacement hors
    /// ligne finalement refusé par le serveur) : ils réapparaissent dans leur
    /// dossier. Sans effet si le serveur ne répond pas.
    fn restore_headers(&self, account_id: &str, mailbox: &str, uids: &[u32]) {
        let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let Ok(account) = self.imap_account(account_id) else { return };
        let known = self.known_validity(account_id, mailbox);
        let fetched = account.run(Lane::User, |s| {
            ensure_selected(s, mailbox, known)?;
            s.uid_fetch_set(&set, LIST_QUERY)
        });
        let Ok(items) = fetched else { return };
        let headers: Vec<Header> = items.iter().filter_map(header_from).collect();
        if let Err(e) = self.cache().upsert_headers(account_id, mailbox, &headers) {
            crate::diag::report(&e);
        }
    }

    /// Messages concernés par des actions en attente du réseau.
    pub fn pending_count(&self) -> u32 {
        self.cache().pending_count().unwrap_or(0)
    }

    /// Au retour du réseau : reprend les déplacements interrompus, puis rejoue
    /// les actions faites hors connexion, dans l'ordre. S'arrête à la première
    /// erreur réseau (tout le reste attend la prochaine fois).
    pub fn flush_pending(&self, journal: &Mutex<Journal>) -> FlushReport {
        let mut report = FlushReport::default();
        let mut shared = SharedJournal(journal);
        if shared.unfinished_count() > 0 {
            let resumed = mail_core::resume_moves(&mut self.backend(), &mut shared);
            report.done = report.done.saturating_add(u32::try_from(resumed.moved.len()).unwrap_or(u32::MAX));
        }
        // Copies de messages envoyés : le premier essai cherche déjà la copie par
        // son `Message-ID` (le dépôt interrompu a pu aboutir), pour ne pas la doubler.
        for copy in self.cache().sent_pending().unwrap_or_default() {
            let flags = vec!["\\Seen".to_string()];
            let mut attempts = 1;
            let outcome = match self.imap_account(&copy.account) {
                Ok(account) => {
                    account.run(Lane::User, |s| append_once(s, &mut attempts, &copy.mailbox, &flags, Some(copy.date), &copy.raw))
                }
                Err(e) => Err(e.to_string()),
            };
            match outcome {
                Err(e) if is_network_error(&e) => {
                    report.remaining = self.pending_count().saturating_add(u32::try_from(shared.unfinished_count()).unwrap_or(u32::MAX));
                    return report;
                }
                Err(e) => crate::diag::report(&format_args!("copie d'un message envoyé abandonnée : {e}")),
                Ok(_) => report.done += 1,
            }
            let _ = self.cache().sent_remove(copy.id);
        }
        let actions = self.cache().pending_actions().unwrap_or_default();
        for action in actions {
            let outcome: Result<(), String> = match action.kind.as_str() {
                "flag" => {
                    let op = format!("{}FLAGS.SILENT ({})", if action.value { "+" } else { "-" }, action.arg);
                    let (mailbox, uids) = (action.mailbox.clone(), action.uids.clone());
                    let known = self.known_validity(&action.account, &action.mailbox);
                    match self.imap_account(&action.account) {
                        Ok(account) => account.run(Lane::User, |s| {
                            ensure_selected(s, &mailbox, known)?;
                            s.uid_store(&uids, &op)
                        }),
                        Err(e) => Err(e.to_string()),
                    }
                }
                "move" => {
                    let ids: Vec<MessageId> = {
                        let mut c = self.cache();
                        action
                            .uids
                            .iter()
                            .map(|&uid| c.id_for(ImapRef { account: action.account.clone(), mailbox: action.mailbox.clone(), uid }))
                            .collect()
                    };
                    // Déjà copiés, source en cours de suppression : le journal termine.
                    // Verrou du cache rendu avant celui du journal (qui prend ensuite
                    // celui du cache pour s'enregistrer : jamais dans l'autre ordre).
                    let ids: Vec<MessageId> = ids.into_iter().filter(|id| !shared.is_moving(*id)).collect();
                    if ids.is_empty() {
                        Ok(())
                    } else {
                        match mail_core::move_messages(&mut self.backend(), &mut shared, &ids, &action.arg) {
                            // Panne réseau : l'action attend le retour du réseau. Refus du
                            // serveur (dossier supprimé ailleurs…) : le message réapparaît
                            // dans son dossier (Y4, même quand le dossier cible est encore
                            // dans l'arborescence en cache).
                            Ok(r) => match r.failed.iter().find(|f| is_network_error(&f.reason)).or(r.failed.first()) {
                                Some(f) => Err(f.reason.clone()),
                                None => Ok(()),
                            },
                            Err(e) => Err(e.to_string()),
                        }
                    }
                }
                _ => Ok(()),
            };
            match outcome {
                Err(e) if is_network_error(&e) => break,
                // Réussi, ou refusé pour de bon (message disparu…) : l'action est retirée.
                other => {
                    // Déplacement refusé : le message, retiré d'avance de son
                    // dossier, y est toujours sur le serveur ; il réapparaît (Y4).
                    if other.is_err() && action.kind == "move" {
                        self.restore_headers(&action.account, &action.mailbox, &action.uids);
                    }
                    let _ = self.cache().pending_remove(action.id);
                    report.done += 1;
                }
            }
        }
        report.remaining = self.pending_count().saturating_add(u32::try_from(shared.unfinished_count()).unwrap_or(u32::MAX));
        report
    }

    // ------------------------------------------------------------ flags

    /// Pose ou retire un flag (`\Seen`, `\Flagged`) sur le serveur, puis dans le cache.
    pub fn set_flag(&self, ids: &[MessageId], flag: &str, on: bool) -> Result<(), MailError> {
        let mut groups: Vec<(String, String, Vec<u32>)> = Vec::new();
        for &id in ids {
            let r = self.imap_ref(id)?;
            match groups.iter_mut().find(|g| g.0 == r.account && g.1 == r.mailbox) {
                Some(g) => g.2.push(r.uid),
                None => groups.push((r.account, r.mailbox, vec![r.uid])),
            }
        }
        let op = format!("{}FLAGS.SILENT ({flag})", if on { "+" } else { "-" });
        let is_seen = flag.eq_ignore_ascii_case("\\Seen");
        for (account, mailbox, uids) in groups {
            let known = self.known_validity(&account, &mailbox);
            let result = self.imap_account(&account)?.run(Lane::User, |s| {
                ensure_selected(s, &mailbox, known)?;
                s.uid_store(&uids, &op)
            });
            match result {
                Ok(()) => {}
                // Hors connexion : appliqué ici tout de suite, sur le serveur au retour du réseau.
                Err(e) if is_network_error(&e) => {
                    let action = PendingAction {
                        id: 0,
                        kind: "flag".into(),
                        account: account.clone(),
                        mailbox: mailbox.clone(),
                        uids: uids.clone(),
                        arg: flag.to_string(),
                        value: on,
                    };
                    self.cache().pending_push(&action).map_err(backend)?;
                }
                Err(e) => return Err(backend(e)),
            }

            let mut c = self.cache();
            let mut delta = 0i64;
            for uid in &uids {
                if is_seen {
                    if let Ok(Some(previous)) = c.is_seen(&account, &mailbox, *uid) {
                        if previous != on {
                            delta += if on { -1 } else { 1 };
                        }
                    }
                }
                c.set_flag(&account, &mailbox, *uid, flag, on).map_err(backend)?;
            }
            if delta != 0 {
                c.adjust_unread(&account, &mailbox, delta).map_err(backend)?;
            }
        }
        Ok(())
    }
}

/// Opérations de `mail-core` (déplacements, y compris entre comptes) :
/// chacune ne verrouille que la connexion du compte concerné.
impl Router {
    pub fn folder(&self, id: &str) -> Option<Folder> {
        let (account_id, _) = id.split_once(':')?;
        self.imap_account(account_id).ok()?.find_folder(|f| f.id == id)
    }

    pub fn folder_by_role(&self, account_id: &str, role: FolderRole) -> Option<FolderId> {
        self.imap_account(account_id).ok()?.find_folder(|f| f.role == role).map(|f| f.id)
    }

    pub fn message_location(&self, id: MessageId) -> mail_core::Result<FolderId> {
        self.imap_ref(id).map(|r| format!("{}:{}", r.account, r.mailbox))
    }

    pub fn native_move(&self, ids: &[MessageId], target: &str) -> mail_core::Result<Vec<MessageId>> {
        let (_, target_mailbox) = Self::split_folder(target)?;
        for (account, mailbox, uids) in self.by_folder(ids)? {
            let known = self.known_validity(&account, &mailbox);
            // Vers « Éléments supprimés » : dossier d'origine noté pour « Restaurer ».
            let to_trash = mailbox != target_mailbox && self.folder_by_role(&account, FolderRole::Trash).as_deref() == Some(target);
            let mut origins: Vec<(u32, String)> = Vec::new();
            let mut attempts = 0;
            self.imap_account(&account)?
                .run(Lane::User, |s| {
                    ensure_selected(s, &mailbox, known)?;
                    // Lu au premier essai seulement : au suivant, les messages
                    // ont peut-être déjà quitté le dossier.
                    if to_trash && origins.is_empty() {
                        // Sans effet sur la suppression si la lecture échoue : seul
                        // « Restaurer » renverra alors dans la boîte de réception.
                        origins = fetch_message_ids(s, &uids).unwrap_or_default();
                    }
                    move_once(s, &mut attempts, &mailbox, known, &uids, &target_mailbox)
                })
                .map_err(backend)?;
            // Les messages ont quitté ce dossier ; la cible les recevra à sa synchronisation.
            // Une seule transaction pour tout le dossier (audit P4).
            let mut c = self.cache();
            c.delete_messages(&account, &mailbox, &uids).map_err(backend)?;
            if !origins.is_empty() {
                let message_ids: Vec<String> = origins.into_iter().map(|(_, m)| m).collect();
                if let Err(e) = c.trash_origins_put(&account, &mailbox, &message_ids, now()) {
                    crate::diag::report(&e);
                }
            }
        }
        Ok(ids.to_vec())
    }

    pub fn fetch_raw(&self, id: MessageId) -> mail_core::Result<RawMessage> {
        let r = self.imap_ref(id)?;
        let item = self.fetch_full(&r)?;
        Ok(RawMessage {
            bytes: item.body("BODY[]").unwrap_or_default().to_vec(),
            flags: flags_from(&item),
            internal_date: item.internal_date().unwrap_or_else(now),
        })
    }

    pub fn append(&self, target: &str, raw: &RawMessage) -> mail_core::Result<MessageId> {
        let (account_id, mailbox) = Self::split_folder(target)?;
        let mut flags = Vec::new();
        if raw.flags.seen {
            flags.push("\\Seen".to_string());
        }
        if raw.flags.flagged {
            flags.push("\\Flagged".to_string());
        }
        if raw.flags.answered {
            flags.push("\\Answered".to_string());
        }
        let date = Some(raw.internal_date).filter(|d| *d > 0).or(Some(now()));
        let mut attempts = 0;
        let uid = self
            .imap_account(&account_id)?
            .run(Lane::User, |s| append_once(s, &mut attempts, &mailbox, &flags, date, &raw.bytes))
            .map_err(backend)?;
        let mut c = self.cache();
        Ok(match uid {
            Some(uid) => c.id_for(ImapRef { account: account_id, mailbox, uid }),
            None => c.fresh_id(), // serveur sans UIDPLUS : la liste sera relue
        })
    }

    pub fn find_message(&self, folder: &str, message_id: &str) -> mail_core::Result<Option<MessageId>> {
        let (account_id, mailbox) = Self::split_folder(folder)?;
        let target = mailbox.clone();
        let uids = self
            .imap_account(&account_id)?
            .run(Lane::User, |s| {
                ensure_selected(s, &target, None)?;
                s.uid_search("HEADER Message-ID", message_id)
            })
            .map_err(backend)?;
        Ok(uids.last().map(|&uid| self.cache().id_for(ImapRef { account: account_id, mailbox, uid })))
    }

    pub fn remove(&self, id: MessageId) -> mail_core::Result<()> {
        let r = self.imap_ref(id)?;
        let (mailbox, uid) = (r.mailbox.clone(), r.uid);
        let known = self.known_validity(&r.account, &r.mailbox);
        self.imap_account(&r.account)?
            .run(Lane::User, |s| {
                ensure_selected(s, &mailbox, known)?;
                s.uid_store(&[uid], "+FLAGS.SILENT (\\Deleted)")?;
                s.uid_expunge(&[uid])
            })
            .map_err(backend)?;
        self.cache().delete_message(&r.account, &r.mailbox, uid).map_err(backend)
    }
}

/// Vue du routeur pour `mail-core`, avec l'arborescence figée au début de
/// l'opération (le trait rend des références vers les dossiers).
struct Backend<'a> {
    router: &'a Router,
    folders: Vec<Folder>,
}

impl Router {
    fn backend(&self) -> Backend<'_> {
        Backend { router: self, folders: self.all_accounts().iter().flat_map(|a| a.folders()).collect() }
    }
}

impl MailBackend for Backend<'_> {
    fn folder(&self, id: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    fn folder_by_role(&self, account_id: &str, role: FolderRole) -> Option<FolderId> {
        self.folders.iter().find(|f| f.account_id == account_id && f.role == role).map(|f| f.id.clone())
    }

    fn message_location(&self, id: MessageId) -> mail_core::Result<FolderId> {
        self.router.message_location(id)
    }

    fn native_move(&mut self, ids: &[MessageId], target: &str) -> mail_core::Result<Vec<MessageId>> {
        self.router.native_move(ids, target)
    }

    fn fetch_raw(&mut self, id: MessageId) -> mail_core::Result<RawMessage> {
        self.router.fetch_raw(id)
    }

    fn append(&mut self, target: &str, raw: &RawMessage) -> mail_core::Result<MessageId> {
        self.router.append(target, raw)
    }

    fn find_message(&mut self, folder: &str, message_id: &str) -> mail_core::Result<Option<MessageId>> {
        self.router.find_message(folder, message_id)
    }

    fn remove(&mut self, id: MessageId) -> mail_core::Result<()> {
        self.router.remove(id)
    }
}

/// Taille d'une tranche d'export mbox, au plus (sauf un message seul plus gros).
const EXPORT_CHUNK_BYTES: u64 = 16 << 20;
/// Messages par tranche d'export, au plus.
const EXPORT_CHUNK_COUNT: usize = 50;

/// Tranches d'UID pour l'export, du plus ancien au plus récent : `max_bytes`
/// (d'après `RFC822.SIZE`) ou `max_count` messages au plus ; un message plus
/// gros que `max_bytes` forme une tranche à lui seul (audit P7).
fn export_chunks(mut sizes: Vec<(u32, u32)>, max_bytes: u64, max_count: usize) -> Vec<Vec<u32>> {
    sizes.sort_unstable_by_key(|&(uid, _)| uid);
    sizes.dedup_by_key(|&mut (uid, _)| uid);
    let mut chunks: Vec<Vec<u32>> = Vec::new();
    let (mut current, mut bytes) = (Vec::new(), 0u64);
    for (uid, size) in sizes {
        let size = u64::from(size);
        if !current.is_empty() && (bytes + size > max_bytes || current.len() >= max_count) {
            chunks.push(std::mem::take(&mut current));
            bytes = 0;
        }
        current.push(uid);
        bytes += size;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Adresse de l'expéditeur pour la ligne `From ` d'une boîte mbox : celle entre
/// `<` et `>` de l'en-tête `From`, sinon l'en-tête entier ; vide s'il manque.
fn envelope_sender(raw: &[u8]) -> String {
    let from = compose::header_value(raw, "From").unwrap_or_default();
    match (from.rfind('<'), from.rfind('>')) {
        (Some(start), Some(end)) if start < end => from[start + 1..end].trim().to_string(),
        _ => from.trim().to_string(),
    }
}

/// Critères `UID SEARCH` d'une recherche (lot E7). Les pièces jointes n'ont pas
/// de critère IMAP : elles sont triées après lecture des en-têtes.
/// Recherche d'un compte, dans son propre fil (lot J14) : un dossier à la
/// fois sur la connexion de synchronisation, rendue entre deux dossiers ;
/// arrêt à `deadline`, et dès qu'une erreur de connexion survient.
fn search_account(account: &ImapAccount, keys: &[SearchKey], deadline: std::time::Instant, tx: &std::sync::mpsc::Sender<SearchStep>) {
    let mut folders: Vec<(String, FolderRole)> = account
        .folders()
        .iter()
        .filter(|f| !matches!(f.role, FolderRole::Trash | FolderRole::Junk) && f.total > 0)
        .map(|f| (f.path.clone(), f.role))
        .collect();
    folders.sort_by(|a, b| search_rank(a.1).cmp(&search_rank(b.1)).then_with(|| a.0.cmp(&b.0)));
    // Appelant parti (temps écoulé) : les envois échouent, sans importance.
    let _ = tx.send(SearchStep::Planned(folders.len()));
    for (index, (path, role)) in folders.iter().enumerate() {
        if std::time::Instant::now() >= deadline {
            break;
        }
        // Un seul essai : un dossier qui dépasse le délai de lecture n'est pas
        // refait. Échéance posée sur la session (lot N2) : un dossier lent est
        // arrêté au plus un délai de lecture court après elle, la connexion
        // abandonnée (réponse à moitié lue) et la synchronisation libérée.
        let result = account.run_tries(Lane::Sync, 1, |s| {
            s.set_deadline(Some(deadline));
            let found = search_folder(s, path, keys);
            s.set_deadline(None);
            found
        });
        match result {
            Ok(items) => {
                let hit = items.map(|items| (account.config.id.clone(), path.clone(), *role, items));
                let _ = tx.send(SearchStep::Searched(hit));
            }
            // Arrêté par l'échéance : dossiers non parcourus, pas une erreur du compte.
            Err(_) if std::time::Instant::now() >= deadline => break,
            Err(message) => {
                let remaining = folders.len() - index;
                let _ = tx.send(SearchStep::Failed { account_id: account.config.id.clone(), message, remaining });
                break;
            }
        }
    }
    let _ = tx.send(SearchStep::Done);
}

/// Recherche dans un dossier : les plus récents des messages trouvés (`None` :
/// aucun, ou dossier et critère refusés par le serveur).
fn search_folder(s: &mut Session, path: &str, keys: &[SearchKey]) -> imap_lite::Result<Option<Vec<FetchItem>>> {
    match s.select(path) {
        Ok(_) => {}
        Err(e) if e.is_connection_lost() => return Err(e),
        Err(_) => return Ok(None),
    }
    let uids = match s.uid_search_keys(keys) {
        Ok(uids) => uids,
        Err(e) if e.is_connection_lost() => return Err(e),
        Err(_) => return Ok(None), // critère refusé par ce dossier
    };
    let recent: Vec<u32> = uids.iter().rev().take(SEARCH_PER_FOLDER).copied().collect();
    if recent.is_empty() {
        return Ok(None);
    }
    s.uid_fetch(&recent, SEARCH_QUERY).map(Some)
}

fn imap_search_keys(q: &SearchQuery) -> Vec<SearchKey> {
    let text = |name: &'static str, value: &String| SearchKey::Text(name, value.clone());
    let mut keys: Vec<SearchKey> = Vec::new();
    keys.extend(q.words.iter().map(|w| text("TEXT", w)));
    keys.extend(q.excluded.iter().map(|w| SearchKey::Not(Box::new(text("TEXT", w)))));
    keys.extend(q.from.iter().map(|w| text("FROM", w)));
    keys.extend(q.to.iter().map(|w| SearchKey::Or(Box::new(text("TO", w)), Box::new(text("CC", w)))));
    keys.extend(q.subject.iter().map(|w| text("SUBJECT", w)));
    if let Some(day) = q.before {
        keys.push(SearchKey::Date("BEFORE", day.imap()));
    }
    if let Some(day) = q.after {
        keys.push(SearchKey::Date("SINCE", day.imap()));
    }
    match q.unread {
        Some(true) => keys.push(SearchKey::Flag("UNSEEN")),
        Some(false) => keys.push(SearchKey::Flag("SEEN")),
        None => {}
    }
    if q.flagged {
        keys.push(SearchKey::Flag("FLAGGED"));
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e8a_expediteur_de_la_ligne_from() {
        assert_eq!(envelope_sender(b"From: Marie <marie@x.fr>\r\n\r\n"), "marie@x.fr");
        assert_eq!(envelope_sender(b"From: paul@y.fr\r\n\r\n"), "paul@y.fr");
        assert_eq!(envelope_sender(b"Subject: sans expediteur\r\n\r\n"), "");
    }

    #[test]
    fn e7_criteres_imap() {
        let q = SearchQuery::parse(
            "budget -brouillon de:marie à:paul objet:devis après:01/11/2026 avant:16/11/2026 est:nonlu est:suivi avec:pj",
        )
        .unwrap();
        let keys = imap_search_keys(&q);
        assert_eq!(keys.len(), 9, "la pièce jointe n'a pas de critère IMAP : {keys:?}");
        assert_eq!(keys[0], SearchKey::Text("TEXT", "budget".into()));
        assert_eq!(keys[1], SearchKey::Not(Box::new(SearchKey::Text("TEXT", "brouillon".into()))));
        assert_eq!(keys[3], SearchKey::Or(Box::new(SearchKey::Text("TO", "paul".into())), Box::new(SearchKey::Text("CC", "paul".into()))));
        assert_eq!(keys[5], SearchKey::Date("BEFORE", "16-Nov-2026".into()));
        assert_eq!(keys[6], SearchKey::Date("SINCE", "1-Nov-2026".into()));
        assert_eq!(keys[7..], [SearchKey::Flag("UNSEEN"), SearchKey::Flag("FLAGGED")]);
        assert_eq!(imap_search_keys(&SearchQuery::parse("est:lu").unwrap()), [SearchKey::Flag("SEEN")]);
    }
    use std::collections::VecDeque;
    use std::io::{Read, Write};

    /// Serveur IMAP joué en mémoire : chaque commande complète du client reçoit
    /// aussitôt sa réponse ; les commandes reçues sont notées (lot E5).
    struct Scripted {
        to_client: VecDeque<u8>,
        line: Vec<u8>,
        /// Octets d'un littéral encore attendus, et début de la commande qui l'annonce.
        literal: usize,
        pending: String,
        /// UID trouvé par `UID SEARCH`.
        found: Option<u32>,
        /// `UID MOVE` refusé (dossier cible supprimé ailleurs).
        refuse_move: bool,
        /// Réponse à `UID FETCH` (lignes `* n FETCH …`).
        fetch: String,
        /// Débuts de commande (sans l'étiquette) refusés par `NO`.
        refused: Vec<String>,
        /// Dossier dont la sélection fait attendre (serveur lent, lot J14).
        slow_select: Option<(String, std::time::Duration)>,
        /// Délai de lecture joué (lot N2) : le dossier lent ne fait plus
        /// attendre l'écriture, la lecture répond `WouldBlock` à chaque délai
        /// écoulé jusqu'à `stalled_until`, comme un socket à délai de lecture.
        tick: Option<std::time::Duration>,
        stalled_until: Option<std::time::Instant>,
        received: Arc<Mutex<Vec<String>>>,
    }

    impl Scripted {
        fn session(found: Option<u32>) -> (Session, Arc<Mutex<Vec<String>>>) {
            let received = Arc::new(Mutex::new(Vec::new()));
            (Self::open(found, false, String::new(), &received), received)
        }

        fn open(found: Option<u32>, refuse_move: bool, fetch: String, received: &Arc<Mutex<Vec<String>>>) -> Session {
            let server = Scripted {
                to_client: b"* OK [CAPABILITY IMAP4rev1 MOVE UIDPLUS] pret\r\n".iter().copied().collect(),
                line: Vec::new(),
                literal: 0,
                pending: String::new(),
                found,
                refuse_move,
                fetch,
                refused: Vec::new(),
                slow_select: None,
                tick: None,
                stalled_until: None,
                received: Arc::clone(received),
            };
            let stream: Box<dyn crate::transport::Stream> = Box::new(server);
            imap_lite::Client::connect(stream).expect("accueil du serveur joué")
        }

        /// Serveur sans l'extension `MOVE` : déplacement par copie, puis suppression.
        fn without_move(found: Option<u32>, fetch: String) -> (Session, Arc<Mutex<Vec<String>>>) {
            let received = Arc::new(Mutex::new(Vec::new()));
            let server = Scripted {
                to_client: b"* OK [CAPABILITY IMAP4rev1 UIDPLUS] pret\r\n".iter().copied().collect(),
                line: Vec::new(),
                literal: 0,
                pending: String::new(),
                found,
                refuse_move: false,
                fetch,
                refused: Vec::new(),
                slow_select: None,
                tick: None,
                stalled_until: None,
                received: Arc::clone(&received),
            };
            let stream: Box<dyn crate::transport::Stream> = Box::new(server);
            (imap_lite::Client::connect(stream).expect("accueil du serveur joué"), received)
        }

        /// Serveur dont la sélection du dossier `folder` prend `delay` (lot J14).
        fn slow(found: Option<u32>, fetch: String, folder: &str, delay: std::time::Duration) -> (Session, Arc<Mutex<Vec<String>>>) {
            Self::slow_with(found, fetch, folder, delay, None)
        }

        /// Comme [`Scripted::slow`], avec un délai de lecture joué (`tick`, lot N2).
        fn slow_with(
            found: Option<u32>,
            fetch: String,
            folder: &str,
            delay: std::time::Duration,
            tick: Option<std::time::Duration>,
        ) -> (Session, Arc<Mutex<Vec<String>>>) {
            let received = Arc::new(Mutex::new(Vec::new()));
            let server = Scripted {
                to_client: b"* OK [CAPABILITY IMAP4rev1 MOVE UIDPLUS] pret\r\n".iter().copied().collect(),
                line: Vec::new(),
                literal: 0,
                pending: String::new(),
                found,
                refuse_move: false,
                fetch,
                refused: Vec::new(),
                slow_select: Some((folder.to_owned(), delay)),
                tick,
                stalled_until: None,
                received: Arc::clone(&received),
            };
            let stream: Box<dyn crate::transport::Stream> = Box::new(server);
            (imap_lite::Client::connect(stream).expect("accueil du serveur joué"), received)
        }

        /// Serveur qui refuse par `NO` les commandes commençant par `refused`.
        fn refusing(refused: &[&str]) -> (Session, Arc<Mutex<Vec<String>>>) {
            let received = Arc::new(Mutex::new(Vec::new()));
            let server = Scripted {
                to_client: b"* OK [CAPABILITY IMAP4rev1 MOVE UIDPLUS] pret\r\n".iter().copied().collect(),
                line: Vec::new(),
                literal: 0,
                pending: String::new(),
                found: None,
                refuse_move: false,
                fetch: String::new(),
                refused: refused.iter().map(|start| (*start).to_string()).collect(),
                slow_select: None,
                tick: None,
                stalled_until: None,
                received: Arc::clone(&received),
            };
            let stream: Box<dyn crate::transport::Stream> = Box::new(server);
            (imap_lite::Client::connect(stream).expect("accueil du serveur joué"), received)
        }

        fn command(&mut self, text: &str) {
            let mut words = text.split_whitespace();
            let tag = words.next().unwrap_or("*").to_owned();
            let verb = words.next().unwrap_or("").to_ascii_uppercase();
            let next = words.next().unwrap_or("").to_ascii_uppercase();
            {
                let mut received = self.received.lock().expect("journal du serveur joué");
                received.push(format!("{verb} {next}"));
                // Commande entière (sans l'étiquette), pour vérifier ses arguments.
                received.push(format!("> {}", text.split_once(' ').map_or(text, |(_, rest)| rest)));
            }
            let rest = text.split_once(' ').map_or("", |(_, rest)| rest);
            if let Some((folder, delay)) = &self.slow_select {
                if verb == "SELECT" && rest.contains(folder.as_str()) {
                    match self.tick {
                        Some(_) => self.stalled_until = Some(std::time::Instant::now() + *delay),
                        None => std::thread::sleep(*delay),
                    }
                }
            }
            if self.refused.iter().any(|start| rest.starts_with(start.as_str())) {
                self.to_client.extend(format!("{tag} NO refusé\r\n").bytes());
                return;
            }
            let reply = match (verb.as_str(), next.as_str()) {
                ("SELECT" | "EXAMINE", _) => format!("* 1 EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n{tag} OK [READ-WRITE] ok\r\n"),
                ("UID", "SEARCH") => {
                    let hit = self.found.map_or_default(|uid| format!(" {uid}"));
                    format!("* SEARCH{hit}\r\n{tag} OK ok\r\n")
                }
                ("APPEND", _) => format!("{tag} OK [APPENDUID 7 43] ok\r\n"),
                ("UID", "MOVE") if self.refuse_move => format!("{tag} NO [TRYCREATE] dossier inexistant\r\n"),
                ("UID", "FETCH") => format!("{}{tag} OK ok\r\n", self.fetch),
                ("CAPABILITY", _) => format!("* CAPABILITY IMAP4rev1\r\n{tag} OK ok\r\n"),
                _ => format!("{tag} OK ok\r\n"),
            };
            self.to_client.extend(reply.bytes());
        }
    }

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if let (Some(tick), Some(until)) = (self.tick, self.stalled_until) {
                if std::time::Instant::now() < until {
                    std::thread::sleep(tick);
                    return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "délai de lecture joué"));
                }
            }
            let n = buf.len().min(self.to_client.len());
            for (slot, byte) in buf.iter_mut().zip(self.to_client.drain(..n)) {
                *slot = byte;
            }
            Ok(n)
        }
    }

    impl Write for Scripted {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            for &byte in buf {
                if self.literal > 0 {
                    self.literal -= 1;
                    continue;
                }
                self.line.push(byte);
                if !self.line.ends_with(b"\r\n") {
                    continue;
                }
                let text = String::from_utf8_lossy(&self.line).trim_end().to_owned();
                self.line.clear();
                let size = text.strip_suffix('}').and_then(|t| t.rsplit_once('{')).and_then(|(_, n)| n.trim_end_matches('+').parse().ok());
                if let Some(size) = size {
                    // Littéral annoncé : feu vert, puis le reste de la commande après lui.
                    self.literal = size;
                    self.pending.push_str(&text);
                    self.to_client.extend(b"+ ok\r\n");
                } else {
                    let mut whole = std::mem::take(&mut self.pending);
                    whole.push_str(&text);
                    self.command(&whole);
                }
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    const SENT: &[u8] = b"Message-ID: <envoi-1@example.com>\r\nSubject: Essai\r\n\r\nCorps\r\n";

    #[test]
    fn y8_append_rejoue_sans_doublon() {
        // La connexion est tombée après un APPEND accepté : au nouvel essai, le
        // message est retrouvé par son Message-ID au lieu d'être déposé une seconde fois.
        let (mut s, received) = Scripted::session(Some(42));
        let mut attempts = 1;
        assert_eq!(append_once(&mut s, &mut attempts, "Sent", &[], None, SENT).unwrap(), Some(42));
        let received = received.lock().unwrap().clone();
        assert!(received.iter().any(|c| c == "UID SEARCH"), "cherché d'abord : {received:?}");
        assert!(!received.iter().any(|c| c.starts_with("APPEND")), "pas de second dépôt : {received:?}");
    }

    #[test]
    fn y8_append_premier_essai_et_message_absent() {
        // Premier essai : déposé directement.
        let (mut s, received) = Scripted::session(None);
        let mut attempts = 0;
        assert_eq!(append_once(&mut s, &mut attempts, "Sent", &[], None, SENT).unwrap(), Some(43));
        assert_eq!(received.lock().unwrap().iter().filter(|c| c.starts_with("APPEND")).count(), 1);
        // Nouvel essai, message pas arrivé sur le serveur : déposé.
        let (mut s, received) = Scripted::session(None);
        let mut attempts = 1;
        assert_eq!(append_once(&mut s, &mut attempts, "Sent", &[], None, SENT).unwrap(), Some(43));
        assert_eq!(received.lock().unwrap().iter().filter(|c| c.starts_with("APPEND")).count(), 1);
    }

    /// Réponse `UID FETCH` qui donne le `Message-ID` du message 5.
    fn message_id_of_5() -> String {
        let header = "Message-ID: <m5@example.com>\r\n\r\n";
        format!("* 1 FETCH (UID 5 BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{header})\r\n", header.len())
    }

    fn count(received: &Arc<Mutex<Vec<String>>>, command: &str) -> usize {
        received.lock().expect("journal du serveur joué").iter().filter(|c| *c == command).count()
    }

    #[test]
    fn b1_deplacement_rejoue_sans_recopie() {
        // Serveur sans MOVE, connexion tombée après le COPY : au nouvel essai,
        // le message déjà dans la cible n'est que retiré de la source.
        let (mut s, received) = Scripted::without_move(Some(9), message_id_of_5());
        let mut attempts = 1;
        move_once(&mut s, &mut attempts, "INBOX", None, &[5], "Archive").expect("déplacement repris");
        assert_eq!(count(&received, "UID SEARCH"), 1, "cherché dans la cible : {:?}", received.lock().unwrap());
        assert_eq!(count(&received, "UID COPY"), 0, "pas de seconde copie : {:?}", received.lock().unwrap());
        assert_eq!(count(&received, "UID STORE"), 1, "source marquée supprimée");
        assert_eq!(count(&received, "UID EXPUNGE"), 1, "source retirée");
    }

    #[test]
    fn b1_deplacement_rejoue_message_absent_de_la_cible() {
        // Premier essai : copie directe, sans recherche.
        let (mut s, received) = Scripted::without_move(None, message_id_of_5());
        let mut attempts = 0;
        move_once(&mut s, &mut attempts, "INBOX", None, &[5], "Archive").expect("déplacement");
        assert_eq!(count(&received, "UID SEARCH"), 0);
        assert_eq!(count(&received, "UID COPY"), 1);
        // Nouvel essai, copie pas arrivée dans la cible : recopiée une fois.
        let (mut s, received) = Scripted::without_move(None, message_id_of_5());
        let mut attempts = 1;
        move_once(&mut s, &mut attempts, "INBOX", None, &[5], "Archive").expect("déplacement repris");
        assert_eq!(count(&received, "UID SEARCH"), 1);
        assert_eq!(count(&received, "UID COPY"), 1);
    }

    /// Lit le message 5 de « mailo:INBOX » sur un serveur qui répond `fetch`.
    fn fetch_raw_with(fetch: &str) -> mail_core::Result<RawMessage> {
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let tree = build_folders("mailo", vec![entry("INBOX", &[])]);
        lock_cache(&cache).save_folders("mailo", &tree).unwrap();
        let router = Router::new(vec![config("mailo")], Arc::clone(&cache));
        let id = lock_cache(&cache).id_for(ImapRef { account: "mailo".into(), mailbox: "INBOX".into(), uid: 5 });
        let fetch = fetch.to_owned();
        let received = Arc::new(Mutex::new(Vec::new()));
        TEST_SERVER.with(|s| *s.borrow_mut() = Some(Box::new(move || Scripted::open(None, false, fetch.clone(), &received))));
        let raw = router.fetch_raw(id);
        TEST_SERVER.with(|s| *s.borrow_mut() = None);
        raw
    }

    /// Message à deux pièces jointes, lu par `attachment` sur un serveur joué ;
    /// rend le nombre de `UID FETCH` reçus.
    fn attachments_fetches(validity: Option<u32>) -> usize {
        let body = "Subject: Devis\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n\
                    --b\r\nContent-Type: text/plain\r\n\r\nCorps\r\n\
                    --b\r\nContent-Type: text/plain; name=a.txt\r\nContent-Disposition: attachment; filename=a.txt\r\n\r\nA\r\n\
                    --b\r\nContent-Type: text/plain; name=b.txt\r\nContent-Disposition: attachment; filename=b.txt\r\n\r\nB\r\n\
                    --b--\r\n";
        let fetch = format!("* 1 FETCH (UID 5 FLAGS (\\Seen) BODY[] {{{}}}\r\n{body})\r\n", body.len());
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let tree = build_folders("mailo", vec![entry("INBOX", &[])]);
        lock_cache(&cache).save_folders("mailo", &tree).unwrap();
        if let Some(validity) = validity {
            lock_cache(&cache).set_uid_validity("mailo", "INBOX", validity).unwrap();
        }
        let router = Router::new(vec![config("mailo")], Arc::clone(&cache));
        let id = lock_cache(&cache).id_for(ImapRef { account: "mailo".into(), mailbox: "INBOX".into(), uid: 5 });
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        TEST_SERVER.with(|s| *s.borrow_mut() = Some(Box::new(move || Scripted::open(None, false, fetch.clone(), &log))));
        let names: Vec<String> = (0..4).filter_map(|index| router.attachment(id, index).ok()).map(|a| a.name).collect();
        TEST_SERVER.with(|s| *s.borrow_mut() = None);
        assert!(names.contains(&"a.txt".to_owned()) && names.contains(&"b.txt".to_owned()), "pièces lues : {names:?}");
        count(&received, "UID FETCH")
    }

    #[test]
    fn p5_tout_enregistrer_telecharge_une_fois() {
        assert_eq!(attachments_fetches(Some(7)), 1, "un seul téléchargement pour toutes les pièces");
    }

    #[test]
    fn p5_sans_uidvalidity_connue_rien_n_est_garde() {
        assert_eq!(attachments_fetches(None), 4, "UID sans UIDVALIDITY : relu à chaque fois");
    }

    #[test]
    fn b5_drapeaux_seuls_avant_le_corps() {
        // Un autre appareil marque le message lu pendant la lecture : la réponse
        // de drapeaux seuls arrive avant celle qui porte le corps.
        let body = "Subject: Devis\r\n\r\nCorps\r\n";
        let fetch = format!("* 1 FETCH (UID 5 FLAGS (\\Seen))\r\n* 1 FETCH (UID 5 FLAGS (\\Seen) BODY[] {{{}}}\r\n{body})\r\n", body.len());
        let raw = fetch_raw_with(&fetch).expect("corps reçu");
        assert_eq!(raw.bytes, body.as_bytes());
    }

    #[test]
    fn b5_corps_absent_ou_vide_refuse() {
        for fetch in ["* 1 FETCH (UID 5 FLAGS (\\Seen))\r\n", "* 1 FETCH (UID 5 BODY[] NIL)\r\n", "* 1 FETCH (UID 5 BODY[] {0}\r\n)\r\n"] {
            let result = fetch_raw_with(fetch);
            assert!(matches!(result, Err(MailError::Backend(_))), "réponse {fetch:?} : {result:?}");
        }
        let result = fetch_raw_with("");
        assert!(matches!(result, Err(MailError::NotFound(_))), "message disparu : {result:?}");
    }

    #[test]
    fn s4_brouillon_chiffre_jamais_remplace_en_clair() {
        // Brouillon chiffré rouvert alors que le certificat n'est plus lisible :
        // la version en clair n'est déposée que si l'utilisateur a désactivé « Chiffrer ».
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let tree = build_folders("mailo", vec![entry("INBOX", &[]), entry("Drafts", &["\\Drafts"])]);
        lock_cache(&cache).save_folders("mailo", &tree).unwrap();
        let router = Router::new(vec![config("mailo")], Arc::clone(&cache));
        let id = lock_cache(&cache).id_for(ImapRef { account: "mailo".into(), mailbox: "Drafts".into(), uid: 5 });
        let extra = BodyExtra { signature: cache::ENCRYPTED.into(), ciphertext: b"chiffre".to_vec(), ..BodyExtra::default() };
        lock_cache(&cache).save_body("mailo", "Drafts", 5, "", "", &extra).unwrap();
        let draft = Draft { to: vec!["paul@example.com".into()], subject: "Devis".into(), ..Draft::default() };

        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        TEST_SERVER.with(|s| *s.borrow_mut() = Some(Box::new(move || Scripted::open(None, false, String::new(), &log))));
        let refused = router.save_draft("mailo", draft.clone(), Vec::new(), Some(id), PlainReplacement::Refused);
        let sent_before_confirmation = received.lock().unwrap().iter().filter(|c| c.starts_with("APPEND")).count();
        let confirmed = router.save_draft("mailo", draft, Vec::new(), Some(id), PlainReplacement::Confirmed);
        TEST_SERVER.with(|s| *s.borrow_mut() = None);

        assert!(matches!(refused, Err(MailError::InvalidTarget(_))), "remplacement en clair refusé : {refused:?}");
        assert_eq!(sent_before_confirmation, 0, "rien de déposé sur le serveur");
        assert!(confirmed.is_ok(), "remplacement demandé par l'utilisateur : {confirmed:?}");
    }

    #[test]
    fn p2_reste_a_telecharger_recompte_le_seul_dossier_en_cours() {
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let router = Router::new(vec![config("mailo")], Arc::clone(&cache));
        let header = |uid: u32| Header {
            uid,
            from_name: String::new(),
            from_email: "a@example.com".into(),
            subject: format!("n° {uid}"),
            preview: String::new(),
            date: i64::from(uid),
            flags: Flags::default(),
            has_attachments: false,
        };
        lock_cache(&cache).upsert_headers("mailo", "INBOX", &[header(1), header(2)]).unwrap();
        let mut st = Prefetch {
            queue: std::collections::VecDeque::new(),
            synced: HashSet::default(),
            backfilled: HashSet::default(),
            resync: false,
            counts: [("mailo:INBOX".to_string(), 0), ("mailo:Archive".to_string(), 7)].into_iter().collect(),
        };
        assert_eq!(router.remaining_after(&mut st, "mailo:INBOX"), 9, "2 recomptés dans INBOX, 7 gardés pour Archive");
    }

    #[test]
    fn p7_export_mbox_par_taille() {
        const MO: u32 = 1 << 20;
        // Ordre du serveur quelconque : tranches du plus ancien au plus récent.
        let sizes = vec![(4, 2 * MO), (1, 5 * MO), (2, 6 * MO), (3, 40 * MO), (5, 1), (6, 1), (7, 1)];
        let chunks = export_chunks(sizes, 16 << 20, 3);
        assert_eq!(chunks, vec![vec![1, 2], vec![3], vec![4, 5, 6], vec![7]], "16 Mo ou 3 messages au plus ; un gros message seul");
        assert_eq!(export_chunks(Vec::new(), 16 << 20, 3), Vec::<Vec<u32>>::new());
    }

    #[test]
    fn j11b_compte_sans_lecture_hors_connexion() {
        // En-têtes en cache, contenu absent : rien à télécharger d'avance, et
        // l'étape de téléchargement ne demande aucun message au serveur.
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let tree = build_folders("mailo", vec![entry("INBOX", &[])]);
        lock_cache(&cache).save_folders("mailo", &tree).unwrap();
        let header = Header {
            uid: 1,
            from_name: String::new(),
            from_email: "a@example.com".into(),
            subject: "n° 1".into(),
            preview: String::new(),
            date: 1,
            flags: Flags::default(),
            has_attachments: false,
        };
        lock_cache(&cache).upsert_headers("mailo", "INBOX", &[header]).unwrap();
        let router = Router::new(vec![AccountConfig { offline_bodies: false, ..config("mailo") }], Arc::clone(&cache));
        assert_eq!(router.count_without_body("mailo:INBOX"), 0);
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        TEST_SERVER.with(|s| *s.borrow_mut() = Some(Box::new(move || Scripted::open(None, false, String::new(), &log))));
        // Tour déjà commencé sur ce seul dossier (pas de relecture de l'arborescence).
        *guard(&router.prefetch) = Some(Prefetch {
            queue: ["mailo:INBOX".to_string()].into_iter().collect(),
            synced: HashSet::default(),
            backfilled: HashSet::default(),
            resync: false,
            counts: std::collections::HashMap::new(),
        });
        let report = router.prefetch_step(false);
        TEST_SERVER.with(|s| *s.borrow_mut() = None);
        let report = report.expect("étape sans erreur");
        assert!(report.done && report.fetched == 0 && report.remaining == 0, "{report:?}");
        assert_eq!(count(&received, "UID FETCH"), 0, "aucun contenu demandé : {:?}", received.lock().unwrap());
        // Activé : le même message est à télécharger.
        let router = Router::new(vec![config("mailo")], Arc::clone(&cache));
        assert_eq!(router.count_without_body("mailo:INBOX"), 1);
    }

    #[test]
    fn p4_suppression_definitive_groupee_par_dossier() {
        // Trois messages d'« Éléments supprimés » et un du courrier indésirable :
        // une commande par dossier, plus un aller-retour par message.
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let tree = build_folders("mailo", vec![entry("INBOX", &[]), entry("Trash", &["\\Trash"]), entry("Junk", &["\\Junk"])]);
        lock_cache(&cache).save_folders("mailo", &tree).unwrap();
        let router = Router::new(vec![config("mailo")], Arc::clone(&cache));
        let id = |mailbox: &str, uid: u32| lock_cache(&cache).id_for(ImapRef { account: "mailo".into(), mailbox: mailbox.into(), uid });
        let ids = [id("Trash", 5), id("Trash", 6), id("Junk", 2), id("Trash", 9)];
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        TEST_SERVER.with(|s| *s.borrow_mut() = Some(Box::new(move || Scripted::open(None, false, String::new(), &log))));
        let removed = router.delete_forever(&ids);
        let refused = router.delete_forever(&[id("INBOX", 1)]);
        TEST_SERVER.with(|s| *s.borrow_mut() = None);

        assert_eq!(removed.unwrap(), 4);
        assert_eq!(count(&received, "UID STORE"), 2, "un STORE par dossier : {:?}", received.lock().unwrap());
        assert_eq!(count(&received, "UID EXPUNGE"), 2, "un EXPUNGE par dossier");
        assert!(received.lock().unwrap().iter().any(|c| c.starts_with("> UID STORE 5:6,9 ")), "{:?}", received.lock().unwrap());
        assert!(matches!(refused, Err(MailError::InvalidTarget(_))), "hors corbeille : refusé");
    }

    #[test]
    fn s7_arborescence_profondeur_bornee() {
        let names: Vec<String> = (1..=200).map(|n| vec!["d"; n].join("/")).collect();
        let entries = names.iter().map(|n| entry(n, &[])).collect();
        let folders = build_folders("mailo", entries);
        assert_eq!(folders.len(), usize::try_from(MAX_FOLDER_DEPTH).unwrap(), "niveaux au-delà ignorés");
        assert!(folders.iter().all(|f| f.depth < MAX_FOLDER_DEPTH));
    }

    #[test]
    fn b2_message_ajoute_pendant_la_synchronisation_garde() {
        // Cache lu au début : 1, 2, 3 ; le serveur n'a plus le 2. Un brouillon
        // (UID 9) enregistré pendant l'échange n'est pas dans `cached` : gardé.
        assert_eq!(missing_since(&[1, 2, 3], &[1, 3, 9]), [2]);
        assert_eq!(missing_since(&[1, 2], &[]), [1, 2], "dossier vidé sur le serveur");
    }

    #[test]
    fn b8_renommage_rejoue_apres_une_coupure() {
        // Le premier RENAME avait abouti avant la coupure : le second est refusé,
        // mais « Archives » existe et « Clients » n'existe plus, c'est fait.
        let (mut s, _) = Scripted::refusing(&["RENAME", "STATUS \"Clients\""]);
        let mut attempts = 1;
        assert!(rename_once(&mut s, &mut attempts, "Clients", "Archives").is_ok(), "renommage déjà fait");
        // Premier essai refusé, ou ancien nom encore présent : l'erreur remonte.
        let (mut s, _) = Scripted::refusing(&["RENAME"]);
        let mut attempts = 0;
        assert!(rename_once(&mut s, &mut attempts, "Clients", "Archives").is_err(), "premier essai refusé");
        let (mut s, _) = Scripted::refusing(&["RENAME"]);
        let mut attempts = 1;
        assert!(rename_once(&mut s, &mut attempts, "Clients", "Archives").is_err(), "« Clients » existe encore");
    }

    #[test]
    fn y4_deplacement_refuse_au_retour_du_reseau() {
        // Hors ligne, un ancien message a été déplacé vers « Projets » (retiré
        // d'avance de la boîte) ; « Projets » a été supprimé depuis un autre
        // logiciel. Au retour du réseau, le serveur refuse : le message réapparaît.
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let tree = build_folders("mailo", vec![entry("INBOX", &[]), entry("Projets", &[])]);
        lock_cache(&cache).save_folders("mailo", &tree).unwrap();
        let router = Router::new(vec![config("mailo")], Arc::clone(&cache));
        lock_cache(&cache)
            .pending_push(&crate::cache::PendingAction {
                id: 0,
                kind: "move".into(),
                account: "mailo".into(),
                mailbox: "INBOX".into(),
                uids: vec![5],
                arg: "mailo:Projets".into(),
                value: false,
            })
            .unwrap();
        let header = "From: Marie <marie@example.com>\r\nSubject: Ancien message\r\nDate: Mon, 2 Nov 2026 10:00:00 +0100\r\n\r\n";
        let fetch = format!(
            "* 1 FETCH (UID 5 FLAGS (\\Seen) INTERNALDATE \"02-Nov-2026 10:00:00 +0100\" RFC822.SIZE 120 \
             BODY[HEADER.FIELDS (FROM TO SUBJECT DATE)] {{{}}}\r\n{header} BODY[TEXT]<0> {{6}}\r\nCorps\n)\r\n",
            header.len()
        );
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        TEST_SERVER.with(|s| *s.borrow_mut() = Some(Box::new(move || Scripted::open(None, true, fetch.clone(), &log))));
        let report = router.flush_pending(&Mutex::new(Journal::default()));
        TEST_SERVER.with(|s| *s.borrow_mut() = None);

        assert_eq!(report.done, 1, "action retirée de la file");
        assert!(received.lock().unwrap().iter().any(|c| c == "UID MOVE"), "déplacement tenté");
        let list = lock_cache(&cache).list("mailo", "INBOX").unwrap();
        assert!(list.iter().any(|m| m.subject == "Ancien message"), "le message est revenu dans la boîte : {list:?}");
    }

    #[test]
    fn restaurer_dans_le_dossier_d_origine() {
        // « Clients » → « Éléments supprimés » : le dossier d'origine est noté
        // par Message-ID, et « Restaurer » l'y remet ; sa copie au même
        // Message-ID supprimée d'« Éléments envoyés » y retourne ; un message
        // supprimé ailleurs (origine inconnue) revient dans la boîte de réception.
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let tree = build_folders(
            "mailo",
            vec![entry("INBOX", &[]), entry("Clients", &[]), entry("Sent", &["\\Sent"]), entry("Trash", &["\\Trash"])],
        );
        lock_cache(&cache).save_folders("mailo", &tree).unwrap();
        let router = Router::new(vec![config("mailo")], Arc::clone(&cache));
        let id = |mailbox: &str, uid: u32| lock_cache(&cache).id_for(ImapRef { account: "mailo".into(), mailbox: mailbox.into(), uid });
        let item = |uid: u32, header: &str| {
            format!("* {uid} FETCH (UID {uid} BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{header})\r\n", header.len())
        };
        let fetch = format!(
            "{}{}{}",
            item(5, "Message-ID: <m5@example.com>\r\n\r\n"),
            item(6, "Message-ID: <ailleurs@example.com>\r\n\r\n"),
            item(7, "Message-ID: <m5@example.com>\r\n\r\n")
        );
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        TEST_SERVER.with(|s| *s.borrow_mut() = Some(Box::new(move || Scripted::open(None, false, fetch.clone(), &log))));

        router.native_move(&[id("Clients", 5)], "mailo:Trash").unwrap();
        assert_eq!(lock_cache(&cache).trash_origins("mailo", "<m5@example.com>").unwrap(), ["Clients"]);
        router.native_move(&[id("Sent", 7)], "mailo:Trash").unwrap();
        assert_eq!(lock_cache(&cache).trash_origins("mailo", "<m5@example.com>").unwrap().len(), 2, "les deux dossiers notés");
        assert_eq!(
            lock_cache(&cache).trash_origins("mailo", "<ailleurs@example.com>").unwrap(),
            Vec::<String>::new(),
            "seuls les messages déplacés"
        );

        received.lock().unwrap().clear();
        let report = router.restore(&Mutex::new(Journal::default()), &[id("Trash", 5), id("Trash", 6), id("Trash", 7)]);
        let refused = router.restore(&Mutex::new(Journal::default()), &[id("INBOX", 9)]);
        TEST_SERVER.with(|s| *s.borrow_mut() = None);

        let report = report.unwrap();
        assert_eq!(report.to_inbox, 1, "{report:?}");
        assert!(report.report.failed.is_empty(), "{report:?}");
        let received = received.lock().unwrap().clone();
        let moves: Vec<&String> = received.iter().filter(|c| c.starts_with("> UID MOVE")).collect();
        assert!(
            moves.iter().any(|c| c.contains("Clients")) && moves.iter().any(|c| c.contains("Sent")),
            "une copie par dossier : {moves:?}"
        );
        assert!(!moves.iter().any(|c| c.starts_with("> UID MOVE 5,7 ")), "pas les deux au même endroit : {moves:?}");
        assert!(moves.iter().any(|c| c.starts_with("> UID MOVE 6 ") && c.contains("INBOX")), "{moves:?}");
        assert_eq!(
            lock_cache(&cache).trash_origins("mailo", "<m5@example.com>").unwrap(),
            Vec::<String>::new(),
            "note oubliée une fois restauré"
        );
        assert!(matches!(refused, Err(MailError::InvalidTarget(_))), "hors des éléments supprimés : {refused:?}");
    }

    #[test]
    fn y3_dossier_jamais_synchronise_relu_entierement() {
        assert!(full_sync(None, Some(7)), "jamais synchronisé : passage complet, même avec des messages en cache");
        assert!(full_sync(Some(6), Some(7)), "renuméroté");
        assert!(!full_sync(Some(7), Some(7)), "déjà synchronisé : seulement les nouveautés");
        assert!(!full_sync(None, None), "serveur sans UIDVALIDITY : synchronisation habituelle");
    }

    #[test]
    fn network_errors_in_every_language() {
        for m in [
            "serveur « imap.x.fr » introuvable : failed to lookup",
            "server “imap.x.fr” not found: failed to lookup",
            "servidor “imap.x.fr” no encontrado: x",
            "connexion à imap.x.fr:993 impossible : refusé",
            "could not connect to imap.x.fr:993: refused",
            "no se puede conectar a imap.x.fr:993: x",
            "erreur réseau : broken pipe",
            "error de red: x",
            "SMTP: connection closed by the server",
        ] {
            assert!(is_network_error(m), "{m}");
        }
        // Un message ou un compte introuvable n'est pas une panne réseau.
        assert!(!is_network_error("introuvable : compte imap-1"));
        assert!(!is_network_error("élément introuvable sur Outlook.com"));
        assert!(!is_network_error("refusé par le serveur : [AUTHENTICATIONFAILED]"));
        // Refus définitif dont le texte parle de délai : pas une panne (n° 32).
        assert!(!is_network_error("erreur serveur : refusé par le serveur : [UNAVAILABLE] Backend timed out"));
        assert!(!is_network_error("server error: refused by the server: [UNAVAILABLE] network backend down"));
        assert!(!is_network_error("Le dossier INBOX a été renuméroté par le serveur : action annulée"));
    }

    fn selected(exists: u32, validity: u32, modseq: Option<u64>) -> SelectInfo {
        SelectInfo { exists, uid_validity: Some(validity), uid_next: Some(100), highest_modseq: modseq, ..SelectInfo::default() }
    }

    #[test]
    fn incremental_sync_only_when_safe() {
        let info = selected(10, 7, Some(500));
        // Rien de mémorisé, serveur sans CONDSTORE, NOMODSEQ, autre UIDVALIDITY, MODSEQ en recul.
        assert_eq!(flag_plan(Some(7), None, &info, true, true), FlagPlan::Full);
        assert_eq!(flag_plan(Some(7), Some((500, Some(10))), &info, false, false), FlagPlan::Full);
        let nomodseq = SelectInfo { no_modseq: true, highest_modseq: None, ..info };
        assert_eq!(flag_plan(Some(7), Some((500, Some(10))), &nomodseq, true, false), FlagPlan::Full);
        assert_eq!(flag_plan(Some(6), Some((500, Some(10))), &info, true, true), FlagPlan::Full);
        assert_eq!(flag_plan(None, Some((500, Some(10))), &info, true, true), FlagPlan::Full);
        assert_eq!(flag_plan(Some(7), Some((600, Some(10))), &info, true, true), FlagPlan::Full);
        assert_eq!(flag_plan(Some(7), Some((500, Some(10))), &selected(10, 7, None), true, true), FlagPlan::Full);
    }

    #[test]
    fn unchanged_modseq_skips_the_flags_fetch() {
        let info = selected(10, 7, Some(500));
        assert_eq!(flag_plan(Some(7), Some((500, Some(10))), &info, true, true), FlagPlan::Unchanged);
        assert_eq!(flag_plan(Some(7), Some((500, Some(10))), &info, true, false), FlagPlan::Unchanged);
        // Même MODSEQ mais un message de moins : un EXPUNGE à détecter, sans relire les états.
        assert_eq!(flag_plan(Some(7), Some((500, Some(11))), &info, true, false), FlagPlan::Condstore { since: 500, fetch_flags: false });
        assert_eq!(flag_plan(Some(7), Some((500, None)), &info, true, true), FlagPlan::Qresync { since: 500 });
    }

    #[test]
    fn changed_modseq_fetches_only_changes() {
        let info = selected(12, 7, Some(520));
        assert_eq!(flag_plan(Some(7), Some((500, Some(10))), &info, true, true), FlagPlan::Qresync { since: 500 });
        assert_eq!(flag_plan(Some(7), Some((500, Some(10))), &info, true, false), FlagPlan::Condstore { since: 500, fetch_flags: true });
    }

    #[test]
    fn j14_recherche_bornee_malgre_un_dossier_lent() {
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let router = Router::new(vec![config("mailo")], Arc::clone(&cache));
        let account = Arc::clone(&router.all_accounts()[0]);
        guard(&account.state).folders = build_folders("mailo", vec![entry("INBOX", &[]), entry("Lent", &[]), entry("Factures", &[])]);
        let header = "Message-ID: <devis@x>\r\nSubject: Devis\r\n\r\n";
        let fetch = format!("* 1 FETCH (UID 5 BODY[HEADER.FIELDS (MESSAGE-ID SUBJECT)] {{{}}}\r\n{header})\r\n", header.len());
        let delay = std::time::Duration::from_millis(900);
        let (session, received) = Scripted::slow(Some(5), fetch, "Lent", delay);
        *account.sync.lock().unwrap() = Some(session);

        let started = std::time::Instant::now();
        let query = SearchQuery::parse("devis").unwrap();
        let report = router.search_within(&query, std::time::Duration::from_millis(250));
        assert!(started.elapsed() < delay, "rendu sans attendre le dossier lent : {:?}", started.elapsed());
        let selected: Vec<String> = received.lock().unwrap().iter().filter(|l| l.starts_with("> SELECT")).cloned().collect();
        assert!(selected.first().is_some_and(|l| l.contains("INBOX")), "boîte de réception d'abord : {selected:?}");
        assert!(selected.get(1).is_some_and(|l| l.contains("Factures")), "puis par nom : {selected:?}");
        assert_eq!(report.results.len(), 1, "même Message-ID dans deux dossiers : un seul résultat");
        assert_eq!(report.unfinished, 1, "« Lent » non parcouru");
        assert_eq!(report.errors.len(), 0, "{:?}", report.errors);
        // Le dossier lent fini, la connexion de synchronisation est rendue.
        std::thread::sleep(delay + std::time::Duration::from_millis(300));
        assert!(account.sync.try_lock().is_ok(), "connexion de synchronisation libre");
    }

    #[test]
    fn n2_dossier_lent_arrete_a_l_echeance() {
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let router = Router::new(vec![config("mailo")], Arc::clone(&cache));
        let account = Arc::clone(&router.all_accounts()[0]);
        guard(&account.state).folders = build_folders("mailo", vec![entry("INBOX", &[]), entry("Lent", &[])]);
        let delay = std::time::Duration::from_secs(5);
        let tick = std::time::Duration::from_millis(20);
        let (mut session, _) = Scripted::slow_with(None, String::new(), "Lent", delay, Some(tick));
        session.set_patience(Some(std::time::Duration::from_mins(1)));
        *account.sync.lock().unwrap() = Some(session);

        let started = std::time::Instant::now();
        let report = router.search_within(&SearchQuery::parse("devis").unwrap(), std::time::Duration::from_millis(200));
        assert_eq!(report.unfinished, 1, "« Lent » non parcouru");
        assert_eq!(report.errors.len(), 0, "arrêt à l'échéance : pas une erreur du compte ({:?})", report.errors);
        // Arrêté un délai de lecture après l'échéance, pas à la fin du dossier lent.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let sync = account.sync.try_lock().expect("connexion de synchronisation libre bien avant la fin du dossier lent");
        assert!(sync.is_none(), "connexion abandonnée : la réponse est restée à moitié lue");
        assert!(started.elapsed() < delay, "{:?}", started.elapsed());
    }

    #[test]
    fn j14_dossiers_les_plus_utiles_d_abord() {
        let mut roles = [FolderRole::Custom, FolderRole::Archive, FolderRole::Inbox, FolderRole::Drafts, FolderRole::Sent];
        roles.sort_by_key(|r| search_rank(*r));
        assert_eq!(roles, [FolderRole::Inbox, FolderRole::Sent, FolderRole::Archive, FolderRole::Drafts, FolderRole::Custom]);
    }

    fn entry(name: &str, attrs: &[&str]) -> (ListEntry, Option<MailboxStatus>) {
        (
            ListEntry { name: name.into(), delimiter: Some('/'), attributes: attrs.iter().map(std::string::ToString::to_string).collect() },
            Some(MailboxStatus { messages: 4, unseen: 1, ..Default::default() }),
        )
    }

    #[test]
    fn unsubscribed_custom_folders_are_marked() {
        let mut folders =
            build_folders("mailo", vec![entry("INBOX", &[]), entry("Trash", &["\\Trash"]), entry("ecard", &[]), entry("Factures", &[])]);
        mark_subscriptions(&mut folders, &["Factures".to_string()]);
        let view: Vec<(&str, bool)> = folders.iter().map(|f| (f.path.as_str(), f.subscribed)).collect();
        assert_eq!(view, vec![("INBOX", true), ("Trash", true), ("ecard", false), ("Factures", true)]);
    }

    #[test]
    fn attribut_du_serveur_avant_le_nom() {
        // Relecture complète n° 6 : un libellé « Trash » listé avant le vrai
        // dossier `\Trash` ne lui prend pas son rôle ; un sous-dossier
        // « Archive » reste un dossier ordinaire.
        let folders = build_folders(
            "gmail",
            vec![
                entry("INBOX", &[]),
                entry("Trash", &[]),
                entry("Clients/Archive", &[]),
                entry("Clients", &["\\HasChildren"]),
                entry("[Gmail]/Corbeille", &["\\Trash"]),
                entry("[Gmail]", &["\\Noselect", "\\HasChildren"]),
            ],
        );
        let role = |path: &str| folders.iter().find(|f| f.path == path).map(|f| f.role);
        assert_eq!(role("[Gmail]/Corbeille"), Some(FolderRole::Trash), "l'attribut l'emporte");
        assert_eq!(role("Trash"), Some(FolderRole::Custom), "le libellé personnel reste ordinaire");
        assert_eq!(role("Clients/Archive"), Some(FolderRole::Custom), "pas de rôle deviné sous un dossier");
    }

    #[test]
    fn folder_tree_with_special_folders_first() {
        let folders = build_folders(
            "mailo",
            vec![
                entry("Archives", &[]),
                entry("Clients/Soci&AOk-t&AOk-", &[]),
                entry("Trash", &["\\Trash"]),
                entry("INBOX/Factures", &[]),
                entry("Envoy&AOk-s", &[]),
                entry("Clients", &["\\HasChildren"]),
                entry("INBOX", &[]),
                entry("Drafts", &["\\Drafts"]),
                entry("Spam", &[]),
                entry("Corbeille", &[]), // second dossier « corbeille » : reste personnalisé
            ],
        );
        let view: Vec<(String, u32, FolderRole)> = folders.iter().map(|f| (f.name.clone(), f.depth, f.role)).collect();
        assert_eq!(
            view,
            vec![
                ("Boîte de réception".into(), 0, FolderRole::Inbox),
                ("Factures".into(), 1, FolderRole::Custom),
                ("Brouillons".into(), 0, FolderRole::Drafts),
                ("Éléments envoyés".into(), 0, FolderRole::Sent),
                ("Archive".into(), 0, FolderRole::Archive),
                ("Éléments supprimés".into(), 0, FolderRole::Trash),
                ("Courrier indésirable".into(), 0, FolderRole::Junk),
                ("Clients".into(), 0, FolderRole::Custom),
                ("Société".into(), 1, FolderRole::Custom),
                ("Corbeille".into(), 0, FolderRole::Custom),
            ]
        );
        let societe = folders.iter().find(|f| f.name == "Société").unwrap();
        assert_eq!(societe.id, "mailo:Clients/Soci&AOk-t&AOk-");
        assert_eq!(societe.parent_id.as_deref(), Some("mailo:Clients"));
        assert_eq!((societe.unread, societe.total), (1, 4));
    }

    fn config(id: &str) -> AccountConfig {
        use crate::config::{AuthMethod, Security, ServerConfig};
        let server = ServerConfig {
            host: "mail.example.com".into(),
            port: 993,
            security: Security::SslTls,
            auth: AuthMethod::Normal,
            username: "moi@example.com".into(),
            password: String::new(),
            pinned_cert: String::new(),
        };
        AccountConfig {
            id: id.into(),
            name: "Mailo".into(),
            email: "moi@example.com".into(),
            display_name: "Fabrice".into(),
            color: "#0f6cbd".into(),
            imap: server.clone(),
            smtp: server,
            smtp_same_credentials: true,
            oauth_client_id: String::new(),
            dav_url: String::new(),
            card_url: String::new(),
            offline_bodies: true,
        }
    }

    #[test]
    fn startup_uses_cached_folders_and_publishes_accounts() {
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let tree = build_folders("mailo", vec![entry("INBOX", &[]), entry("Archives", &[])]);
        lock_cache(&cache).save_folders("mailo", &tree).unwrap();

        let router = Router::new(vec![config("mailo")], Arc::clone(&cache));
        assert_eq!(router.folder("mailo:INBOX").map(|f| f.role), Some(FolderRole::Inbox));
        assert_eq!(router.folder_by_role("mailo", FolderRole::Archive).as_deref(), Some("mailo:Archives"));
        // Lisible sans le verrou du routeur :
        let c = lock_cache(&cache);
        assert_eq!(c.accounts().unwrap().len(), 1);
        assert_eq!(c.all_folders().unwrap().len(), 2);
    }

    #[test]
    fn seuls_les_brouillons_sont_repris_ou_abandonnes() {
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let tree = build_folders("a", vec![entry("INBOX", &[]), entry("Drafts", &["\\Drafts"])]);
        lock_cache(&cache).save_folders("a", &tree).unwrap();
        let router = Router::new(vec![config("a")], Arc::clone(&cache));
        let inbox = lock_cache(&cache).id_for(ImapRef { account: "a".into(), mailbox: "INBOX".into(), uid: 1 });
        let draft = lock_cache(&cache).id_for(ImapRef { account: "a".into(), mailbox: "Drafts".into(), uid: 2 });

        assert_eq!(router.ensure_draft(draft).unwrap().uid, 2);
        // Refusé avant toute connexion : ni lecture, ni suppression d'un message reçu.
        assert!(matches!(router.ensure_draft(inbox), Err(MailError::InvalidTarget(_))));
        assert!(matches!(router.draft_content(inbox), Err(MailError::InvalidTarget(_))));
        assert!(matches!(router.discard_draft(inbox), Err(MailError::InvalidTarget(_))));
        assert_eq!(router.message_location(inbox).unwrap(), "a:INBOX");
    }

    #[test]
    fn h1_apercu_d_une_reponse_d_invitation() {
        // Réponse d'invitation dont le calendrier dépasse les 2 048 octets lus pour l'aperçu.
        let ics = format!("BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\n{}END:VCALENDAR\r\n", "X-REMPLISSAGE:0123456789abcdef\r\n".repeat(120));
        let draft = Draft {
            from_email: "moi@exemple.fr".into(),
            to: vec!["marie@exemple.fr".into()],
            subject: "Accepté : Test".into(),
            html: "<p>moi@exemple.fr a accepté votre invitation.</p>".into(),
            calendar: Some(compose::OutgoingCalendar { method: "REPLY".into(), ics }),
            ..Draft::default()
        };
        let raw = compose::build(&draft, 1_700_000_000, "<id@mailo.com>");
        assert_eq!(mimeparse::preview(&mimeparse::parse(&raw)), "moi@exemple.fr a accepté votre invitation.", "message entier");
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let mut sample = raw[..split].to_vec();
        sample.extend_from_slice(preview_sample(&raw[split..split + PREVIEW_BYTES]));
        assert_eq!(mimeparse::preview(&parse_for_preview(&sample)), "moi@exemple.fr a accepté votre invitation.", "début seulement");
    }

    #[test]
    fn o_apercu_sans_la_ligne_vide_des_en_tetes() {
        // Message court envoyé par Vortex Mail : texte « test » et sa version HTML.
        let draft = Draft {
            from_email: "fabrice_test@mailo.com".into(),
            to: vec!["fabrice_test@mailo.com".into()],
            subject: "test".into(),
            html: "<p>test</p>".into(),
            ..Draft::default()
        };
        let raw = compose::build(&draft, 1_700_000_000, "<o@mailo.com>");
        let blank = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("fin des en-têtes");
        let text = &raw[blank + 4..];
        // En-têtes sans leur ligne vide, comme certains serveurs les rendent pour `HEADER.FIELDS`.
        let headers = &raw[..blank + 2];
        let mut broken = headers.to_vec();
        broken.extend_from_slice(text);
        let before = mimeparse::preview(&parse_for_preview(&broken));
        assert!(before.contains("Content-Type"), "défaut reproduit (aperçu d'avant le lot O) : {before}");
        for headers in [headers, &raw[..blank], &raw[..blank + 4]] {
            let mut sample = header_block(headers);
            sample.extend_from_slice(text);
            let end = String::from_utf8_lossy(&headers[headers.len().saturating_sub(4)..]);
            assert_eq!(mimeparse::preview(&parse_for_preview(&sample)), "test", "fin des en-têtes : {end:?}");
        }
        assert_eq!(header_block(b""), b"\r\n", "sans en-têtes : le texte reste le corps");
    }

    #[test]
    fn apercu_d_une_partie_base64_coupee() {
        let headers = b"From: a@x.fr\r\nSubject: TR\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n";
        let body = "Bonjour, voici le message transf\u{e9}r\u{e9}.\r\n".repeat(80);
        let encoded = compose::base64(body.as_bytes());
        let lines: Vec<&str> = encoded.as_bytes().chunks(76).map(|c| std::str::from_utf8(c).unwrap()).collect();
        let text = format!(
            "--b\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--b--\r\n",
            lines.join("\r\n")
        );
        // Ce que renvoie BODY.PEEK[TEXT]<0.2048> : coupé au milieu d'une ligne.
        let cut = &text.as_bytes()[..PREVIEW_BYTES];
        assert!(!cut.ends_with(b"\n"));
        let sample = preview_sample(cut);
        assert!(sample.ends_with(b"\n"));
        let mut raw = headers.to_vec();
        raw.extend_from_slice(sample);
        let preview = mimeparse::preview(&parse_for_preview(&raw));
        assert!(preview.starts_with("Bonjour, voici le message transf\u{e9}r\u{e9}."), "{preview}");
        // Texte complet, plus court que la limite : gardé tel quel.
        assert_eq!(preview_sample(b"court"), b"court");
        // Un texte ordinaire n'est jamais pris pour du base64.
        assert_eq!(mimeparse::decode_if_base64("Bonjour,\nvoici le compte rendu de la r\u{e9}union."), None);
        assert_eq!(mimeparse::decode_if_base64("ok"), None);
        assert!(mimeparse::decode_if_base64("Qm9uam91ciwgdm9pY2kgbGUgbWVzc2FnZSB0cmFuc2bDqXLDqS4N\nCkJvbmpvdXIs")
            .is_some_and(|t| t.starts_with("Bonjour, voici")));
    }

    #[test]
    fn removing_an_account_clears_its_cache() {
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let router = Router::new(vec![config("a"), config("b")], Arc::clone(&cache));
        lock_cache(&cache).save_folders("a", &build_folders("a", vec![entry("INBOX", &[])])).unwrap();
        let id = lock_cache(&cache).id_for(ImapRef { account: "a".into(), mailbox: "INBOX".into(), uid: 1 });
        assert_eq!(router.message_location(id).unwrap(), "a:INBOX");

        assert!(router.remove_account("a"));
        let c = lock_cache(&cache);
        assert_eq!(c.folders("a").unwrap().len(), 0);
        assert!(c.imap_ref(id).is_none());
        assert_eq!(c.accounts().unwrap().iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["b"]);
    }
}
