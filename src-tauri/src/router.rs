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
    Account, Flags, Folder, FolderId, FolderRole, Journal, MailBackend, MailError, Message, MessageId, MessageSummary, MoveReport,
    RawMessage,
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
    fn run<T>(&self, lane: Lane, mut op: impl FnMut(&mut Session) -> imap_lite::Result<T>) -> Result<T, String> {
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
                    if attempt >= 2 {
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
                    s.uid_fetch_set(&format!("{}:*", max + 1), LIST_QUERY)?
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

    let mut c = lock_cache(cache);
    if validity_changed {
        c.reset_folder(&account_id, &mailbox).map_err(backend)?;
    }
    if let Some(v) = info.uid_validity {
        c.set_uid_validity(&account_id, &mailbox, v).map_err(backend)?;
    }
    let headers: Vec<Header> = new_items.iter().filter_map(header_from).collect();
    c.upsert_headers(&account_id, &mailbox, &headers).map_err(backend)?;

    let item_flags = |items: &[FetchItem]| -> Vec<(u32, Flags)> { items.iter().filter_map(|i| Some((i.uid()?, flags_from(i)))).collect() };
    match refresh {
        Refresh::Nothing => {}
        Refresh::Full(known_items) => {
            if let Some(&min) = cached_uids.first() {
                let present: Vec<u32> = known_items.iter().filter_map(FetchItem::uid).collect();
                c.remove_missing(&account_id, &mailbox, min, &present).map_err(backend)?;
                c.update_flags(&account_id, &mailbox, &item_flags(&known_items)).map_err(backend)?;
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
            c.delete_messages(&account_id, &mailbox, &gone).map_err(backend)?;
            c.update_flags(&account_id, &mailbox, &item_flags(&changed)).map_err(backend)?;
        }
    }
    // En dernier, une fois le cache à jour : point de départ de la prochaine fois.
    let modseq = info.highest_modseq.filter(|_| condstore && !info.no_modseq);
    c.set_modseq_state(&account_id, &mailbox, modseq, info.exists).map_err(backend)?;
    Ok(())
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

    let mut c = lock_cache(cache);
    if validity_changed {
        c.reset_folder(account_id, mailbox).map_err(backend)?;
    }
    if let Some(v) = info.uid_validity {
        c.set_uid_validity(account_id, mailbox, v).map_err(backend)?;
    }
    let headers: Vec<Header> = new_items.iter().filter_map(header_from).collect();
    c.upsert_headers(account_id, mailbox, &headers).map_err(backend)?;
    if !validity_changed {
        // Revenu dans la réception (ou supprimé) : retiré de l'archive.
        let archived: std::collections::HashSet<u32> = archived.into_iter().collect();
        let gone: Vec<u32> = cached.iter().filter(|u| !archived.contains(u)).copied().collect();
        c.delete_messages(account_id, mailbox, &gone).map_err(backend)?;
        let flags: Vec<(u32, Flags)> = flag_items.iter().filter_map(|i| Some((i.uid()?, flags_from(i)))).collect();
        c.update_flags(account_id, mailbox, &flags).map_err(backend)?;
    }
    // Liste recalculée à chaque passage : pas de synchronisation incrémentale ici.
    c.set_modseq_state(account_id, mailbox, None, info.exists).map_err(backend)?;
    Ok(())
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

fn build_folders(account_id: &str, entries: Vec<(ListEntry, Option<MailboxStatus>)>) -> Vec<Folder> {
    fn visit(
        parent: Option<&str>,
        depth: u32,
        nodes: &[(ListEntry, Option<MailboxStatus>, FolderRole)],
        parent_of: &dyn Fn(&ListEntry) -> Option<String>,
        account_id: &str,
        out: &mut Vec<Folder>,
    ) {
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

/// En-tête à mettre en cache, depuis une réponse FETCH de la liste.
fn header_from(item: &FetchItem) -> Option<Header> {
    let uid = item.uid()?;
    let headers = item.body("BODY[HEADER").unwrap_or_default();
    let mut sample = headers.to_vec();
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
        let router = Self { accounts: RwLock::new(accounts), cache, prefetch: Mutex::new(None) };
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
        items
            .into_iter()
            .find(|i| i.uid() == Some(uid))
            .ok_or_else(|| MailError::NotFound(tr!("message supprimé entre-temps sur le serveur").into()))
    }

    /// Télécharge un message absent du cache, l'affiche et le garde pour la suite.
    pub fn message(&self, id: MessageId) -> Result<Message, MailError> {
        let r = self.imap_ref(id)?;
        let item = self.fetch_full(&r)?;
        let raw = item.body("BODY[]").unwrap_or_default();
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

    fn offline_remaining(&self) -> u32 {
        let c = self.cache();
        self.offline_folders().iter().filter_map(|f| c.count_without_body(&f.account_id, &f.path).ok()).sum()
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
            *tour = Some(Prefetch {
                queue: self.offline_folders().into_iter().map(|f| f.id).collect(),
                synced: HashSet::default(),
                backfilled: HashSet::default(),
                resync,
            });
        }
        loop {
            let st = tour.as_mut().expect("tour en cours");
            let Some(folder_id) = st.queue.front().cloned() else {
                *tour = None;
                return Ok(PrefetchReport { fetched: 0, remaining: self.offline_remaining(), done: true });
            };
            let do_sync = st.resync && !st.synced.contains(&folder_id);
            let do_backfill = st.resync && !do_sync && !st.backfilled.contains(&folder_id);
            if do_sync {
                st.synced.insert(folder_id.clone());
                if let Err(e) = self.sync_folder(&folder_id) {
                    crate::diag::report(&format_args!("hors connexion, {folder_id} : {e}"));
                    st.queue.pop_front();
                }
                return Ok(PrefetchReport { fetched: 0, remaining: self.offline_remaining(), done: false });
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
                return Ok(PrefetchReport { fetched: 0, remaining: self.offline_remaining(), done: false });
            }

            let (account_id, mailbox) = Self::split_folder(&folder_id)?;
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
            // Analyse des messages hors verrou : l'affichage n'attend pas.
            let records: Vec<_> = items.iter().filter_map(|item| Some((item.uid()?, body_record(item.body("BODY[]")?)))).collect();
            let mut fetched = 0;
            {
                let mut c = self.cache();
                for (uid, (parsed, html, extra)) in &records {
                    if c.save_body(&account_id, &mailbox, *uid, &parsed.to, html, extra).is_ok() {
                        fetched += 1;
                    }
                }
            }
            // Messages disparus du serveur entre-temps : on passe au dossier suivant.
            if fetched == 0 {
                if let Some(st) = tour.as_mut() {
                    st.queue.pop_front();
                }
            }
            return Ok(PrefetchReport { fetched, remaining: self.offline_remaining(), done: false });
        }
    }

    /// Contenu d'une pièce jointe, relu sur le serveur.
    pub fn attachment(&self, id: MessageId, index: u32) -> Result<mimeparse::AttachmentData, MailError> {
        let r = self.imap_ref(id)?;
        let item = self.fetch_full(&r)?;
        let raw = item.body("BODY[]").unwrap_or_default();
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
        self.imap_account(account_id)?
            .run(Lane::User, |s| {
                // Un serveur peut refuser de renommer le dossier ouvert.
                if s.selected().is_some_and(|m| m == from || m.starts_with(&prefix)) {
                    s.select("INBOX")?;
                }
                s.rename(&from, &to)?;
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
    /// E8a), du plus ancien au plus récent, par tranches de 50 : la connexion
    /// est rendue entre deux tranches, les autres actions ne l'attendent pas.
    /// Renvoie le nombre de messages écrits.
    ///
    /// # Errors
    /// Dossier inconnu, serveur injoignable, ou écriture du fichier impossible.
    pub fn export_folder(&self, folder_id: &str, out: &mut impl std::io::Write) -> Result<u32, MailError> {
        const CHUNK: usize = 50;
        let (account_id, mailbox) = Self::split_folder(folder_id)?;
        let account = self.imap_account(&account_id)?;
        let known = self.known_validity(&account_id, &mailbox);
        let mut uids = account
            .run(Lane::User, |s| {
                ensure_selected(s, &mailbox, known)?;
                s.uid_search_raw("ALL")
            })
            .map_err(backend)?;
        uids.sort_unstable();
        let mut written = 0u32;
        for chunk in uids.chunks(CHUNK) {
            let mut items = account
                .run(Lane::User, |s| {
                    ensure_selected(s, &mailbox, known)?;
                    s.uid_fetch(chunk, "(UID INTERNALDATE BODY.PEEK[])")
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

    /// Supprime définitivement des messages de la corbeille ou du courrier
    /// indésirable (`\Deleted` puis `UID EXPUNGE`), comme « Supprimer » dans
    /// ces dossiers chez Thunderbird (relecture complète n° 40). Renvoie le
    /// nombre de messages supprimés ; les autres dossiers sont refusés.
    pub fn delete_forever(&self, ids: &[MessageId]) -> Result<u32, MailError> {
        for &id in ids {
            let r = self.imap_ref(id)?;
            let role = self.folder(&format!("{}:{}", r.account, r.mailbox)).map(|f| f.role);
            if !matches!(role, Some(FolderRole::Trash | FolderRole::Junk)) {
                return Err(MailError::InvalidTarget(
                    tr!("Seuls les éléments supprimés et le courrier indésirable peuvent être supprimés définitivement.").into(),
                ));
            }
        }
        let mut removed = 0u32;
        for &id in ids {
            self.remove(id)?;
            removed += 1;
        }
        Ok(removed)
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
    ) -> Result<SavedDraft, MailError> {
        if let Some(old) = replace {
            self.ensure_draft(old)?;
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
    /// [`SearchQuery`], lot E7). Les comptes injoignables sont signalés sans
    /// bloquer les autres.
    pub fn search(&self, query: &SearchQuery) -> SearchReport {
        type PerFolder = Vec<(String, FolderRole, Vec<FetchItem>)>;
        let mut report = SearchReport { results: Vec::new(), errors: Vec::new() };
        if query.is_empty() {
            return report;
        }
        let keys = imap_search_keys(query);
        // (compte, dossier, rôle, en-tête, Message-ID)
        let mut found: Vec<(String, String, FolderRole, Header, String)> = Vec::new();
        // Tous les comptes en même temps, chacun sur sa connexion de synchronisation.
        let accounts = self.all_accounts();
        let per_account: Vec<(String, Result<PerFolder, String>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = accounts
                .iter()
                .map(|account| {
                    let keys = &keys;
                    scope.spawn(move || {
                        let folders: Vec<(String, FolderRole)> = account
                            .folders()
                            .iter()
                            .filter(|f| !matches!(f.role, FolderRole::Trash | FolderRole::Junk) && f.total > 0)
                            .map(|f| (f.path.clone(), f.role))
                            .collect();
                        let result = account.run(Lane::Sync, |s| {
                            let mut out = Vec::new();
                            for (path, role) in &folders {
                                match s.select(path) {
                                    Ok(_) => {}
                                    Err(e) if e.is_connection_lost() => return Err(e),
                                    Err(_) => continue,
                                }
                                let uids = match s.uid_search_keys(keys) {
                                    Ok(uids) => uids,
                                    Err(e) if e.is_connection_lost() => return Err(e),
                                    Err(_) => continue, // critère refusé par ce dossier
                                };
                                let recent: Vec<u32> = uids.iter().rev().take(SEARCH_PER_FOLDER).copied().collect();
                                if recent.is_empty() {
                                    continue;
                                }
                                let items = s.uid_fetch(&recent, SEARCH_QUERY)?;
                                out.push((path.clone(), *role, items));
                            }
                            Ok(out)
                        });
                        (account.config.id.clone(), result)
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        for (account_id, result) in per_account {
            match result {
                Ok(per_folder) => {
                    for (path, role, items) in per_folder {
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
                }
                Err(message) => report.errors.push(AccountError { account_id, message }),
            }
        }
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
        journal: &mut Journal,
        ids: &[MessageId],
        target: Option<&str>,
        role: Option<FolderRole>,
    ) -> Result<MoveReport, MailError> {
        let mut report = match (target, role) {
            (Some(target), _) => mail_core::move_messages(&mut self.backend(), journal, ids, target)?,
            (None, Some(role)) => mail_core::move_to_role(&mut self.backend(), journal, ids, role)?,
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
            let journaled = journal.unfinished().any(|e| e.source_id == f.id);
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
    pub fn flush_pending(&self, journal: &mut Journal) -> FlushReport {
        let mut report = FlushReport::default();
        if journal.unfinished().next().is_some() {
            let resumed = mail_core::resume_moves(&mut self.backend(), journal);
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
                    report.remaining = self.pending_count().saturating_add(u32::try_from(journal.unfinished().count()).unwrap_or(u32::MAX));
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
                            // Déjà copiés, source en cours de suppression : le journal termine.
                            .filter(|id| !journal.unfinished().any(|e| e.source_id == *id))
                            .collect()
                    };
                    if ids.is_empty() {
                        Ok(())
                    } else {
                        match mail_core::move_messages(&mut self.backend(), journal, &ids, &action.arg) {
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
        report.remaining = self.pending_count().saturating_add(u32::try_from(journal.unfinished().count()).unwrap_or(u32::MAX));
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
        self.imap_account(account_id).ok()?.folders().into_iter().find(|f| f.id == id)
    }

    pub fn folder_by_role(&self, account_id: &str, role: FolderRole) -> Option<FolderId> {
        self.imap_account(account_id).ok()?.folders().into_iter().find(|f| f.role == role).map(|f| f.id)
    }

    pub fn message_location(&self, id: MessageId) -> mail_core::Result<FolderId> {
        self.imap_ref(id).map(|r| format!("{}:{}", r.account, r.mailbox))
    }

    pub fn native_move(&self, ids: &[MessageId], target: &str) -> mail_core::Result<Vec<MessageId>> {
        let (_, target_mailbox) = Self::split_folder(target)?;
        let mut groups: Vec<(String, String, Vec<u32>)> = Vec::new();
        for &id in ids {
            let r = self.imap_ref(id)?;
            match groups.iter_mut().find(|g| g.0 == r.account && g.1 == r.mailbox) {
                Some(g) => g.2.push(r.uid),
                None => groups.push((r.account, r.mailbox, vec![r.uid])),
            }
        }
        for (account, mailbox, uids) in groups {
            let known = self.known_validity(&account, &mailbox);
            self.imap_account(&account)?
                .run(Lane::User, |s| {
                    ensure_selected(s, &mailbox, known)?;
                    s.uid_move(&uids, &target_mailbox)
                })
                .map_err(backend)?;
            // Les messages ont quitté ce dossier ; la cible les recevra à sa synchronisation.
            let mut c = self.cache();
            for uid in &uids {
                c.delete_message(&account, &mailbox, *uid).map_err(backend)?;
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
                received: Arc::clone(received),
            };
            let stream: Box<dyn crate::transport::Stream> = Box::new(server);
            imap_lite::Client::connect(stream).expect("accueil du serveur joué")
        }

        fn command(&mut self, text: &str) {
            let mut words = text.split_whitespace();
            let tag = words.next().unwrap_or("*").to_owned();
            let verb = words.next().unwrap_or("").to_ascii_uppercase();
            let next = words.next().unwrap_or("").to_ascii_uppercase();
            self.received.lock().expect("journal du serveur joué").push(format!("{verb} {next}"));
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
        let report = router.flush_pending(&mut Journal::default());
        TEST_SERVER.with(|s| *s.borrow_mut() = None);

        assert_eq!(report.done, 1, "action retirée de la file");
        assert!(received.lock().unwrap().iter().any(|c| c == "UID MOVE"), "déplacement tenté");
        let list = lock_cache(&cache).list("mailo", "INBOX").unwrap();
        assert!(list.iter().any(|m| m.subject == "Ancien message"), "le message est revenu dans la boîte : {list:?}");
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
