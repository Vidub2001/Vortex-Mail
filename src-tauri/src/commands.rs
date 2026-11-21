//! Commandes appelées depuis le frontend via `invoke("nom", { args })`.
//! Tauri convertit les arguments camelCase du JS (`folderId`) en `snake_case`.
//!
//! Deux familles de commandes :
//! - lecture du cache (`list_*`, `get_message` si déjà lu) : immédiate, sans réseau ;
//! - synchronisation (`sync_*`) et actions : réseau, dans un fil dédié
//!   (`spawn_blocking`) pour ne jamais figer l'interface.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use mail_core::{Account, Draft, Folder, FolderRole, MailError, Message, MessageId, MessageSummary, MoveReport};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::attachments;
use crate::cache::Cache;
use crate::config::{self, same_server, AccountConfig};
use crate::router::{
    lock_cache, AccountError, AttachmentInput, DraftContent, FlushReport, PlainReplacement, PrefetchReport, RestoreReport, Router,
    SavedDraft, SearchReport,
};
use crate::sanitize;
use crate::secrets;
use crate::transport;
use crate::AppState;

type CmdResult<T> = Result<T, String>;

fn lock<T>(m: &Mutex<T>) -> CmdResult<MutexGuard<'_, T>> {
    m.lock().map_err(|_| tr!("état interne verrouillé après une erreur").to_string())
}

async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| tr!("tâche interrompue : {e}", e = e))?
}

fn router(state: &State<'_, AppState>) -> Arc<Router> {
    Arc::clone(&state.router)
}

fn cache(state: &State<'_, AppState>) -> Arc<Mutex<Cache>> {
    Arc::clone(&state.cache)
}

fn split_folder(folder_id: &str) -> CmdResult<(&str, &str)> {
    folder_id.split_once(':').ok_or_else(|| tr!("dossier inconnu : {folder}", folder = folder_id))
}

// ------------------------------------------------------------------ lecture du cache

#[tauri::command]
pub(crate) async fn list_accounts(state: State<'_, AppState>) -> CmdResult<Vec<Account>> {
    let _timed = crate::diag::timed("list_accounts");
    let cache = cache(&state);
    blocking(move || lock_cache(&cache).accounts()).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderListing {
    folders: Vec<Folder>,
    errors: Vec<AccountError>,
}

/// Arborescence telle qu'en cache : immédiat, même hors ligne.
#[tauri::command]
pub(crate) async fn list_folders(state: State<'_, AppState>) -> CmdResult<FolderListing> {
    let _timed = crate::diag::timed("list_folders");
    let cache = cache(&state);
    blocking(move || Ok(FolderListing { folders: lock_cache(&cache).all_folders()?, errors: Vec::new() })).await
}

/// Relit l'arborescence sur les serveurs, puis renvoie le cache à jour.
#[tauri::command]
pub(crate) async fn sync_folders(state: State<'_, AppState>) -> CmdResult<FolderListing> {
    let _timed = crate::diag::timed("sync_folders");
    let (router, cache) = (router(&state), cache(&state));
    blocking(move || {
        let errors = router.refresh_folders();
        Ok(FolderListing { folders: lock_cache(&cache).all_folders()?, errors })
    })
    .await
}

/// Messages d'un dossier tels qu'en cache.
#[tauri::command]
pub(crate) async fn list_messages(state: State<'_, AppState>, folder_id: String) -> CmdResult<Vec<MessageSummary>> {
    let _timed = crate::diag::timed("list_messages");
    let cache = cache(&state);
    blocking(move || {
        let (account, path) = split_folder(&folder_id)?;
        lock_cache(&cache).list(account, path)
    })
    .await
}

/// Synchronise un dossier avec son serveur, puis renvoie la liste à jour.
#[tauri::command]
pub(crate) async fn sync_messages(state: State<'_, AppState>, folder_id: String) -> CmdResult<Vec<MessageSummary>> {
    let (router, cache) = (router(&state), cache(&state));
    blocking(move || {
        router.sync_folder(&folder_id).map_err(|e| e.to_string())?;
        let (account, path) = split_folder(&folder_id)?;
        lock_cache(&cache).list(account, path)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderSync {
    folder_id: String,
    /// Liste à jour (en cache si la synchronisation a échoué) ; absente si
    /// elle n'a pas changé depuis la version `known` donnée par l'interface.
    messages: Option<Vec<MessageSummary>>,
    /// Version de la liste, à renvoyer à la synchronisation suivante.
    revision: u64,
    /// `UIDVALIDITY` du dossier (repère des règles, audit P8).
    uid_validity: Option<u32>,
    error: Option<String>,
}

/// Vue unifiée : synchronise plusieurs dossiers d'un coup, les comptes en
/// parallèle. `known` : version de la liste que l'interface a déjà, par
/// dossier ; une liste inchangée n'est ni relue ni envoyée (audit P1 : une
/// boîte de 30 000 messages faisait 10 Mo de JSON à chaque cycle).
#[tauri::command]
pub(crate) async fn sync_messages_many(
    state: State<'_, AppState>,
    folder_ids: Vec<String>,
    known: Option<Vec<Option<u64>>>,
) -> CmdResult<Vec<FolderSync>> {
    let _timed = crate::diag::timed("sync_messages_many");
    let (router, cache) = (router(&state), cache(&state));
    blocking(move || {
        let results = router.sync_many(&folder_ids);
        let known = known.unwrap_or_default();
        let c = &mut lock_cache(&cache);
        folder_ids
            .into_iter()
            .zip(results)
            .enumerate()
            .map(|(i, (folder_id, r))| {
                let (account, path) = split_folder(&folder_id)?;
                let revision = c.revision(account, path);
                let unchanged = known.get(i).copied().flatten() == Some(revision);
                let messages = if unchanged { None } else { Some(c.list(account, path)?) };
                let uid_validity = c.uid_validity(account, path)?;
                Ok(FolderSync { messages, revision, uid_validity, error: r.err().map(|e| e.to_string()), folder_id })
            })
            .collect()
    })
    .await
}

/// Message complet : depuis le cache s'il a déjà été lu, sinon depuis le serveur.
#[tauri::command]
pub(crate) async fn get_message(state: State<'_, AppState>, id: MessageId) -> CmdResult<Message> {
    let _timed = crate::diag::timed("get_message");
    let (router, cache) = (router(&state), cache(&state));
    blocking(move || {
        let cached = lock_cache(&cache).message(id)?;
        let message = match cached {
            Some(m) => m,
            None => router.message(id).map_err(|e| e.to_string())?,
        };
        // Message chiffré (lot D3a) : déchiffré en mémoire, jamais en cache.
        Ok(prepare_body(router.open(id, message)))
    })
    .await
}

/// Corps prêt pour l'interface (lot 4) : l'original, affiché dans le volet de
/// lecture isolé, perd ses redirections automatiques. Le corps nettoyé et le
/// texte ne sont plus calculés ici (audit P7) : en mode Original ils ne
/// servaient qu'à répondre ; l'interface les demande à [`body_views`].
fn prepare_body(mut message: Message) -> Message {
    message.body_html = sanitize::strip_refresh(&message.body_html);
    message
}

/// Corps nettoyé (liste blanche) et texte brut d'un corps HTML (audit P7).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BodyViews {
    /// Corps nettoyé : mode « HTML simplifié », citation, brouillons.
    simple: String,
    /// Texte brut tiré du corps nettoyé : mode « Texte ».
    text: String,
}

fn body_views_of(html: &str) -> BodyViews {
    let simple = sanitize::simplify(html);
    let text = mail_core::compose::html_to_text(&simple);
    BodyViews { simple, text }
}

/// Vues d'un corps rendu par [`get_message`], calculées seulement quand
/// l'affichage ou une réponse en a besoin (audit P7). Le HTML vient de la
/// fenêtre : il repasse par la liste blanche comme tout corps de message, et
/// rien n'est relu ni déchiffré une seconde fois.
#[tauri::command]
pub(crate) async fn body_views(html: String) -> CmdResult<BodyViews> {
    let _timed = crate::diag::timed("body_views");
    if html.len() > MAX_VIEW_HTML {
        return Err(tr!("Corps trop volumineux pour cet affichage : {limit} Mo au plus.", limit = MAX_VIEW_HTML >> 20));
    }
    blocking(move || Ok(body_views_of(&html))).await
}

/// Corps reçu par [`body_views`], au plus : bien au-delà d'un message réel
/// (25 Mo d'images intégrées au plus, voir `images.rs`), même plafond que
/// l'état confié à une fenêtre (`windows::MAX_INIT`).
const MAX_VIEW_HTML: usize = 64 * 1024 * 1024;

#[tauri::command]
pub(crate) async fn set_seen(state: State<'_, AppState>, ids: Vec<MessageId>, seen: bool) -> CmdResult<()> {
    let router = router(&state);
    blocking(move || router.set_flag(&ids, "\\Seen", seen).map_err(|e| e.to_string())).await
}

#[tauri::command]
pub(crate) async fn set_flagged(state: State<'_, AppState>, ids: Vec<MessageId>, flagged: bool) -> CmdResult<()> {
    let router = router(&state);
    blocking(move || router.set_flag(&ids, "\\Flagged", flagged).map_err(|e| e.to_string())).await
}

// ------------------------------------------------------------------ déplacements

/// Glisser-déposer ou menu « Déplacer vers » : la cible peut être sur un autre compte.
#[tauri::command]
pub(crate) async fn move_messages(state: State<'_, AppState>, ids: Vec<MessageId>, target_folder_id: String) -> CmdResult<MoveReport> {
    let (router, journal) = (router(&state), Arc::clone(&state.journal));
    blocking(move || {
        // Journal partagé : verrouillé un instant à chaque étape (audit P4).
        router.move_or_queue(&journal, &ids, Some(&target_folder_id), None).map_err(|e| e.to_string())
    })
    .await
}

/// Supprimer / Archiver : chaque message va dans le dossier du rôle de son propre compte.
#[tauri::command]
pub(crate) async fn move_to_role(state: State<'_, AppState>, ids: Vec<MessageId>, role: FolderRole) -> CmdResult<MoveReport> {
    let (router, journal) = (router(&state), Arc::clone(&state.journal));
    blocking(move || router.move_or_queue(&journal, &ids, None, Some(role)).map_err(|e| e.to_string())).await
}

// ------------------------------------------------------------------ envoi

/// Pièces jointes reprises d'un autre message : seulement celles que la
/// fenêtre qui a ouvert la rédaction lui a confiées (relecture du 10 novembre 2026).
fn check_sources(window: &tauri::WebviewWindow, attachments: &[AttachmentInput]) -> CmdResult<()> {
    let label = window.label();
    if attachments.iter().filter_map(|a| a.message_id).all(|id| crate::windows::may_reuse(label, id)) {
        Ok(())
    } else {
        Err(tr!("Pièce jointe d'un autre message refusée.").into())
    }
}

/// Message confié à l'envoi en arrière-plan (lot E5). S'il n'a pas été rendu par
/// [`SendJob::take`] quand le garde est détruit, c'est que l'envoi a paniqué : le
/// message revient à la fenêtre principale comme un échec, au lieu de laisser
/// « Envoi en cours… » sans suite.
///
/// `report` annonce l'échec (à la fenêtre principale ; à un test, lot E5b).
struct SendJob<R: FnMut(crate::windows::SendFailed)> {
    report: R,
    message: Option<crate::windows::SendFailed>,
}

impl<R: FnMut(crate::windows::SendFailed)> SendJob<R> {
    /// Le message, rendu à l'appelant : le garde n'annoncera plus rien.
    fn take(&mut self) -> Option<crate::windows::SendFailed> {
        self.message.take()
    }
}

impl<R: FnMut(crate::windows::SendFailed)> Drop for SendJob<R> {
    fn drop(&mut self) {
        if let Some(mut message) = self.message.take() {
            tr!("Erreur interne pendant l'envoi : vérifiez « Éléments envoyés » avant de renvoyer.").clone_into(&mut message.error);
            (self.report)(message);
        }
    }
}

/// Échec d'un envoi en arrière-plan : notification du bureau (la fenêtre
/// principale est peut-être cachée dans la zone de notification), puis le
/// message rendu à la fenêtre principale, qui rouvre la rédaction.
fn send_failed(app: &tauri::AppHandle, message: crate::windows::SendFailed) {
    crate::idle::desktop_notify(app, tr!("Message non envoyé"), &message.error, "dialog-error", "email");
    crate::windows::send_failed(app, message);
}

/// Envoie un message en arrière-plan (lot E1) : rend la main dès que le message
/// est vérifié, pour que la fenêtre de rédaction se ferme aussitôt. Le résultat
/// arrive à la fenêtre principale : envoyé (`sent`), ou échoué (`sendFailed`,
/// avec le message, qu'elle rouvre). La copie de secours de la fenêtre n'est
/// retirée qu'une fois le message parti.
///
/// `answered_id` : message auquel on répond (marqué « répondu ») ; `draft_id` :
/// brouillon enregistré de ce message (supprimé après l'envoi).
#[tauri::command]
#[expect(clippy::too_many_arguments, reason = "arguments d'une commande Tauri, nommés par l'interface")]
pub(crate) async fn send_message(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    account_id: String,
    draft: Draft,
    answered_id: Option<MessageId>,
    attachments: Option<Vec<AttachmentInput>>,
    draft_id: Option<MessageId>,
) -> CmdResult<()> {
    let router = router(&state);
    let attachments = attachments.unwrap_or_default();
    check_sources(&window, &attachments)?;
    draft.validate()?;
    let (label, cache) = (window.label().to_owned(), Arc::clone(&state.cache));
    std::thread::spawn(move || {
        // Le message reste dans le garde pendant l'envoi, qui ne fait que le lire :
        // aucune copie, et il est rendu à la rédaction même si l'envoi panique.
        let panicked = app.clone();
        let mut job = SendJob {
            report: move |message| send_failed(&panicked, message),
            message: Some(crate::windows::SendFailed {
                error: String::new(),
                account_id,
                draft,
                attachments,
                answered_id,
                draft_id,
                window: label,
            }),
        };
        let Some(pending) = job.message.as_ref() else { return };
        let result = router.send(&pending.account_id, &pending.draft, answered_id, &pending.attachments, draft_id);
        let Some(mut message) = job.take() else { return };
        match result {
            Ok(report) => {
                crate::windows::drop_backup(&cache, &message.window);
                // Destinataires proposés à la complétion (lot J13) ; un échec
                // ici ne change rien à l'envoi, déjà fait.
                if let Err(e) = crate::pim::note_sent(&cache, &message.account_id, &message.draft) {
                    crate::diag::report(&e);
                }
                crate::windows::send_succeeded(&app, &report, message.account_id, answered_id);
                if draft_id.is_some() {
                    crate::windows::drafts_changed(&app);
                }
            }
            Err(e) => {
                message.error = e.to_string();
                send_failed(&app, message);
            }
        }
    });
    Ok(())
}

// ------------------------------------------------------------------ brouillons

/// Enregistre le brouillon dans « Brouillons » (remplace la version `replace`).
#[tauri::command]
pub(crate) async fn save_draft(
    state: State<'_, AppState>,
    window: tauri::WebviewWindow,
    account_id: String,
    draft: Draft,
    attachments: Option<Vec<AttachmentInput>>,
    replace: Option<MessageId>,
    allow_plain: Option<bool>,
) -> CmdResult<SavedDraft> {
    let router = router(&state);
    let attachments = attachments.unwrap_or_default();
    check_sources(&window, &attachments)?;
    // Brouillon chiffré remplacé en clair seulement sur demande de la rédaction (audit S4).
    let plain = if allow_plain == Some(true) { PlainReplacement::Confirmed } else { PlainReplacement::Refused };
    blocking(move || router.save_draft(&account_id, draft, attachments, replace, plain).map_err(|e| e.to_string())).await
}

#[tauri::command]
pub(crate) async fn discard_draft(state: State<'_, AppState>, id: MessageId) -> CmdResult<()> {
    let router = router(&state);
    blocking(move || router.discard_draft(id).map_err(|e| e.to_string())).await
}

/// Contenu d'un brouillon, pour reprendre sa rédaction.
#[tauri::command]
pub(crate) async fn open_draft(state: State<'_, AppState>, id: MessageId) -> CmdResult<DraftContent> {
    let router = router(&state);
    blocking(move || {
        let mut draft = router.draft_content(id).map_err(|e| e.to_string())?;
        // Un brouillon peut venir d'ailleurs (autre logiciel, filtre, glisser-
        // déposer) : nettoyé comme une citation avant d'entrer dans l'éditeur (S4).
        draft.html = sanitize::simplify(&draft.html);
        Ok(draft)
    })
    .await
}

// ------------------------------------------------------------------ recherche

/// Recherche immédiate dans le cache (fonctionne hors connexion).
#[tauri::command]
pub(crate) async fn search_local(state: State<'_, AppState>, query: String) -> CmdResult<Vec<MessageSummary>> {
    let _timed = crate::diag::timed("search_local");
    let cache = cache(&state);
    // Recherche mal écrite (date illisible, critère inconnu) : message clair (lot E7).
    let query = mail_core::search::SearchQuery::parse(&query)?;
    blocking(move || lock_cache(&cache).search(&query, 200)).await
}

/// Recherche sur les serveurs, dans tous les dossiers de tous les comptes.
#[tauri::command]
pub(crate) async fn search_messages(state: State<'_, AppState>, query: String) -> CmdResult<SearchReport> {
    let _timed = crate::diag::timed("search_messages");
    let router = router(&state);
    let query = mail_core::search::SearchQuery::parse(&query)?;
    blocking(move || Ok(router.search(&query))).await
}

// ------------------------------------------------------------------ contacts et agenda

fn account_configs(state: &State<'_, AppState>) -> CmdResult<Vec<AccountConfig>> {
    Ok(lock(&state.configs)?.clone())
}

/// Relit carnets et agendas sur les serveurs (CardDAV/CalDAV).
#[tauri::command]
pub(crate) async fn pim_sync(state: State<'_, AppState>) -> CmdResult<crate::pim::SyncReport> {
    let (configs, cache) = (account_configs(&state)?, cache(&state));
    blocking(move || Ok(crate::pim::sync_all(&cache, &configs))).await
}

#[tauri::command]
pub(crate) async fn pim_collections(state: State<'_, AppState>) -> CmdResult<Vec<crate::cache::PimCollection>> {
    let cache = cache(&state);
    blocking(move || crate::pim::collections(&cache)).await
}

#[tauri::command]
pub(crate) async fn contacts_list(state: State<'_, AppState>) -> CmdResult<Vec<crate::pimfmt::Contact>> {
    let cache = cache(&state);
    blocking(move || crate::pim::contacts(&cache)).await
}

#[tauri::command]
pub(crate) async fn contact_save(
    state: State<'_, AppState>,
    contact: crate::pimfmt::Contact,
    collection: String,
) -> CmdResult<crate::pimfmt::Contact> {
    let (configs, cache) = (account_configs(&state)?, cache(&state));
    blocking(move || crate::pim::save_contact(&cache, &configs, contact, &collection, None)).await
}

#[tauri::command]
pub(crate) async fn events_list(state: State<'_, AppState>) -> CmdResult<Vec<crate::pimfmt::Event>> {
    let cache = cache(&state);
    blocking(move || crate::pim::events(&cache)).await
}

#[tauri::command]
pub(crate) async fn event_save(
    state: State<'_, AppState>,
    event: crate::pimfmt::Event,
    collection: String,
    raw: Option<String>,
) -> CmdResult<crate::pimfmt::Event> {
    let (configs, cache) = (account_configs(&state)?, cache(&state));
    blocking(move || crate::pim::save_event(&cache, &configs, event, &collection, raw.as_deref())).await
}

/// Source d'un contact ou d'un rendez-vous (vCard, iCalendar ou JSON de
/// l'API), telle que le serveur l'a donnée : pour comprendre un rendez-vous
/// mal lu (lot H1b).
#[tauri::command]
pub(crate) async fn pim_source(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    let cache = cache(&state);
    blocking(move || Ok(lock_cache(&cache).pim_get(&id)?.map_or_default(|(_, _, item)| item.raw))).await
}

/// Supprime un contact ou un rendez-vous (toute la série).
#[tauri::command]
pub(crate) async fn pim_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let (configs, cache) = (account_configs(&state)?, cache(&state));
    blocking(move || crate::pim::delete_item(&cache, &configs, &id)).await
}

/// Adresses proposées pendant la saisie d'un destinataire.
#[tauri::command]
pub(crate) async fn address_suggest(state: State<'_, AppState>, query: String) -> CmdResult<Vec<crate::pim::Suggestion>> {
    let _timed = crate::diag::timed("address_suggest");
    let cache = cache(&state);
    blocking(move || crate::pim::suggest(&cache, &query)).await
}

/// Écarte une adresse collectée de la complétion (lot J13) ; un contact du
/// carnet reste proposé.
#[tauri::command]
pub(crate) async fn forget_address(state: State<'_, AppState>, email: String) -> CmdResult<()> {
    let cache = cache(&state);
    blocking(move || crate::pim::forget_address(&cache, &email)).await
}

/// Invitation reçue (pièce jointe `.ics`) : rendez-vous qu'elle contient, et le calendrier brut.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Invitation {
    events: Vec<crate::pimfmt::Event>,
    raw: String,
    /// `METHOD` du calendrier (`REQUEST`, `CANCEL`, `REPLY`…), vide s'il n'y en a pas.
    method: String,
    /// Comptes qui peuvent répondre : celui qui a reçu l'invitation d'abord,
    /// puis les autres comptes invités (lot H1b) ; vide si le compte n'est pas connu.
    addresses: Vec<MyAddress>,
}

/// Compte qui peut répondre à une invitation, et sa réponse déjà donnée.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MyAddress {
    account: String,
    email: String,
    name: String,
    /// Réponse déjà donnée à cette version (lot H1) : `accepted`, `tentative`
    /// ou `declined`.
    answer: Option<String>,
}

/// Comptes qui peuvent répondre à `events` : `received` (le compte qui l'a
/// reçue), puis ceux dont l'adresse figure parmi les invités.
fn answering_accounts(configs: Vec<AccountConfig>, received: Option<&str>, events: &[crate::pimfmt::Event]) -> Vec<AccountConfig> {
    let invited = |c: &AccountConfig| events.iter().any(|e| e.attendees.iter().any(|a| a.email.eq_ignore_ascii_case(&c.email)));
    let (mut first, others): (Vec<_>, Vec<_>) = configs.into_iter().partition(|c| Some(c.id.as_str()) == received);
    first.extend(others.into_iter().filter(invited));
    first
}

#[tauri::command]
pub(crate) async fn read_invitation(
    state: State<'_, AppState>,
    id: MessageId,
    index: u32,
    account_id: Option<String>,
) -> CmdResult<Invitation> {
    let (router, cache) = (router(&state), cache(&state));
    blocking(move || {
        let a = router.attachment(id, index).map_err(|e| e.to_string())?;
        let raw = String::from_utf8_lossy(&a.data).into_owned();
        let events = crate::pimfmt::parse_ical(&raw);
        if events.is_empty() {
            return Err(tr!("Aucun rendez-vous dans ce fichier.").into());
        }
        let method = crate::pimfmt::calendar_method(&raw);
        // Même rendez-vous que la réponse : la série, sinon le premier.
        let main = events.iter().find(|e| e.recurrence_id.is_empty()).or_else(|| events.first());
        let mut addresses = Vec::new();
        if account_id.is_some() {
            for c in answering_accounts(router.configs(), account_id.as_deref(), &events) {
                let answer =
                    main.map(|e| lock_cache(&cache).answer_for(&c.id, &e.uid, e.recurrence_id.trim(), e.sequence)).transpose()?.flatten();
                addresses.push(MyAddress { account: c.id, email: c.email, name: c.display_name, answer });
            }
        }
        Ok(Invitation { events, raw, method, addresses })
    })
    .await
}

/// Réponse à une invitation (lot H1) : l'invitation où la ligne du compte porte
/// la réponse, à garder dans l'agenda.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InvitationAnswer {
    raw: String,
}

/// Réponse choisie dans l'interface, pour [`answer_invitation`] et [`answer_event`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplyChoice {
    /// `accepted`, `tentative` ou `declined`.
    answer: String,
    /// Envoyer la réponse à l'organisateur ; sinon, seul l'agenda change
    /// (invitation reçue par une liste, par exemple).
    send: bool,
    /// Date du rendez-vous mise en forme par l'interface, pour le message.
    when: Option<String>,
    /// Autre compte invité au nom duquel répondre (lot H1b).
    as_account: Option<String>,
}

/// Répond à une invitation reçue sur le compte `account_id`. Avec `send`, la
/// réponse part à l'organisateur (iMIP) et sa copie va dans « Éléments envoyés ».
#[tauri::command]
pub(crate) async fn answer_invitation(
    state: State<'_, AppState>,
    id: MessageId,
    index: u32,
    account_id: String,
    reply: ReplyChoice,
) -> CmdResult<InvitationAnswer> {
    let (router, cache) = (router(&state), cache(&state));
    blocking(move || {
        let ReplyChoice { answer, send, when, as_account } = reply;
        let answer = crate::imip::PartStat::parse(&answer).ok_or_else(|| tr!("Réponse inconnue.").to_string())?;
        let a = router.attachment(id, index).map_err(|e| e.to_string())?;
        let raw = String::from_utf8_lossy(&a.data).into_owned();
        // Le compte qui a reçu l'invitation, ou un autre compte invité.
        let chosen = as_account.unwrap_or_else(|| account_id.clone());
        let config = answering_accounts(router.configs(), Some(&account_id), &crate::pimfmt::parse_ical(&raw))
            .into_iter()
            .find(|c| c.id == chosen)
            .ok_or_else(|| tr!("Ce compte n'est pas invité à ce rendez-vous.").to_string())?;
        let account_id = config.id.clone();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let stamp = crate::pim::utc_stamp(i64::try_from(now).unwrap_or(0));
        let reply = crate::imip::reply(&raw, &config.email, &config.display_name, answer, &stamp)?;
        if send {
            // Compte sans nom affiché : son adresse, pour que la phrase ait un sujet.
            let who = if config.display_name.trim().is_empty() { &config.email } else { &config.display_name };
            // Date mise en forme par l'interface : texte seulement, borné.
            let when: String = when.unwrap_or_default().chars().filter(|c| !c.is_control()).take(200).collect();
            let (subject, html) = crate::imip::message(&reply, answer, who, &when);
            let draft = Draft {
                to: vec![reply.organizer.clone()],
                subject,
                html,
                calendar: Some(mail_core::compose::OutgoingCalendar { method: "REPLY".into(), ics: reply.ics }),
                ..Draft::default()
            };
            router.send(&account_id, &draft, None, &[], None).map_err(|e| e.to_string())?;
        }
        lock_cache(&cache).answer_put(&account_id, &reply.uid, &reply.recurrence, answer.name(), reply.sequence)?;
        Ok(InvitationAnswer { raw: reply.updated })
    })
    .await
}

/// Répond à une invitation déposée dans l'agenda (lot H1b) : `id` est le
/// rendez-vous en cache ; `reply` comme pour [`answer_invitation`].
/// Le serveur (planification `CalDAV`) ou l'API d'Outlook.com prévient
/// l'organisateur ; sinon la réponse part par courrier.
#[tauri::command]
pub(crate) async fn answer_event(state: State<'_, AppState>, id: String, reply: ReplyChoice) -> CmdResult<()> {
    let (router, cache, configs) = (router(&state), cache(&state), account_configs(&state)?);
    blocking(move || {
        let ReplyChoice { answer, send, when, as_account } = reply;
        let answer = crate::imip::PartStat::parse(&answer).ok_or_else(|| tr!("Réponse inconnue.").to_string())?;
        let crate::pim::Answered::ByMail { account, raw } =
            crate::pim::answer_event(&cache, &configs, &id, answer, send, as_account.as_deref())?
        else {
            return Ok(());
        };
        let config = router.config(&account).ok_or_else(|| tr!("compte {account} introuvable", account = account))?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let stamp = crate::pim::utc_stamp(i64::try_from(now).unwrap_or(0));
        let reply = crate::imip::reply(&raw, &config.email, &config.display_name, answer, &stamp)?;
        let who = if config.display_name.trim().is_empty() { &config.email } else { &config.display_name };
        let when: String = when.unwrap_or_default().chars().filter(|c| !c.is_control()).take(200).collect();
        let (subject, html) = crate::imip::message(&reply, answer, who, &when);
        let draft = Draft {
            to: vec![reply.organizer.clone()],
            subject,
            html,
            calendar: Some(mail_core::compose::OutgoingCalendar { method: "REPLY".into(), ics: reply.ics }),
            ..Draft::default()
        };
        router.send(&account, &draft, None, &[], None).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
}

// ------------------------------------------------------------------ actions en attente

/// Rejoue les actions faites hors connexion et reprend les déplacements interrompus.
#[tauri::command]
pub(crate) async fn flush_pending(state: State<'_, AppState>) -> CmdResult<FlushReport> {
    let (router, journal) = (router(&state), Arc::clone(&state.journal));
    blocking(move || Ok(router.flush_pending(&journal))).await
}

/// Messages concernés par des actions en attente du réseau.
#[tauri::command]
pub(crate) async fn pending_count(state: State<'_, AppState>) -> CmdResult<u32> {
    let cache = cache(&state);
    blocking(move || lock_cache(&cache).pending_count()).await
}

// ------------------------------------------------------------------ pièces jointes

/// Crée `nom.pdf`, sinon `nom (1).pdf`, `nom (2).pdf`… : jamais un fichier
/// existant, ni un lien symbolique, même cassé. Création exclusive
/// (`create_new`) : avant, `exists()` répondait « non » pour un lien cassé que
/// l'écriture suivait ensuite, et le fichier pouvait apparaître entre le test
/// et l'écriture (audit S1).
fn create_unique(dir: &Path, name: &str) -> Result<(PathBuf, std::fs::File), String> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 0..10_000u32 {
        let path = if n == 0 { dir.join(name) } else { dir.join(format!("{stem} ({n}){ext}")) };
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(tr!("écriture de {name} : {e}", name = path.display(), e = e)),
        }
    }
    let e = std::io::Error::from(std::io::ErrorKind::AlreadyExists);
    Err(tr!("écriture de {name} : {e}", name = dir.join(name).display(), e = e))
}

/// Écrit `data` dans un nouveau fichier de `dir` (voir [`create_unique`]) ; rend son chemin.
fn write_new(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf, String> {
    let (path, mut file) = create_unique(dir, name)?;
    std::io::Write::write_all(&mut file, data).map_err(|e| tr!("écriture de {name} : {e}", name = path.display(), e = e))?;
    Ok(path)
}

// ------------------------------------------------------------------ import et export (lot E8a)

/// Taille maximale d'un fichier importé (après décodage base64).
const MAX_IMPORT: usize = 200 * 1024 * 1024;

/// Enregistre les messages `ids` dans Téléchargements, un fichier `.eml` par
/// message, nommé d'après son objet ; renvoie les chemins écrits.
#[tauri::command]
pub(crate) async fn export_eml(state: State<'_, AppState>, ids: Vec<MessageId>) -> CmdResult<Vec<String>> {
    let router = router(&state);
    let dir = state.downloads_dir.clone();
    blocking(move || {
        std::fs::create_dir_all(&dir).map_err(|e| tr!("dossier {dir} : {e}", dir = dir.display(), e = e))?;
        let mut paths = Vec::new();
        for id in ids {
            let raw = router.fetch_raw(id).map_err(|e| e.to_string())?;
            let subject = mail_core::compose::header_value(&raw.bytes, "Subject").unwrap_or_default();
            let subject = if subject.trim().is_empty() { tr!("message").to_string() } else { subject };
            let path = write_new(&dir, &attachments::safe_file_name(&format!("{}.eml", subject.trim())), &raw.bytes)?;
            // Message venu d'Internet, comme une pièce jointe enregistrée (audit S3).
            attachments::mark_from_internet(&path);
            paths.push(path.display().to_string());
        }
        Ok(paths)
    })
    .await
}

/// Dossier exporté : chemin du fichier `.mbox` et nombre de messages.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderExport {
    path: String,
    count: u32,
}

/// Enregistre tout un dossier dans Téléchargements, au format mbox. Un export
/// interrompu ne laisse pas de fichier incomplet.
#[tauri::command]
pub(crate) async fn export_mbox(state: State<'_, AppState>, folder_id: String) -> CmdResult<FolderExport> {
    let router = router(&state);
    let dir = state.downloads_dir.clone();
    blocking(move || {
        let name = router.folder(&folder_id).map_or_else(|| tr!("dossier").to_string(), |f| f.name);
        std::fs::create_dir_all(&dir).map_err(|e| tr!("dossier {dir} : {e}", dir = dir.display(), e = e))?;
        let (path, file) = create_unique(&dir, &attachments::safe_file_name(&format!("{name}.mbox")))?;
        let mut out = std::io::BufWriter::new(file);
        let written = router.export_folder(&folder_id, &mut out).and_then(|count| {
            std::io::Write::flush(&mut out).map_err(|e| MailError::Backend(e.to_string()))?;
            Ok(count)
        });
        match written {
            Ok(count) => {
                attachments::mark_from_internet(&path);
                Ok(FolderExport { path: path.display().to_string(), count })
            }
            Err(e) => {
                // Fichier écrit par Vortex Mail à l'instant, jamais un fichier de l'utilisateur.
                drop(out);
                let _ = std::fs::remove_file(&path);
                Err(e.to_string())
            }
        }
    })
    .await
}

/// En-tête qui porte le dossier cible d'un import, encodé comme une adresse
/// (`encodeURIComponent`) : un nom de dossier n'est pas toujours en ASCII.
const IMPORT_FOLDER_HEADER: &str = "x-vortex-folder";

/// Dossier cible et contenu d'une demande d'import (audit P7) : le fichier
/// arrive tel quel, en octets, sans passer par du base64 ni du JSON.
fn import_request(body: &tauri::ipc::InvokeBody, folder: Option<&str>) -> CmdResult<(String, Vec<u8>)> {
    let Some(folder) = folder.map(crate::oauth::url_decode).filter(|f| !f.is_empty()) else {
        return Err(tr!("dossier d'import absent").into());
    };
    let tauri::ipc::InvokeBody::Raw(data) = body else {
        return Err(tr!("fichier d'import attendu en octets").into());
    };
    if data.len() > MAX_IMPORT {
        return Err(tr!("Fichier trop volumineux : {limit} Mo au plus.", limit = MAX_IMPORT / (1024 * 1024)));
    }
    Ok((folder, data.clone()))
}

/// Dépose dans un dossier les messages d'un fichier `.eml` ou `.mbox`, lu par
/// la fenêtre et transmis en octets (audit P7 : plus de base64, qui gardait le
/// fichier trois à quatre fois en mémoire). Le dossier est dans l'en-tête
/// [`IMPORT_FOLDER_HEADER`].
#[tauri::command]
pub(crate) async fn import_messages(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> CmdResult<crate::router::ImportReport> {
    let folder = request.headers().get(IMPORT_FOLDER_HEADER).and_then(|v| v.to_str().ok());
    let (folder_id, data) = import_request(request.body(), folder)?;
    let router = router(&state);
    blocking(move || router.import_messages(&folder_id, &data).map_err(|e| e.to_string())).await
}

/// Taille maximale d'un agenda ou d'un carnet importé.
const MAX_PIM_IMPORT: usize = 20 * 1024 * 1024;

/// Enregistre un agenda (`.ics`) ou un carnet (`.vcf`) dans Téléchargements (lot E8b).
#[tauri::command]
pub(crate) async fn export_pim(state: State<'_, AppState>, collection: String) -> CmdResult<FolderExport> {
    let cache = cache(&state);
    let dir = state.downloads_dir.clone();
    blocking(move || {
        let found = lock_cache(&cache).pim_collections()?.into_iter().find(|c| c.url == collection);
        let Some(info) = found else { return Err(tr!("carnet ou agenda introuvable : resynchronisez").into()) };
        let (text, count) = crate::pim::export_collection(&cache, &collection)?;
        let extension = if info.kind == "contacts" { "vcf" } else { "ics" };
        std::fs::create_dir_all(&dir).map_err(|e| tr!("dossier {dir} : {e}", dir = dir.display(), e = e))?;
        let path = write_new(&dir, &attachments::safe_file_name(&format!("{}.{extension}", info.name)), text.as_bytes())?;
        Ok(FolderExport { path: path.display().to_string(), count })
    })
    .await
}

/// Importe un fichier `.ics` dans un agenda, ou `.vcf` dans un carnet, selon la collection (lot E8b).
#[tauri::command]
pub(crate) async fn import_pim(state: State<'_, AppState>, collection: String, content: String) -> CmdResult<crate::router::ImportReport> {
    let (cache, configs) = (cache(&state), account_configs(&state)?);
    blocking(move || {
        if content.len() > MAX_PIM_IMPORT {
            return Err(tr!("Fichier trop volumineux : {limit} Mo au plus.", limit = MAX_PIM_IMPORT / (1024 * 1024)));
        }
        let kind = lock_cache(&cache).pim_collections()?.into_iter().find(|c| c.url == collection).map(|c| c.kind);
        match kind.as_deref() {
            Some("contacts") => crate::pim::import_contacts(&cache, &configs, &collection, &content),
            Some(_) => crate::pim::import_calendar(&cache, &configs, &collection, &content),
            None => Err(tr!("carnet ou agenda introuvable : resynchronisez").into()),
        }
    })
    .await
}

/// Connexions Google / Outlook.com à refaire (bandeau « Reconnecter »).
#[tauri::command]
pub(crate) async fn auth_pending() -> CmdResult<Vec<crate::auth::AuthRequired>> {
    Ok(crate::auth::pending())
}

// ------------------------------------------------------------------ mode portable

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PortableStatus {
    /// Données à côté de l'exécutable (fichier `portable.txt`).
    pub portable: bool,
    /// Coffre des mots de passe déjà créé (sinon : premier lancement).
    pub has_vault: bool,
    pub unlocked: bool,
}

#[tauri::command]
pub(crate) async fn portable_status() -> CmdResult<PortableStatus> {
    Ok(PortableStatus { portable: crate::portable::is_portable(), has_vault: secrets::vault_exists(), unlocked: secrets::vault_unlocked() })
}

/// Mode portable : ouvre le coffre (ou le crée), puis charge et démarre les comptes.
#[tauri::command]
pub(crate) async fn portable_unlock(state: State<'_, AppState>, password: String) -> CmdResult<()> {
    if !crate::portable::is_portable() {
        return Ok(());
    }
    let (router, file, watchers) = (router(&state), state.config_file.clone(), Arc::clone(&state.watchers));
    let (configs, background) = (Arc::clone(&state.configs), Arc::clone(&state.background));
    let (cache, journal) = (Arc::clone(&state.cache), Arc::clone(&state.journal));
    blocking(move || {
        // Une seule fois : une seconde demande pendant la première (fenêtre
        // rechargée) trouve le coffre déjà ouvert et ne recharge pas les comptes (C5).
        if !secrets::unlock_vault(&password)? {
            return Ok(());
        }
        // Cache chiffré par une clé du coffre (lot G4b), ouvert avant les comptes :
        // il remplace le cache en mémoire du démarrage, puis le journal des
        // déplacements interrompus est relu dans le vrai cache.
        if let Some(dir) = crate::portable::data_dir() {
            match crate::cache::open_profile(&dir.join("cache.sqlite")) {
                Ok(opened) => *crate::router::lock_cache(&cache) = opened,
                Err(e) => crate::diag::report(&format_args!("{e} ; cache en mémoire pour cette session")),
            }
            let reloaded = crate::journal::load(Arc::clone(&cache));
            *lock(&journal)? = reloaded;
        }
        // Comptes enregistrés : leurs mots de passe viennent maintenant du coffre.
        for config in secrets::load_accounts(&file) {
            router.add_account(config.clone());
            background.add_account(config.clone());
            lock(&watchers)?.start(config);
        }
        *lock(&configs)? = router.configs();
        Ok(())
    })
    .await
}

/// Mode portable : change le mot de passe maître.
#[tauri::command]
pub(crate) async fn portable_change_password(old: String, new: String) -> CmdResult<()> {
    blocking(move || secrets::change_vault_password(&old, &new)).await
}

/// Langue de l'interface, pour les textes produits par l'application (notifications).
#[tauri::command]
pub(crate) async fn set_language(app: tauri::AppHandle, language: String) -> CmdResult<()> {
    crate::i18n::set_language(&language);
    crate::tray::relabel(&app);
    Ok(())
}

/// Rappel de rendez-vous : notification du bureau.
#[tauri::command]
pub(crate) async fn show_reminder(app: tauri::AppHandle, summary: String, body: String) -> CmdResult<()> {
    blocking(move || {
        crate::idle::desktop_notify(&app, &summary, &body, "appointment-soon", "x-vortex.reminder");
        Ok(())
    })
    .await
}

/// Nouveaux messages restés dans une boîte de réception après les règles :
/// notification du bureau (lot B4 ; l'interface l'envoie après le passage des
/// règles, pour ne pas annoncer un message qu'elles mettent à la corbeille).
/// Une notification par compte, qui additionne les messages tant qu'elle
/// reste affichée (lots M1 et P) : `added` messages de plus, `summary` et
/// `body` pour un message seul, `account_name` pour plusieurs.
#[tauri::command]
pub(crate) async fn notify_new_mail(
    app: tauri::AppHandle,
    account: String,
    added: u32,
    summary: String,
    body: String,
    account_name: String,
) -> CmdResult<()> {
    blocking(move || {
        let arrivals = crate::idle::Arrivals { account: &account, added, summary: &summary, body: &body, account_name: &account_name };
        crate::idle::notify_arrivals(&app, &arrivals);
        Ok(())
    })
    .await
}

/// Réglage « garder Vortex Mail dans la zone de notification » (lot B3).
#[tauri::command]
pub(crate) fn set_keep_in_tray(keep: bool) {
    crate::tray::set_keep(keep);
}

/// Ouvre une adresse web dans le navigateur par défaut (liens de « À propos »).
#[tauri::command]
pub(crate) async fn open_url(url: String) -> CmdResult<()> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err(tr!("adresse non prise en charge").into());
    }
    crate::oauth::open_browser(&url)
}

/// Nouvelle version publiée sur GitHub (lot K1) ; `None` : déjà à jour.
#[tauri::command]
pub(crate) async fn check_update() -> CmdResult<Option<crate::update::NewVersion>> {
    blocking(|| crate::update::check(env!("CARGO_PKG_VERSION"))).await
}

/// Pièce jointe ouverte avec l'application par défaut, si son type fait partie
/// des types sûrs et que son contenu correspond à son extension (lot 2) ;
/// sinon, l'utilisateur est invité à la télécharger.
#[tauri::command]
pub(crate) async fn open_attachment(state: State<'_, AppState>, id: MessageId, index: u32) -> CmdResult<String> {
    let router = router(&state);
    let dir = state.files_dir.join(format!("{id}-{index}"));
    blocking(move || {
        let a = router.attachment(id, index).map_err(|e| e.to_string())?;
        let name = attachments::safe_file_name(&a.name);
        attachments::check_open(&name, &a.data)?;
        let path = attachments::write_temp(&dir, &name, &a.data)?;
        attachments::open_with_system(path.as_os_str())?;
        Ok(path.display().to_string())
    })
    .await
}

/// Enregistre la pièce jointe dans le dossier Téléchargements ; renvoie le chemin choisi.
#[tauri::command]
pub(crate) async fn save_attachment(state: State<'_, AppState>, id: MessageId, index: u32) -> CmdResult<String> {
    let router = router(&state);
    let dir = state.downloads_dir.clone();
    blocking(move || {
        let a = router.attachment(id, index).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| tr!("dossier {dir} : {e}", dir = dir.display(), e = e))?;
        let path = write_new(&dir, &attachments::safe_file_name(&a.name), &a.data)?;
        attachments::mark_from_internet(&path);
        Ok(path.display().to_string())
    })
    .await
}

// ------------------------------------------------------------------ comptes

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckResult {
    ok: bool,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TestReport {
    imap: CheckResult,
    smtp: CheckResult,
}

fn check_imap(cfg: &AccountConfig) -> CheckResult {
    match transport::open_imap(&cfg.imap) {
        Ok((mut session, security)) => {
            let folders = session.list().map_or(0, |l| l.len());
            let _ = session.logout();
            CheckResult {
                ok: true,
                message: if folders > 1 {
                    tr!("Connecté ({security}), {n} dossiers", security = transport::security_label(security), n = folders)
                } else {
                    tr!("Connecté ({security}), {n} dossier", security = transport::security_label(security), n = folders)
                },
            }
        }
        Err(message) => CheckResult { ok: false, message },
    }
}

fn check_smtp(cfg: &AccountConfig) -> CheckResult {
    let smtp = cfg.effective_smtp();
    match transport::check_smtp(&smtp) {
        Ok(security) => CheckResult { ok: true, message: tr!("Authentifié ({security})", security = transport::security_label(security)) },
        // Envoi SMTP coupé par Outlook.com : l'envoi passe par l'API, s'il est autorisé.
        Err(e) if transport::smtp_disabled_by_service(&smtp, &e) => match crate::oauth::send_token(smtp.username.trim()) {
            Ok(_) => CheckResult {
                ok: true,
                message: tr!("Envoi SMTP désactivé par Outlook.com pour cette boîte : les messages partent par l'API d'Outlook.com.")
                    .into(),
            },
            Err(message) => CheckResult { ok: false, message },
        },
        Err(message) => CheckResult { ok: false, message },
    }
}

/// En modification, un mot de passe laissé vide signifie « inchangé ».
fn keep_passwords(configs: &Mutex<Vec<AccountConfig>>, config: &mut AccountConfig) -> CmdResult<()> {
    if config.id.is_empty() {
        return Ok(());
    }
    let existing = lock(configs)?.iter().find(|c| c.id == config.id).cloned();
    match existing {
        Some(existing) => reuse_passwords(&existing, config),
        None => Ok(()),
    }
}

/// Reprend les mots de passe enregistrés laissés vides, seulement si le serveur
/// qui les recevra est inchangé (hôte, port, identifiant). Sinon, un formulaire
/// modifié (ou une fenêtre compromise) pourrait les envoyer à un autre serveur.
fn reuse_passwords(existing: &AccountConfig, config: &mut AccountConfig) -> CmdResult<()> {
    let changed = |proto: &str| tr!("Le serveur ou l'identifiant {proto} a changé : saisissez à nouveau le mot de passe.", proto = proto);
    if config.imap.password.is_empty() && !existing.imap.password.is_empty() {
        // Avec « mêmes identifiants », le mot de passe IMAP part aussi au serveur SMTP.
        let smtp_ok = !config.smtp_same_credentials || same_server(&existing.effective_smtp(), &config.effective_smtp());
        if !same_server(&existing.imap, &config.imap) {
            return Err(changed("IMAP"));
        }
        if !smtp_ok {
            return Err(changed("SMTP"));
        }
        config.imap.password.clone_from(&existing.imap.password);
    }
    if !config.smtp_same_credentials && config.smtp.password.is_empty() && !existing.smtp.password.is_empty() {
        if !same_server(&existing.smtp, &config.smtp) {
            return Err(changed("SMTP"));
        }
        config.smtp.password.clone_from(&existing.smtp.password);
    }
    Ok(())
}

/// Réglages d'un compte trouvés d'après son adresse (lot I1), sans mot de passe.
#[tauri::command]
pub(crate) async fn detect_account(email: String) -> CmdResult<crate::autoconfig::Detected> {
    blocking(move || crate::autoconfig::detect(&email)).await
}

/// Empreinte du certificat de Proton Mail Bridge sur cet ordinateur (lot I3), à
/// montrer à l'utilisateur avant de l'épingler dans le compte.
#[tauri::command]
pub(crate) async fn bridge_certificate(port: u16, security: config::Security) -> CmdResult<String> {
    blocking(move || transport::bridge_fingerprint(port, security)).await
}

/// Bouton « Tester les paramètres du compte ».
#[tauri::command]
pub(crate) async fn test_account(state: State<'_, AppState>, mut config: AccountConfig) -> CmdResult<TestReport> {
    let configs = Arc::clone(&state.configs);
    blocking(move || {
        keep_passwords(&configs, &mut config)?;
        config.validate()?;
        Ok(TestReport { imap: check_imap(&config), smtp: check_smtp(&config) })
    })
    .await
}

/// Enregistre les modifications d'un compte existant, après vérification IMAP.
#[tauri::command]
pub(crate) async fn update_account(state: State<'_, AppState>, mut config: AccountConfig) -> CmdResult<Account> {
    let (router, file, watchers) = (router(&state), state.config_file.clone(), Arc::clone(&state.watchers));
    let (configs, background) = (Arc::clone(&state.configs), Arc::clone(&state.background));
    blocking(move || {
        keep_passwords(&configs, &mut config)?;
        config.validate()?;
        let imap = check_imap(&config);
        if !imap.ok {
            return Err(imap.message);
        }
        let r = &router;
        if r.config(&config.id).is_none() {
            return Err(tr!("Compte introuvable").into());
        }
        let all: Vec<AccountConfig> = r.configs().into_iter().map(|c| if c.id == config.id { config.clone() } else { c }).collect();
        secrets::store_account(&config)?;
        config::save(&file, &all)?;
        r.update_account(config.clone());
        *lock(&configs)? = r.configs();
        background.update_account(config.clone());
        lock(&watchers)?.start(config.clone());
        Ok(Account { id: config.id, name: config.name, email: config.email, color: config.color })
    })
    .await
}

/// Ajoute un compte après avoir vérifié la connexion IMAP.
#[tauri::command]
pub(crate) async fn add_account(state: State<'_, AppState>, mut config: AccountConfig) -> CmdResult<Account> {
    config.validate()?;
    if config.id.trim().is_empty() {
        let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        config.id = format!("imap{millis}");
    }
    let (router, file, watchers) = (router(&state), state.config_file.clone(), Arc::clone(&state.watchers));
    let (configs, background) = (Arc::clone(&state.configs), Arc::clone(&state.background));
    blocking(move || {
        let imap = check_imap(&config);
        if !imap.ok {
            return Err(imap.message);
        }
        secrets::store_account(&config)?;
        let r = &router;
        let mut all = r.configs();
        all.push(config.clone());
        config::save(&file, &all)?;
        r.add_account(config.clone());
        *lock(&configs)? = r.configs();
        background.add_account(config.clone());
        lock(&watchers)?.start(config.clone());
        Ok(Account { id: config.id, name: config.name, email: config.email, color: config.color })
    })
    .await
}

// ------------------------------------------------------------------ hors connexion et images

/// Une étape du téléchargement pour la lecture hors connexion (voir `Router::prefetch_step`).
#[tauri::command]
pub(crate) async fn prefetch_step(state: State<'_, AppState>, resync: bool) -> CmdResult<PrefetchReport> {
    // Routeur d'arrière-plan, avec ses propres connexions : les actions de
    // l'utilisateur n'attendent jamais la fin d'un téléchargement.
    let background = Arc::clone(&state.background);
    blocking(move || background.prefetch_step(resync).map_err(|e| e.to_string())).await
}

/// « Afficher les images » : télécharge les images distantes, les intègre au
/// message en cache et s'en souvient (affichage direct, même hors connexion).
#[tauri::command]
pub(crate) async fn show_images(state: State<'_, AppState>, id: MessageId) -> CmdResult<Message> {
    let cache = cache(&state);
    blocking(move || {
        let message = lock_cache(&cache).message(id)?.ok_or(tr!("Message absent du cache : rouvrez-le."))?;
        // Message chiffré (lot D3a) : ses images, intégrées au corps en cache, y
        // laisseraient son contenu en clair.
        if message.signature == crate::cache::ENCRYPTED {
            return Err(tr!("Les images distantes d'un message chiffré ne sont pas chargées.").into());
        }
        // Téléchargement sans verrou : l'interface reste disponible.
        let (html, _) = crate::images::inline_remote(&message.body_html);
        let mut c = lock_cache(&cache);
        c.allow_images(id, &html)?;
        let message = c.message(id)?.ok_or_else(|| tr!("Message absent du cache").to_string())?;
        Ok(prepare_body(message))
    })
    .await
}

/// Signature S/MIME examinée de nouveau (lot D1) : « Vérifier de nouveau »,
/// ou premier examen d'un message gardé avant le lot D1.
#[tauri::command]
pub(crate) async fn check_signature(state: State<'_, AppState>, id: MessageId) -> CmdResult<Message> {
    let (router, cache) = (router(&state), cache(&state));
    blocking(move || {
        router.check_signature(id).map_err(|e| e.to_string())?;
        let message = lock_cache(&cache).message(id)?.ok_or_else(|| tr!("Message absent du cache").to_string())?;
        Ok(prepare_body(router.open(id, message)))
    })
    .await
}

// ------------------------------------------------------------------ dossiers

/// Crée un dossier ou un sous-dossier, puis renvoie l'arborescence à jour.
#[tauri::command]
pub(crate) async fn create_folder(
    state: State<'_, AppState>,
    account_id: String,
    parent_id: Option<String>,
    name: String,
) -> CmdResult<FolderListing> {
    let (router, cache) = (router(&state), cache(&state));
    blocking(move || {
        let parent = match parent_id.as_deref() {
            Some(id) => Some(split_folder(id)?.1.to_string()),
            None => None,
        };
        let r = &router;
        r.create_folder(&account_id, parent.as_deref(), &name).map_err(|e| e.to_string())?;
        let errors = r.refresh_folders();
        Ok(FolderListing { folders: lock_cache(&cache).all_folders()?, errors })
    })
    .await
}

#[tauri::command]
pub(crate) async fn delete_folder(state: State<'_, AppState>, folder_id: String) -> CmdResult<FolderListing> {
    let (router, cache) = (router(&state), cache(&state));
    blocking(move || {
        let r = &router;
        r.delete_folder(&folder_id).map_err(|e| e.to_string())?;
        let errors = r.refresh_folders();
        Ok(FolderListing { folders: lock_cache(&cache).all_folders()?, errors })
    })
    .await
}

/// Renomme un dossier personnel, puis renvoie l'arborescence à jour.
#[tauri::command]
pub(crate) async fn rename_folder(state: State<'_, AppState>, folder_id: String, name: String) -> CmdResult<FolderListing> {
    let (router, cache) = (router(&state), cache(&state));
    blocking(move || {
        let r = &router;
        r.rename_folder(&folder_id, &name).map_err(|e| e.to_string())?;
        let errors = r.refresh_folders();
        Ok(FolderListing { folders: lock_cache(&cache).all_folders()?, errors })
    })
    .await
}

/// Déplace un dossier personnel dans un autre (`parent_id`) ou au premier niveau (`None`).
#[tauri::command]
pub(crate) async fn move_folder(state: State<'_, AppState>, folder_id: String, parent_id: Option<String>) -> CmdResult<FolderListing> {
    let (router, cache) = (router(&state), cache(&state));
    blocking(move || {
        let parent = match parent_id.as_deref() {
            Some(id) => {
                let (account, path) = split_folder(id)?;
                if !folder_id.starts_with(&format!("{account}:")) {
                    return Err(tr!("Un dossier ne peut être déplacé que dans son propre compte.").into());
                }
                Some(path.to_string())
            }
            None => None,
        };
        let r = &router;
        r.move_folder(&folder_id, parent.as_deref()).map_err(|e| e.to_string())?;
        let errors = r.refresh_folders();
        Ok(FolderListing { folders: lock_cache(&cache).all_folders()?, errors })
    })
    .await
}

/// Vide la corbeille ou le courrier indésirable ; renvoie le nombre de messages supprimés.
#[tauri::command]
pub(crate) async fn empty_folder(state: State<'_, AppState>, folder_id: String) -> CmdResult<u32> {
    let router = router(&state);
    blocking(move || router.empty_folder(&folder_id).map_err(|e| e.to_string())).await
}

/// Remet des messages d'« Éléments supprimés » dans leur dossier d'origine,
/// ou dans la boîte de réception de leur compte quand il n'est pas connu.
#[tauri::command]
pub(crate) async fn restore_messages(state: State<'_, AppState>, ids: Vec<MessageId>) -> CmdResult<RestoreReport> {
    let (router, journal) = (router(&state), Arc::clone(&state.journal));
    blocking(move || router.restore(&journal, &ids).map_err(|e| e.to_string())).await
}

/// Supprime définitivement des messages de la corbeille ou du courrier indésirable.
#[tauri::command]
pub(crate) async fn delete_forever(state: State<'_, AppState>, ids: Vec<MessageId>) -> CmdResult<u32> {
    let router = router(&state);
    blocking(move || router.delete_forever(&ids).map_err(|e| e.to_string())).await
}

/// Ouvre la connexion Outlook.com dans le navigateur ; attend le retour (5 min au plus).
#[tauri::command]
pub(crate) async fn oauth_sign_in(email: String, client_id: String) -> CmdResult<crate::oauth::SignInReport> {
    blocking(move || {
        let report = crate::oauth::sign_in(&client_id, &email)?;
        crate::auth::signed_in("o365", &email);
        Ok(report)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OAuthStatus {
    /// Jeton de renouvellement présent dans le trousseau.
    signed_in: bool,
    /// Un ID d'application est déjà enregistré (inutile de le ressaisir).
    has_client_id: bool,
}

#[tauri::command]
pub(crate) async fn oauth_status(email: String) -> CmdResult<OAuthStatus> {
    blocking(move || {
        Ok(OAuthStatus { signed_in: crate::oauth::is_signed_in(&email), has_client_id: crate::oauth::known_client_id(&email).is_some() })
    })
    .await
}

/// Relie l'agenda et les contacts Google d'un compte Gmail (navigateur, 5 min au plus).
/// ID et secret vides : l'application Google déjà rangée dans le trousseau.
#[tauri::command]
pub(crate) async fn google_sign_in(email: String, client_id: String, client_secret: String, mail: bool) -> CmdResult<()> {
    blocking(move || {
        crate::google::sign_in(&email, &client_id, &client_secret, mail)?;
        crate::auth::signed_in("google", &email);
        Ok(())
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleStatus {
    signed_in: bool,
    /// ID client et secret déjà enregistrés (jamais renvoyés).
    has_client: bool,
}

#[tauri::command]
pub(crate) async fn google_status(email: String) -> CmdResult<GoogleStatus> {
    blocking(move || Ok(GoogleStatus { signed_in: crate::google::is_signed_in(&email), has_client: crate::google::has_client() })).await
}

/// Déconnecte l'agenda Google et retire ses données du cache.
#[tauri::command]
pub(crate) async fn google_sign_out(state: State<'_, AppState>, account_id: String, email: String) -> CmdResult<()> {
    let cache = Arc::clone(&state.cache);
    blocking(move || {
        crate::google::forget(&email)?;
        crate::pim::forget_account(&cache, &account_id)
    })
    .await
}

/// Outlook.com : autorise l'agenda et les contacts dans le navigateur.
#[tauri::command]
pub(crate) async fn o365_sign_in(email: String) -> CmdResult<()> {
    blocking(move || {
        crate::o365::sign_in(&email)?;
        // Reconnecté : plus de bandeau « Reconnecter » pour cette adresse (A3).
        crate::auth::signed_in("o365", &email);
        Ok(())
    })
    .await
}

#[tauri::command]
pub(crate) async fn o365_status(email: String) -> CmdResult<bool> {
    blocking(move || Ok(crate::o365::is_signed_in(&email))).await
}

/// Déconnecte l'agenda et les contacts Outlook.com et retire leurs données du cache.
#[tauri::command]
pub(crate) async fn o365_sign_out(state: State<'_, AppState>, account_id: String, email: String) -> CmdResult<()> {
    let cache = Arc::clone(&state.cache);
    blocking(move || {
        crate::o365::forget(&email)?;
        crate::pim::forget_account(&cache, &account_id)
    })
    .await
}

#[tauri::command]
pub(crate) async fn remove_account(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let (router, file, watchers) = (router(&state), state.config_file.clone(), Arc::clone(&state.watchers));
    let (configs, background) = (Arc::clone(&state.configs), Arc::clone(&state.background));
    blocking(move || {
        let r = &router;
        if let Some(c) = r.config(&id).filter(|c| c.imap.auth == config::AuthMethod::OAuth2) {
            if let Err(e) = crate::oauth::forget(&c.imap.username) {
                crate::diag::report(&e);
            }
            if let Err(e) = crate::o365::forget(&c.imap.username) {
                crate::diag::report(&e);
            }
        }
        if let Some(c) = r.config(&id).filter(|c| crate::google::is_google_host(&c.imap.host)) {
            if let Err(e) = crate::google::forget(crate::pim::google_user(&c)) {
                crate::diag::report(&e);
            }
        }
        let remaining: Vec<AccountConfig> = r.configs().into_iter().filter(|c| c.id != id).collect();
        config::save(&file, &remaining)?;
        r.remove_account(&id);
        *lock(&configs)? = r.configs();
        background.remove_account(&id);
        lock(&watchers)?.stop(&id);
        // Le compte est retiré même si le trousseau refuse ; on le signale.
        if let Err(e) = secrets::delete_account(&id) {
            crate::diag::report(&e);
        }
        if let Err(e) = crate::smime::forget(&id) {
            crate::diag::report(&e);
        }
        Ok(())
    })
    .await
}

// ------------------------------------------------------------------ certificats S/MIME (lot D2)

/// Taille d'un fichier PKCS #12 encodé en base64, acceptée au plus.
const MAX_PKCS12_BASE64: usize = vortex_smime::MAX_PKCS12_BYTES / 3 * 4 + 4;

/// Certificat de signature d'un compte, s'il en a un.
#[tauri::command]
pub(crate) async fn smime_identity(account_id: String) -> CmdResult<Option<crate::smime::IdentityInfo>> {
    blocking(move || Ok(crate::smime::identity_info(&account_id))).await
}

/// Importe le certificat et la clé d'un compte depuis un fichier PKCS #12
/// (`.p12`, `.pfx`) donné en base64.
#[tauri::command]
pub(crate) async fn smime_import(
    state: State<'_, AppState>,
    account_id: String,
    data: String,
    password: String,
) -> CmdResult<crate::smime::IdentityInfo> {
    let router = router(&state);
    blocking(move || {
        let config = router.config(&account_id).ok_or(tr!("Compte introuvable"))?;
        if data.len() > MAX_PKCS12_BASE64 {
            return Err(tr!("Fichier trop volumineux pour un certificat.").into());
        }
        let bytes = mail_core::compose::base64_decode(&data).map_err(|_| tr!("Fichier illisible, ou mot de passe incorrect."))?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        crate::smime::import(&config, &bytes, &password, i64::try_from(now).unwrap_or(i64::MAX))
    })
    .await
}

/// Retire le certificat et la clé de signature d'un compte.
#[tauri::command]
pub(crate) async fn smime_remove(account_id: String) -> CmdResult<()> {
    blocking(move || crate::smime::forget(&account_id)).await
}

/// « Signer tous les messages » d'un compte.
#[tauri::command]
pub(crate) async fn smime_sign_by_default(account_id: String, on: bool) -> CmdResult<()> {
    blocking(move || crate::smime::set_sign_by_default(&account_id, on)).await
}

/// Comptes qui peuvent signer (fenêtre de rédaction).
#[tauri::command]
pub(crate) async fn smime_signers(state: State<'_, AppState>) -> CmdResult<Vec<crate::smime::SigningAccount>> {
    let router = router(&state);
    blocking(move || Ok(crate::smime::signers(&router.configs()))).await
}

/// Destinataires d'un message à chiffrer : sans certificat (envoi refusé), ou
/// avec un certificat d'autorité inconnue (lots D3b et D4 ; fenêtre de rédaction).
#[tauri::command]
pub(crate) async fn smime_missing(addresses: Vec<String>) -> CmdResult<crate::smime::RecipientCheck> {
    blocking(move || {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Ok(crate::smime::check_recipients(&addresses, i64::try_from(now).unwrap_or(i64::MAX)))
    })
    .await
}

/// Certificats des correspondants, gardés sur leurs messages signés (lot D3b).
#[tauri::command]
pub(crate) async fn smime_correspondents() -> CmdResult<Vec<crate::smime::CorrespondentInfo>> {
    blocking(|| Ok(crate::smime::correspondents())).await
}

/// Oublie le certificat d'un correspondant.
#[tauri::command]
pub(crate) async fn smime_forget_correspondent(address: String) -> CmdResult<()> {
    blocking(move || crate::smime::forget_correspondent(&address)).await
}

// ------------------------------------------------------------------ sauvegarde

/// Exporte la configuration complète dans un fichier chiffré (dossier Téléchargements).
/// `settings` : réglages de l'interface, objet JSON `{ "clé": "valeur" }`.
#[tauri::command]
pub(crate) async fn export_backup(state: State<'_, AppState>, password: String, settings: String) -> CmdResult<ExportResult> {
    let (configs, cache, dir) = (Arc::clone(&state.configs), cache(&state), state.downloads_dir.clone());
    blocking(move || {
        let list = lock(&configs)?.clone();
        let (path, missing) = crate::backup::export(&list, &cache, crate::backup::parse_settings(&settings), &password, &dir)?;
        Ok(ExportResult { path: path.display().to_string(), missing })
    })
    .await
}

/// Sauvegarde écrite : son chemin, et les comptes dont le mot de passe manque.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportResult {
    path: String,
    missing: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupAccount {
    name: String,
    email: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupInfo {
    /// Date de la sauvegarde (secondes depuis 1970).
    created: i64,
    accounts: Vec<BackupAccount>,
    /// Contacts et rendez-vous « Sur cet ordinateur ».
    local_items: u32,
}

/// Contenu d'une sauvegarde, avant de l'importer (vérifie aussi le mot de passe).
#[tauri::command]
pub(crate) async fn inspect_backup(content: String, password: String) -> CmdResult<BackupInfo> {
    blocking(move || {
        let p = crate::backup::read(&content, &password)?;
        Ok(BackupInfo {
            created: p.snapshot.created,
            accounts: p.accounts.iter().map(|c| BackupAccount { name: c.name.clone(), email: c.email.clone() }).collect(),
            local_items: u32::try_from(p.snapshot.local.len()).unwrap_or(u32::MAX),
        })
    })
    .await
}

/// Remplace la configuration par celle de la sauvegarde ; renvoie les réglages
/// de l'interface à remettre en place avant le redémarrage.
#[tauri::command]
pub(crate) async fn import_backup(state: State<'_, AppState>, content: String, password: String) -> CmdResult<String> {
    let (configs, cache, file) = (Arc::clone(&state.configs), cache(&state), state.config_file.clone());
    blocking(move || {
        let preview = crate::backup::read(&content, &password)?;
        let current = lock(&configs)?.clone();
        crate::backup::apply(&preview, &current, &file, &cache)?;
        Ok(crate::backup::settings_json(&preview.snapshot.settings))
    })
    .await
}

/// Redémarre l'application (après un import).
#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri ne passe AppHandle aux commandes que par valeur")]
pub(crate) fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

/// Comptes configurés, sans les mots de passe.
#[tauri::command]
pub(crate) async fn list_account_configs(state: State<'_, AppState>) -> CmdResult<Vec<AccountConfig>> {
    // Copie des réglages : jamais bloquée par une synchronisation en cours.
    Ok(lock(&state.configs)?
        .clone()
        .into_iter()
        .map(|mut c| {
            c.imap.password.clear();
            c.smtp.password.clear();
            // Protégé : l'ID d'application reste dans le trousseau.
            c.oauth_client_id.clear();
            c
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dossier vide, propre au test, dans le dossier temporaire du système.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vortex-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dossier de test");
        dir
    }

    #[test]
    fn p7_import_recu_en_octets() {
        let raw = tauri::ipc::InvokeBody::Raw(b"From: a@example.com\r\n\r\nCorps".to_vec());
        let (folder, data) = import_request(&raw, Some("perso%3AArchives%2F2026%20%C3%A9t%C3%A9")).expect("demande valide");
        assert_eq!(folder, "perso:Archives/2026 été", "nom de dossier décodé");
        assert_eq!(data, b"From: a@example.com\r\n\r\nCorps", "octets reçus tels quels");
        assert!(import_request(&raw, None).is_err(), "dossier absent refusé");
        let json = tauri::ipc::InvokeBody::Json(serde_json::Value::Null);
        assert!(import_request(&json, Some("perso%3AINBOX")).is_err(), "contenu JSON refusé");
        let big = tauri::ipc::InvokeBody::Raw(vec![b'a'; MAX_IMPORT + 1]);
        assert!(import_request(&big, Some("perso%3AINBOX")).is_err(), "fichier trop gros refusé");
    }

    #[test]
    fn p7_vues_du_corps_a_la_demande() {
        let original = r#"<p onclick="x()">Bonjour <b>Anne</b></p><script>alert(1)</script><img src="https://pistage.example/p.gif">"#;
        let views = body_views_of(original);
        assert!(!views.simple.contains("script") && !views.simple.contains("onclick"), "liste blanche appliquée : {}", views.simple);
        assert!(!views.simple.contains("pistage.example"), "image distante retirée : {}", views.simple);
        assert!(views.simple.contains("<b>Anne</b>"), "mise en forme gardée : {}", views.simple);
        assert_eq!(views.text.trim(), "Bonjour Anne", "texte tiré du corps nettoyé");
    }

    #[test]
    fn s1_fichier_existant_jamais_remplace() {
        let dir = scratch("s1-existant");
        std::fs::write(dir.join("facture.pdf"), b"ancien").unwrap();
        let path = write_new(&dir, "facture.pdf", b"nouveau").unwrap();
        assert_eq!(path, dir.join("facture (1).pdf"));
        assert_eq!(std::fs::read(dir.join("facture.pdf")).unwrap(), b"ancien", "l'existant est intact");
        assert_eq!(std::fs::read(&path).unwrap(), b"nouveau");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn s1_lien_symbolique_casse_jamais_suivi() {
        // Un autre programme pose « facture.pdf » -> un fichier qui n'existe pas
        // encore ; l'écriture ne doit jamais le créer.
        let dir = scratch("s1-lien");
        let target = dir.join("cible-hors-du-dossier");
        std::os::unix::fs::symlink(&target, dir.join("facture.pdf")).unwrap();
        let path = write_new(&dir, "facture.pdf", b"contenu").unwrap();
        assert_eq!(path, dir.join("facture (1).pdf"));
        assert!(!target.exists(), "la cible du lien n'a pas été écrite");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn account(imap_host: &str, smtp_host: &str, password: &str, same: bool) -> AccountConfig {
        serde_json::from_value(serde_json::json!({
            "id": "a1", "name": "Test", "email": "moi@example.fr", "color": "#000",
            "smtpSameCredentials": same,
            "imap": { "host": imap_host, "port": 993, "security": "auto", "auth": "normal",
                      "username": "moi", "password": password },
            "smtp": { "host": smtp_host, "port": 465, "security": "auto", "auth": "normal",
                      "username": "moi", "password": if same { "" } else { password } },
        }))
        .expect("compte de test")
    }

    fn pending(error: &str) -> crate::windows::SendFailed {
        crate::windows::SendFailed {
            error: error.into(),
            account_id: "a1".into(),
            draft: Draft::default(),
            attachments: Vec::new(),
            answered_id: None,
            draft_id: None,
            window: "compose-1".into(),
        }
    }

    #[test]
    fn panique_pendant_l_envoi_rend_le_message() {
        // Lot E5b : une panique pendant l'envoi rend le message à la rédaction.
        let reported = std::cell::RefCell::new(Vec::new());
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _job = SendJob { report: |m: crate::windows::SendFailed| reported.borrow_mut().push(m), message: Some(pending("")) };
            std::panic::resume_unwind(Box::new("panne simulée"));
        }));
        assert!(outcome.is_err());
        let reported = reported.into_inner();
        assert_eq!(reported.len(), 1, "échec annoncé une fois");
        assert!(reported[0].error.contains("Éléments envoyés"), "{}", reported[0].error);
        assert_eq!(reported[0].window, "compose-1", "rendu à la bonne fenêtre");

        // Message repris par l'envoi (réussi ou échoué normalement) : le garde se tait.
        let mut silent = Vec::new();
        {
            let mut job = SendJob { report: |m: crate::windows::SendFailed| silent.push(m), message: Some(pending("")) };
            assert!(job.take().is_some());
        }
        assert!(silent.is_empty());
    }

    #[test]
    fn mot_de_passe_repris_si_rien_ne_change() {
        let existing = account("imap.example.fr", "smtp.example.fr", "secret", true);
        let mut edited = account("IMAP.example.fr", "smtp.example.fr", "", true);
        reuse_passwords(&existing, &mut edited).unwrap();
        assert_eq!(edited.imap.password, "secret");
    }

    #[test]
    fn mot_de_passe_refuse_si_l_hote_imap_change() {
        let existing = account("imap.example.fr", "smtp.example.fr", "secret", true);
        let mut edited = account("imap.attaquant.example", "smtp.example.fr", "", true);
        assert!(reuse_passwords(&existing, &mut edited).is_err());
        assert_eq!(edited.imap.password, "");
    }

    #[test]
    fn i3_mot_de_passe_redemande_si_le_certificat_de_bridge_change() {
        let pin = |p: &str, c: &mut AccountConfig| {
            c.imap.pinned_cert = p.into();
            c.smtp.pinned_cert = p.into();
        };
        let mut existing = account("127.0.0.1", "127.0.0.1", "secret", true);
        pin(&crate::transport::fingerprint(b"bridge"), &mut existing);
        let mut same = account("127.0.0.1", "127.0.0.1", "", true);
        pin(&crate::transport::fingerprint(b"bridge").to_lowercase(), &mut same);
        reuse_passwords(&existing, &mut same).unwrap();
        assert_eq!(same.imap.password, "secret");

        let mut other = account("127.0.0.1", "127.0.0.1", "", true);
        pin(&crate::transport::fingerprint(b"autre programme"), &mut other);
        assert!(reuse_passwords(&existing, &mut other).is_err());
        assert_eq!(other.imap.password, "");
    }

    #[test]
    fn mot_de_passe_refuse_si_le_smtp_partage_change() {
        let existing = account("imap.example.fr", "smtp.example.fr", "secret", true);
        let mut edited = account("imap.example.fr", "smtp.attaquant.example", "", true);
        assert!(reuse_passwords(&existing, &mut edited).is_err());
    }

    #[test]
    fn mot_de_passe_smtp_separe() {
        let existing = account("imap.example.fr", "smtp.example.fr", "secret", false);
        let mut same = account("imap.example.fr", "smtp.example.fr", "", false);
        reuse_passwords(&existing, &mut same).unwrap();
        assert_eq!(same.smtp.password, "secret");
        let mut moved = account("imap.example.fr", "smtp.attaquant.example", "", false);
        assert!(reuse_passwords(&existing, &mut moved).is_err());
    }

    #[test]
    fn mot_de_passe_saisi_toujours_accepte() {
        let existing = account("imap.example.fr", "smtp.example.fr", "secret", true);
        let mut edited = account("imap.ailleurs.example", "smtp.ailleurs.example", "nouveau", true);
        reuse_passwords(&existing, &mut edited).unwrap();
        assert_eq!(edited.imap.password, "nouveau");
    }
}
