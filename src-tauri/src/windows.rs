//! Création des fenêtres : en mode portable, le stockage du moteur web est
//! placé dans le dossier `data` (rien dans le profil de l'utilisateur).
//!
//! Les fenêtres secondaires (message, rédaction) ont leur propre stockage,
//! séparé de celui de la fenêtre principale (lot 9c) : une fenêtre secondaire
//! compromise ne peut ni lire ni modifier les règles de tri, les réglages ou
//! les signatures. Ce dont elle a besoin lui est confié à l'ouverture
//! (`window_bootstrap`), et elle ne prévient la fenêtre principale que par le
//! backend (`notify_main`), qui vérifie la forme de l'événement.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::webview::{NewWindowFeatures, NewWindowResponse};
use tauri::{AppHandle, Emitter, Manager, Runtime, State, Url, WebviewUrl, WebviewWindowBuilder};

/// Hôte réservé (`.invalid`, RFC 2606) des liens que le volet de lecture
/// réécrit pour que l'interface les traite : lien trompeur à confirmer,
/// `mailto:` (lot 4b). Ces adresses ne quittent jamais l'application.
pub(crate) const LINK_HOST: &str = "link.vortex.invalid";

/// Étiquette de la fenêtre principale (`tauri.conf.json`).
pub(crate) const MAIN: &str = "main";

/// Messages dont chaque fenêtre de rédaction peut reprendre les pièces
/// jointes (transfert) : fixés par la fenêtre qui l'ouvre, jamais par
/// elle-même (relecture du 10 novembre 2026).
static SOURCES: std::sync::Mutex<Vec<(String, Vec<u64>)>> = std::sync::Mutex::new(Vec::new());

/// La fenêtre `label` peut-elle joindre les pièces jointes du message `id` ?
/// La fenêtre principale, toujours.
pub(crate) fn may_reuse(label: &str, id: u64) -> bool {
    label == MAIN || SOURCES.lock().is_ok_and(|s| s.iter().any(|(l, ids)| l == label && ids.contains(&id)))
}

/// Ce que reçoit une fenêtre secondaire à son ouverture : les réglages de la
/// fenêtre principale (thème, langue, signatures) et, pour la rédaction, le
/// brouillon de départ (lu une seule fois).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WindowBoot {
    settings: String,
    init: Option<String>,
}

static BOOTS: Mutex<Vec<(String, WindowBoot)>> = Mutex::new(Vec::new());

/// Réglages confiés à une fenêtre secondaire : ceux de la fenêtre principale
/// seulement ; une fenêtre de message transmet ceux qu'elle a reçus, jamais
/// les siens (elle pourrait les avoir modifiés).
fn boot_settings(from: &str, given: Option<String>) -> String {
    if from == MAIN {
        return given.unwrap_or_default();
    }
    BOOTS.lock().ok().and_then(|all| all.iter().find(|(l, _)| l == from).map(|(_, b)| b.settings.clone())).unwrap_or_default()
}

fn remember_boot(label: &str, boot: WindowBoot) {
    if let Ok(mut all) = BOOTS.lock() {
        all.retain(|(l, _)| l != label);
        all.push((label.to_owned(), boot));
    }
}

/// Fenêtre fermée : ses réglages, son brouillon de départ et les messages
/// dont elle pouvait reprendre les pièces jointes sont oubliés (audit B8 :
/// ils restaient en mémoire jusqu'à la fin de l'application).
pub(crate) fn forget(label: &str) {
    if let Ok(mut all) = BOOTS.lock() {
        all.retain(|(l, _)| l != label);
    }
    if let Ok(mut all) = SOURCES.lock() {
        all.retain(|(l, _)| l != label);
    }
}

/// Réglages et brouillon de départ de la fenêtre `label` ; le brouillon
/// n'est rendu qu'une fois.
fn take_boot(label: &str) -> WindowBoot {
    let Ok(mut all) = BOOTS.lock() else { return WindowBoot::default() };
    all.iter_mut().find(|(l, _)| l == label).map_or_default(|(_, b)| WindowBoot { settings: b.settings.clone(), init: b.init.take() })
}

/// Taille maximale des réglages et du brouillon confiés à une fenêtre.
const MAX_SETTINGS: usize = 1024 * 1024;
const MAX_INIT: usize = 64 * 1024 * 1024;

/// Stockage du moteur web des fenêtres secondaires, à côté de celui de la
/// fenêtre principale.
fn secondary_data_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    match crate::portable::webview_dir() {
        Some(dir) => Some(dir.with_file_name("webview-fenetres")),
        None => app.path().app_local_data_dir().ok().map(|d| d.join("webview-fenetres")),
    }
}

/// Réglages et brouillon de départ de la fenêtre qui appelle.
#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri ne passe la fenêtre appelante aux commandes que par valeur")]
pub(crate) fn window_bootstrap(caller: tauri::WebviewWindow) -> WindowBoot {
    take_boot(caller.label())
}

/// Ce qu'une fenêtre secondaire signale à la fenêtre principale ; toute autre
/// forme est refusée avant d'arriver jusqu'à elle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum WindowEvent {
    #[serde(rename_all = "camelCase")]
    Sent {
        report: SentReport,
        account_id: String,
        answered_id: Option<u64>,
    },
    Moved {
        ids: Vec<u64>,
    },
    Flags {
        ids: Vec<u64>,
    },
    Draft,
    /// Une fenêtre de rédaction a confié son message au backend, qui l'envoie en
    /// arrière-plan (lot E1) ; le résultat arrive ensuite (`Sent` ou `SendFailed`).
    #[serde(rename_all = "camelCase")]
    Sending {
        account_id: String,
    },
}

/// Compte rendu d'envoi relayé à la fenêtre principale (textes bornés).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SentReport {
    recipients: u32,
    saved_in: Option<String>,
    warning: Option<String>,
}

const MAX_EVENT_TEXT: usize = 500;
const MAX_EVENT_IDS: usize = 10_000;

fn valid_event(event: &WindowEvent) -> bool {
    let short = |s: &Option<String>| s.as_ref().is_none_or(|s| s.chars().count() <= MAX_EVENT_TEXT);
    match event {
        WindowEvent::Sent { report, account_id, .. } => {
            short(&report.saved_in) && short(&report.warning) && account_id.chars().count() <= MAX_EVENT_TEXT
        }
        WindowEvent::Moved { ids } | WindowEvent::Flags { ids } => ids.len() <= MAX_EVENT_IDS,
        WindowEvent::Draft => true,
        WindowEvent::Sending { account_id } => account_id.chars().count() <= MAX_EVENT_TEXT,
    }
}

impl From<&crate::router::SendReport> for SentReport {
    fn from(report: &crate::router::SendReport) -> Self {
        Self { recipients: report.recipients, saved_in: report.saved_in.clone(), warning: report.warning.clone() }
    }
}

/// Envoi fait en arrière-plan qui a échoué (lot E1) : émis par le backend seul,
/// jamais accepté d'une fenêtre (ce n'est pas un `WindowEvent`). La fenêtre
/// principale rouvre la rédaction avec le message et l'erreur.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename = "sendFailed", rename_all = "camelCase")]
pub(crate) struct SendFailed {
    pub(crate) error: String,
    pub(crate) account_id: String,
    pub(crate) draft: mail_core::Draft,
    pub(crate) attachments: Vec<crate::router::AttachmentInput>,
    pub(crate) answered_id: Option<u64>,
    pub(crate) draft_id: Option<u64>,
    /// Fenêtre de rédaction d'où vient le message : sa copie de secours.
    pub(crate) window: String,
}

/// Résultat d'un envoi en arrière-plan, pour la fenêtre principale.
pub(crate) fn send_succeeded(app: &AppHandle, report: &crate::router::SendReport, account_id: String, answered_id: Option<u64>) {
    let event = WindowEvent::Sent { report: report.into(), account_id, answered_id };
    if let Err(e) = app.emit_to(MAIN, WINDOW_EVENT, event) {
        crate::diag::report(&e);
    }
}

/// Échec d'un envoi en arrière-plan, pour la fenêtre principale.
pub(crate) fn send_failed(app: &AppHandle, failed: SendFailed) {
    if let Err(e) = app.emit_to(MAIN, WINDOW_EVENT, failed) {
        crate::diag::report(&e);
    }
}

/// « Brouillons » a changé (brouillon supprimé après un envoi en arrière-plan).
pub(crate) fn drafts_changed(app: &AppHandle) {
    if let Err(e) = app.emit_to(MAIN, WINDOW_EVENT, WindowEvent::Draft) {
        crate::diag::report(&e);
    }
}

/// Événement d'une fenêtre secondaire vers la fenêtre principale. Les fenêtres
/// secondaires ne peuvent pas émettre d'événement elles-mêmes (`capabilities/`).
#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri ne passe AppHandle et la fenêtre appelante que par valeur")]
pub(crate) fn notify_main(app: AppHandle, caller: tauri::WebviewWindow, event: WindowEvent) -> Result<(), String> {
    if caller.label() == MAIN || !valid_event(&event) {
        return Err(tr!("Événement de fenêtre refusé.").into());
    }
    app.emit_to(MAIN, WINDOW_EVENT, event).map_err(|e| e.to_string())
}

/// Événement reçu par la fenêtre principale (`src/windows.ts`).
const WINDOW_EVENT: &str = "vortex://changed";

// ------------------------------------------------------------ copies de secours des brouillons

/// Copies de secours des brouillons (hors connexion, serveur injoignable) : la
/// fenêtre principale les propose à la reprise au démarrage. Depuis le lot G4,
/// dans le cache chiffré, une par fenêtre de rédaction (clé : son libellé) et
/// une par brouillon de la fenêtre principale (clé : `principale:…`).
const MAX_BACKUP: usize = 64 * 1024 * 1024;

/// Préfixe des copies de la fenêtre principale.
const MAIN_PREFIX: &str = "principale:";

/// Fichier des copies des versions d'avant le lot G4, à côté de `accounts.json`.
fn old_backups_file(config_file: &std::path::Path) -> PathBuf {
    config_file.with_file_name("brouillons-secours.json")
}

/// Reprend dans le cache les copies du fichier d'avant le lot G4, puis efface ce
/// fichier (il était en clair). Le fichier reste si le cache refuse une copie.
fn take_over_old_backups(state: &crate::AppState) {
    let path = old_backups_file(&state.config_file);
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let all: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&text).unwrap_or_default();
    let mut cache = crate::router::lock_cache(&state.cache);
    for (key, value) in &all {
        if let Some(data) = value.as_str() {
            if let Err(e) = cache.backup_put(key, data) {
                crate::diag::report(&e);
                return;
            }
        }
    }
    drop(cache);
    if let Err(e) = std::fs::remove_file(&path) {
        crate::diag::report(&tr!("chiffrement du cache local : {e}", e = e));
    }
}

/// Copie de secours d'un brouillon (`None` : la retirer). Une fenêtre de
/// rédaction ne touche qu'à sa propre copie ; la fenêtre principale nomme le
/// brouillon par `key`.
#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri ne passe State et la fenêtre appelante que par valeur")]
pub(crate) fn save_draft_backup(
    state: State<'_, crate::AppState>,
    caller: tauri::WebviewWindow,
    backup: Option<String>,
    key: Option<String>,
) -> Result<(), String> {
    let entry = match (caller.label(), key) {
        (label, _) if label.starts_with("compose-") => label.to_owned(),
        (MAIN, Some(key)) if key.len() <= 128 => format!("{MAIN_PREFIX}{key}"),
        _ => return Err(tr!("Copie de secours refusée.").into()),
    };
    if backup.as_ref().is_some_and(|b| b.len() > MAX_BACKUP) {
        return Err(tr!("Copie de secours refusée.").into());
    }
    let mut cache = crate::router::lock_cache(&state.cache);
    match backup {
        Some(b) => cache.backup_put(&entry, &b),
        None => cache.backup_remove(&entry).map(|_| ()),
    }
}

/// Copies de secours restantes : (clé, copie).
#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri ne passe State aux commandes que par valeur")]
pub(crate) fn draft_backups(state: State<'_, crate::AppState>) -> Result<Vec<(String, String)>, String> {
    take_over_old_backups(&state);
    crate::router::lock_cache(&state.cache).backups()
}

/// Retire la copie de secours de la fenêtre `label` une fois son message
/// envoyé en arrière-plan (lot E1).
pub(crate) fn drop_backup(cache: &std::sync::Arc<Mutex<crate::cache::Cache>>, label: &str) {
    if let Err(e) = crate::router::lock_cache(cache).backup_remove(label) {
        crate::diag::report(&e);
    }
}

/// Réglages gardés dans le cache chiffré (audit S8, lot J12b) : seulement
/// ceux-ci, et seulement pour la fenêtre principale (droits des fenêtres).
pub(crate) const SECURE_PREFS: [&str; 4] = ["vortex.mail", "vortex.rules", "vortex.rules.moved", "vortex.rules.done.v3"];
/// Taille d'un réglage, au plus (une liste de règles fait quelques Ko).
const MAX_PREF: usize = 4 * 1024 * 1024;

/// Travail sur le cache hors du fil principal : une commande synchrone de
/// Tauri y tourne, et le verrou du cache peut être tenu plusieurs secondes
/// par une synchronisation (relecture du lot J12) ; l'interface se figerait.
async fn off_main_thread<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| tr!("tâche interrompue : {e}", e = e))?
}

/// Réglages enregistrés parmi [`SECURE_PREFS`] : (clé, valeur JSON).
#[tauri::command]
pub(crate) async fn prefs_get(state: State<'_, crate::AppState>) -> Result<Vec<(String, String)>, String> {
    let cache = std::sync::Arc::clone(&state.cache);
    off_main_thread(move || crate::router::lock_cache(&cache).prefs(&SECURE_PREFS)).await
}

/// Enregistre un réglage de [`SECURE_PREFS`] (`None` : le retirer). Les
/// écritures arrivent une à une : `src/secure-store.ts` attend chacune.
#[tauri::command]
pub(crate) async fn prefs_set(state: State<'_, crate::AppState>, key: String, value: Option<String>) -> Result<(), String> {
    if !SECURE_PREFS.contains(&key.as_str()) || value.as_ref().is_some_and(|v| v.len() > MAX_PREF) {
        return Err(tr!("Réglage refusé.").into());
    }
    let cache = std::sync::Arc::clone(&state.cache);
    off_main_thread(move || crate::router::lock_cache(&cache).pref_set(&key, value.as_deref())).await
}

/// Retire une copie de secours (reprise ou abandonnée depuis la fenêtre principale).
#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri ne passe State aux commandes que par valeur")]
pub(crate) fn clear_draft_backup(state: State<'_, crate::AppState>, key: String) -> Result<(), String> {
    crate::router::lock_cache(&state.cache).backup_remove(&key).map(|_| ())
}

/// Messages dont la fenêtre de rédaction `label` peut reprendre les pièces
/// jointes (transfert) ; remplace ce qui avait été accordé à ce libellé.
fn grant(label: &str, ids: Vec<u64>) {
    if let Ok(mut all) = SOURCES.lock() {
        all.retain(|(l, _)| l != label);
        all.push((label.to_owned(), ids));
    }
}

/// Événement envoyé à la fenêtre d'où vient un lien réécrit.
const LINK_EVENT: &str = "vortex-link";

/// Demande d'ouverture d'une nouvelle fenêtre par le contenu affiché (lien
/// d'un message, toujours en `target="_blank"`, relecture S6) : jamais de
/// fenêtre dans l'application. Les liens `http` et `https` partent dans le
/// navigateur externe ; les liens réécrits par le volet de lecture
/// reviennent à la fenêtre ; tout le reste est ignoré.
fn on_new_window<R: Runtime>(app: AppHandle<R>, label: String) -> impl Fn(Url, NewWindowFeatures) -> NewWindowResponse<R> + Send + 'static {
    move |url, _| {
        match url.scheme() {
            "http" | "https" if url.host_str() == Some(LINK_HOST) => {
                let _ = app.emit_to(label.as_str(), LINK_EVENT, url.as_str());
            }
            "http" | "https" => {
                if let Err(e) = crate::oauth::open_browser(url.as_str()) {
                    crate::diag::report(&format_args!("lien non ouvert : {e}"));
                }
            }
            _ => {}
        }
        NewWindowResponse::Deny
    }
}

/// Fenêtre principale, d'après `tauri.conf.json` (où elle n'est pas créée d'office).
pub(crate) fn create_main<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let Some(config) = app.config().app.windows.iter().find(|w| w.label == MAIN).cloned() else {
        return Ok(());
    };
    let mut builder = WebviewWindowBuilder::from_config(app, &config)?.on_new_window(on_new_window(app.clone(), config.label.clone()));
    if let Some(dir) = crate::portable::webview_dir() {
        builder = builder.data_directory(dir);
    }
    builder.build()?;
    Ok(())
}

/// Fenêtres secondaires autorisées : le libellé décide des droits de la
/// fenêtre (fichiers `capabilities/`), il doit donc correspondre à la page
/// réellement chargée. `message-<n>` ↔ `view=message&id=<n>`,
/// `compose-…` ↔ `view=compose&…`.
fn allowed_window(label: &str, url: &str) -> bool {
    let simple = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if let Some(id) = label.strip_prefix("message-") {
        return !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && url == format!("index.html?view=message&id={id}");
    }
    if let Some(rest) = label.strip_prefix("compose-") {
        return simple(rest) && url.starts_with("index.html?view=compose&") && !url.contains(['#', '/', '\\']);
    }
    false
}

/// Ouvre (ou ramène au premier plan) une fenêtre secondaire : message, rédaction.
/// Seules la fenêtre principale et les fenêtres de message (« Répondre ») en
/// ouvrent ; une fenêtre de rédaction ne peut pas en créer d'autres.
#[tauri::command]
#[expect(clippy::too_many_arguments, reason = "commande Tauri : l'interface passe chaque argument séparément")]
pub(crate) async fn open_window(
    app: AppHandle,
    caller: tauri::WebviewWindow,
    label: String,
    url: String,
    title: String,
    width: f64,
    height: f64,
    sources: Option<Vec<u64>>,
    init: Option<String>,
    settings: Option<String>,
) -> Result<(), String> {
    let from = caller.label();
    if !(from == MAIN || from.starts_with("message-")) || !allowed_window(&label, &url) {
        return Err(tr!("Ouverture de fenêtre refusée.").into());
    }
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.set_focus();
        return Ok(());
    }
    let settings = boot_settings(from, settings);
    if settings.len() > MAX_SETTINGS || init.as_ref().is_some_and(|i| i.len() > MAX_INIT) {
        return Err(tr!("Ouverture de fenêtre refusée.").into());
    }
    remember_boot(&label, WindowBoot { settings, init: init.filter(|_| label.starts_with("compose-")) });
    if label.starts_with("compose-") {
        grant(&label, sources.unwrap_or_default());
    }
    let mut builder = WebviewWindowBuilder::new(&app, label.clone(), WebviewUrl::App(url.into()))
        .on_new_window(on_new_window(app.clone(), label))
        .title(title)
        .inner_size(width, height)
        .min_inner_size(560.0, 420.0)
        .center();
    // Stockage du moteur web séparé de celui de la fenêtre principale (lot 9c).
    if let Some(dir) = secondary_data_dir(&app) {
        builder = builder.data_directory(dir);
    }
    builder.build().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{allowed_window, boot_settings, forget, grant, may_reuse, remember_boot, take_boot, valid_event, WindowBoot, WindowEvent};

    #[test]
    fn b8_fenetre_fermee_oubliee() {
        remember_boot("compose-test-b8", WindowBoot { settings: "s".into(), init: Some("brouillon".into()) });
        grant("compose-test-b8", vec![5]);
        forget("compose-test-b8");
        assert!(!may_reuse("compose-test-b8", 5), "plus de pièces jointes reprises");
        assert_eq!(take_boot("compose-test-b8"), WindowBoot::default(), "brouillon de départ oublié");
    }

    #[test]
    fn reglages_confies_par_la_fenetre_principale_seulement() {
        remember_boot("message-test-77", WindowBoot { settings: "{\"vortex.theme\":\"dark\"}".into(), init: None });
        // Une fenêtre de message transmet les réglages reçus, pas les siens.
        assert_eq!(boot_settings("message-test-77", Some("{\"vortex.mail\":\"piege\"}".into())), "{\"vortex.theme\":\"dark\"}");
        assert_eq!(boot_settings("main", Some("{}".into())), "{}");
        assert_eq!(boot_settings("message-inconnue", Some("{\"x\":1}".into())), "");
        // Le brouillon de départ n'est rendu qu'une fois ; les réglages restent.
        remember_boot("compose-test-1", WindowBoot { settings: "s".into(), init: Some("brouillon".into()) });
        assert_eq!(take_boot("compose-test-1"), WindowBoot { settings: "s".into(), init: Some("brouillon".into()) });
        assert_eq!(take_boot("compose-test-1"), WindowBoot { settings: "s".into(), init: None });
    }

    #[test]
    fn evenements_des_fenetres_verifies() {
        let ok: WindowEvent = serde_json::from_str(r#"{"type":"moved","ids":[1,2]}"#).unwrap();
        assert!(valid_event(&ok));
        let sent: WindowEvent = serde_json::from_str(
            r#"{"type":"sent","report":{"recipients":2,"savedIn":"Envoyés","warning":null},"accountId":"a","answeredId":null}"#,
        )
        .unwrap();
        assert!(valid_event(&sent));
        // Forme inconnue, ou texte démesuré : refusé.
        assert!(serde_json::from_str::<WindowEvent>(r#"{"type":"rules","rules":[]}"#).is_err());
        // Lot E1 : « envoi en cours » accepté d'une fenêtre ; l'échec d'un envoi,
        // émis par le backend seul, ne l'est jamais.
        let sending: WindowEvent = serde_json::from_str(r#"{"type":"sending","accountId":"a"}"#).unwrap();
        assert!(valid_event(&sending));
        assert!(serde_json::from_str::<WindowEvent>(r#"{"type":"sendFailed","error":"x"}"#).is_err());
        let failed = super::SendFailed {
            error: "refus".into(),
            account_id: "a".into(),
            draft: mail_core::Draft::default(),
            attachments: Vec::new(),
            answered_id: None,
            draft_id: Some(3),
            window: "compose-1".into(),
        };
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!((json["type"].as_str(), json["draftId"].as_u64()), (Some("sendFailed"), Some(3)));
        let long = format!(
            r#"{{"type":"sent","report":{{"recipients":1,"savedIn":null,"warning":"{}"}},"accountId":"a","answeredId":null}}"#,
            "x".repeat(600)
        );
        assert!(!valid_event(&serde_json::from_str(&long).unwrap()));
    }

    #[test]
    fn pieces_jointes_reprises_seulement_si_accordees() {
        grant("compose-test-transfert", vec![12, 13]);
        assert!(may_reuse("compose-test-transfert", 12));
        assert!(!may_reuse("compose-test-transfert", 99));
        // Une autre fenêtre de rédaction n'hérite de rien ; la principale peut tout.
        assert!(!may_reuse("compose-test-autre", 12));
        assert!(may_reuse("main", 99));
        // Rouvrir la même fenêtre remplace l'accord précédent.
        grant("compose-test-transfert", vec![]);
        assert!(!may_reuse("compose-test-transfert", 12));
    }

    #[test]
    fn libelle_et_page_doivent_correspondre() {
        assert!(allowed_window("message-42", "index.html?view=message&id=42"));
        assert!(allowed_window("compose-1731170000-reply-7", "index.html?view=compose&key=1731170000-reply-7"));
        assert!(allowed_window("compose-draft-9", "index.html?view=compose&draft=9"));
        // Libellé d'une autre famille, page différente, ou fenêtre principale.
        assert!(!allowed_window("main", "index.html"));
        assert!(!allowed_window("message-42", "index.html?view=compose&key=x"));
        assert!(!allowed_window("message-42", "index.html?view=message&id=43"));
        assert!(!allowed_window("compose-x", "index.html?view=message&id=1"));
        assert!(!allowed_window("compose-x", "https://example.invalid/?view=compose&"));
        assert!(!allowed_window("compose-a_b", "index.html?view=compose&key=a"));
        assert!(!allowed_window("settings", "index.html?view=compose&key=a"));
    }
}
