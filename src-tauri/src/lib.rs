//! Vortex Mail : application Tauri (commandes, comptes, cache, synchronisation).
//!
//! Les modules restent privés ; `corpus_api` et `fuzz_api` exposent aux
//! tests et au fuzzing les seules fonctions qu'ils appellent.

// En premier : la macro `tr!` sert dans tous les modules.
#[macro_use]
mod i18n;
pub mod attachments;
mod auth;
mod authres;
mod autoconfig;
mod backup;
mod cache;
mod commands;
mod config;
mod crypto;
mod dav;
mod diag;
mod dns;
mod endpoints;
mod google;
mod http;
mod idle;
mod images;
mod imip;
mod journal;
mod json;
mod migrate;
mod mimeparse;
mod o365;
mod oauth;
mod pim;
mod pimfmt;
mod portable;
mod router;
pub mod sanitize;
mod secrets;
mod smime;
mod transport;
mod tray;
mod tz;
mod windows;
mod xml;

/// Fonctions internes exposées aux tests du corpus (`tests/corpus.rs`), sans
/// rendre publics les modules qui les portent.
#[doc(hidden)]
pub mod corpus_api {
    /// Adresses des images distantes d'un corps HTML, telles que « Afficher
    /// les images » les téléchargerait.
    #[must_use]
    pub fn remote_image_urls(html: &str) -> Vec<String> {
        crate::images::find_remote(html).into_iter().map(|found| found.url).collect()
    }

    /// Une image à cette adresse serait-elle téléchargée ?
    ///
    /// # Errors
    ///
    /// Adresse invalide (caractère de contrôle…) ou locale, privée, réservée.
    pub fn check_image_url(url: &str) -> Result<(), String> {
        crate::http::check_image_url(url)
    }

    /// Arguments passés à `notify-send` pour une notification.
    #[must_use]
    pub fn notify_send_args(summary: &str, body: &str) -> Vec<String> {
        crate::idle::notify_send_args(summary, body, "mail-unread", "email.arrived")
    }

    /// Noms des pièces jointes d'un message, tels que l'interface les affiche.
    #[must_use]
    pub fn attachment_names(raw: &[u8]) -> Vec<String> {
        crate::mimeparse::parse(raw).attachments.into_iter().map(|a| a.name).collect()
    }

    /// Rappels (en minutes avant le début) des rendez-vous d'un fichier
    /// iCalendar, tels que l'agenda les retient ; `None` : absent ou ignoré.
    #[must_use]
    pub fn ical_reminders(ics: &str) -> Vec<Option<i64>> {
        crate::pimfmt::parse_ical(ics).into_iter().map(|e| e.reminder).collect()
    }

    /// Ce que signale l'interface de l'expéditeur d'un message (lot G1) :
    /// vide, `dmarc` ou `spf`.
    #[must_use]
    pub fn sender_check(raw: &[u8]) -> &'static str {
        crate::authres::verdict(raw)
    }

    /// Pour chaque rendez-vous répété d'un fichier iCalendar : sa répétition
    /// serait-elle acceptée à l'identique par un agenda Outlook.com ?
    #[must_use]
    pub fn o365_accepts_recurrences(ics: &str) -> Vec<bool> {
        crate::pimfmt::parse_ical(ics)
            .into_iter()
            .filter(|e| !e.rrule.is_empty())
            .map(|e| crate::o365::recurrence_from_rule(&e.rrule, &e.start).is_some())
            .collect()
    }
}

/// Points d'entrée des cibles de fuzzing (`fuzz/`, lot 8) : les lecteurs de ce
/// qui arrive de l'extérieur, appelés comme le fait l'application, sans rendre
/// publics les modules qui les portent. Les résultats sont jetés : seul compte
/// qu'aucune entrée ne fasse paniquer, déborder ou boucler.
#[doc(hidden)]
pub mod fuzz_api {
    /// Un message reçu, décodé comme à l'affichage ; sa première pièce jointe
    /// passe par le nettoyage du nom et le contrôle avant ouverture.
    pub fn message(raw: &[u8]) {
        let parsed = crate::mimeparse::parse(raw);
        let html = crate::mimeparse::display_html(&parsed);
        let _ = crate::mimeparse::preview(&parsed);
        let _ = crate::sanitize::simplify(&crate::sanitize::strip_refresh(&html));
        let _ = crate::mimeparse::looks_like_attachment(raw);
        if let Some(first) = crate::mimeparse::attachment(raw, 0) {
            let _ = crate::attachments::safe_file_name(&first.name);
            let _ = crate::attachments::check_open(&first.name, &first.data);
        }
    }

    /// Une réponse HTTP brute (hébergeur d'images, serveur `WebDAV`, API,
    /// serveur de jetons) : en-têtes, longueur annoncée, découpage `chunked`.
    pub fn http(raw: &[u8]) {
        let _ = crate::http::parse_response(raw);
        let _ = crate::oauth::parse_http_response(raw);
        let _ = crate::oauth::decode_chunked(raw);
    }

    /// Un document `clientConfig` (fichier publié par un domaine, copie de
    /// l'ISPDB) : serveurs choisis, identifiant, nom et aide du fournisseur (lot I1).
    pub fn autoconfig(raw: &[u8]) {
        crate::autoconfig::fuzz_document(&String::from_utf8_lossy(raw));
    }

    /// Les réponses d'un serveur SMTP, lues l'une après l'autre jusqu'à la
    /// fin des données ou à la première erreur.
    pub fn smtp(raw: &[u8]) {
        let mut stream = std::io::Cursor::new(raw.to_vec());
        while crate::transport::smtp_reply(&mut stream).is_ok() {}
    }

    /// Un fichier de sauvegarde ou le coffre portable : l'enveloppe (sans
    /// calcul de clé), puis le contenu tel qu'il serait une fois déchiffré.
    pub fn backup(text: &str) {
        let _ = crate::crypto::envelope(text);
        if let Ok(snapshot) = crate::backup::from_payload(text) {
            let _ = serde_json::from_str::<Vec<crate::config::AccountConfig>>(&snapshot.accounts_json);
        }
        let _ = crate::backup::parse_settings(text);
    }

    /// Le retour de la page de connexion `OAuth2` (adresse appelée par le
    /// navigateur) et le jeton d'identité.
    pub fn oauth(text: &str) {
        let _ = crate::oauth::parse_query(text);
        let _ = crate::oauth::id_token_email(text);
        let _ = crate::oauth::signed_in_as(Some(text), "client");
        let _ = crate::oauth::url_decode(text);
    }

    /// Les en-têtes d'un message reçu repris pour y répondre (`References`,
    /// destinataires, `Message-ID`).
    pub fn headers(raw: &[u8]) {
        for name in ["References", "To", "Cc", "Message-ID"] {
            if let Some(value) = mail_core::compose::header_value(raw, name) {
                let _ = mail_core::compose::parse_address_list(&value);
            }
        }
    }

    /// Un fichier importé (`.eml` ou `.mbox`, lot E8a) : découpé en messages,
    /// réécrit en mbox, puis relu.
    pub fn mbox(data: &[u8]) {
        let messages = mail_core::mbox::split(data);
        let mut out = Vec::new();
        for raw in &messages {
            let _ = mail_core::mbox::write_message(&mut out, raw, "a@b.c", 0);
        }
        let _ = mail_core::mbox::split(&out);
    }

    /// Un corps HTML, nettoyé comme à l'affichage.
    pub fn html(html: &str) {
        let cleaned = crate::sanitize::strip_refresh(html);
        let _ = crate::sanitize::simplify(&cleaned);
        let _ = crate::images::find_remote(&cleaned);
    }

    /// Un fichier iCalendar : lecture, réécriture du premier rendez-vous
    /// (avec le fichier d'origine, comme après une modification), conversion
    /// des répétitions pour Outlook.com.
    pub fn ical(text: &str) {
        let _ = crate::pimfmt::calendar_method(text);
        // Import d'un agenda (lot E8b) : blocs repérés un par un.
        let _ = crate::pimfmt::component_blocks(text, "VTIMEZONE");
        let events = crate::pimfmt::parse_ical(text);
        if let Some(first) = events.first() {
            let _ = crate::pimfmt::write_ical(first, Some(text), "20260101T000000Z");
        }
        for event in events.iter().filter(|e| !e.rrule.is_empty()) {
            let _ = crate::o365::recurrence_from_rule(&event.rrule, &event.start);
        }
    }

    /// Une carte de visite : lecture, réécriture, relecture.
    pub fn vcard(text: &str) {
        // Import d'un carnet (lot E8b) : fiches repérées une par une.
        let _ = crate::pimfmt::component_blocks(text, "VCARD");
        if let Some(contact) = crate::pimfmt::parse_vcard(text) {
            let written = crate::pimfmt::write_vcard(&contact, Some(text));
            let _ = crate::pimfmt::parse_vcard(&written);
            // Nom, société et numéros changés : composants et lignes d'origine réécrits.
            let edited = crate::pimfmt::Contact {
                given_name: format!("{}x", contact.given_name),
                organization: format!("{}x", contact.organization),
                phones: contact.phones.iter().skip(1).cloned().collect(),
                ..contact
            };
            let _ = crate::pimfmt::write_vcard(&edited, Some(text));
        }
    }

    /// Une réponse JSON de l'API d'Outlook.com, lue comme par l'agenda et
    /// les contacts : rendez-vous et fiches convertis, fiche réécrite puis
    /// relue, raison d'une erreur (relecture complète n° 47).
    pub fn json(text: &str) {
        let _ = crate::o365::error_reason(400, text.as_bytes());
        let _ = crate::o365::is_series_instance(text);
        let _ = crate::o365::is_series_master(text);
        let Ok(value) = crate::json::parse(text) else { return };
        let one = std::slice::from_ref(&value);
        let list = value.get("value").items();
        for v in one.iter().chain(list) {
            if let Some(event) = crate::o365::event_from(v, "compte", "agenda") {
                let _ = crate::o365::event_body(&event, None, None, "Europe/Paris").to_text();
                let _ = crate::o365::event_body(&event, None, Some(v), "").to_text();
            }
            if let Some(contact) = crate::o365::contact_from(v, "compte") {
                let _ = crate::json::parse(&crate::o365::contact_body(&contact, Some(v)).to_text());
            }
        }
    }

    /// Une réponse XML d'un serveur d'agenda ou de contacts (`WebDAV`),
    /// parcourue comme la lisent l'agenda et les contacts : éléments, adresses
    /// résolues et contrôlées, raison d'une erreur (relecture complète n° 47).
    pub fn dav_xml(text: &str) {
        const BASE: &str = "https://dav.exemple.fr/cal/";
        let root = crate::xml::parse(text);
        let scope = crate::dav::Scope::new(&["moi@exemple.fr"]);
        for name in ["calendar-data", "address-data"] {
            for item in crate::dav::items_from(&root, BASE, name) {
                let _ = scope.allows(&item.url);
            }
        }
        let _ = crate::dav::server_reason(text.as_bytes());
        let _ = root.text_content();
    }

    /// Une adresse reçue d'un serveur (`Location`, `href`, `@odata.nextLink`)
    /// ou d'un message : découpage, résolution, contrôle des sites du compte.
    pub fn url(text: &str) {
        let _ = crate::http::split_url(text);
        let _ = crate::http::origin(text);
        let resolved = crate::http::resolve("https://dav.exemple.fr/cal/", text);
        let _ = crate::http::split_url(&resolved);
        let _ = crate::dav::Scope::new(&["moi@exemple.fr", text]).allows(&resolved);
    }
}

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mail_core::Journal;
use tauri::Manager;

use crate::idle::Watchers;

use crate::cache::Cache;
use crate::config::AccountConfig;
use crate::router::Router;

/// État partagé entre les commandes.
///
/// `router` détient les connexions réseau (opérations parfois longues) ;
/// `cache` sert l'affichage et ne se verrouille que brièvement.
/// Ordre de verrouillage, toujours le même : `journal`, `configs`, puis `cache`
/// (les connexions de chaque compte ont leurs propres verrous, voir `router.rs`).
pub struct AppState {
    /// Sans verrou global : chaque compte a ses propres verrous et connexions.
    pub router: Arc<Router>,
    /// Second routeur, avec ses propres connexions, réservé au téléchargement
    /// pour la lecture hors connexion : il ne bloque jamais les actions.
    pub background: Arc<Router>,
    /// Copie des réglages des comptes, lisible sans attendre le réseau.
    pub configs: Arc<Mutex<Vec<AccountConfig>>>,
    /// Journal des déplacements entre comptes, repris au démarrage.
    pub journal: Arc<Mutex<Journal>>,
    /// Écoute IMAP IDLE de chaque compte (nouveaux messages en temps réel).
    pub watchers: Arc<Mutex<Watchers>>,
    /// Cache local (`SQLite`) des dossiers et des messages.
    pub cache: Arc<Mutex<Cache>>,
    /// Fichier des réglages des comptes.
    pub config_file: PathBuf,
    /// Copies temporaires des pièces jointes ouvertes (vidé au démarrage).
    pub files_dir: PathBuf,
    /// Destination de « Télécharger ».
    pub downloads_dir: PathBuf,
}

/// Ne prend aucun verrou : n'affiche que les emplacements de fichiers.
impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("config_file", &self.config_file)
            .field("files_dir", &self.files_dir)
            .field("downloads_dir", &self.downloads_dir)
            .finish_non_exhaustive()
    }
}

/// Pièces jointes ouvertes pendant la session : effacées à la fermeture.
fn on_event(app: &tauri::AppHandle, event: &tauri::RunEvent) {
    if matches!(event, tauri::RunEvent::Exit) {
        if let Some(state) = app.try_state::<AppState>() {
            attachments::clean_temp(&state.files_dir);
        }
    }
}

/// Lance l'application.
///
/// # Panics
///
/// Si Tauri ne peut pas démarrer (fenêtre, `WebView`), ou si même une base
/// `SQLite` en mémoire ne peut pas s'ouvrir.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[expect(clippy::too_many_lines, reason = "assemblage de l'application Tauri : état, greffons et liste des commandes")]
pub fn run() {
    // Mesure du démarrage (lot E2) : `VORTEX_MESURE=1` écrit chaque étape sur la sortie d'erreur.
    diag::start_measure();
    // Avant tout accès aux dossiers de l'application : reprise des données « FluxMail »
    // (sans objet en mode portable : tout est dans le dossier `data`).
    if !portable::is_portable() {
        migrate::rename_data_dirs();
    }
    // Une seule instance : relancer Vortex Mail (par exemple fenêtre cachée
    // dans la zone de notification) montre la fenêtre existante, au lieu
    // d'ouvrir une seconde application sur le même cache (lot B3).
    let builder = tauri::Builder::default().plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_main(app)));
    // Notifications du bureau hors Linux (sous Linux : notify-send, voir idle.rs).
    #[cfg(not(target_os = "linux"))]
    let builder = builder.plugin(tauri_plugin_notification::init());
    builder
        .setup(|app| {
            diag::step("setup : début");
            // Mode portable : tout dans le dossier `data` à côté de l'exécutable.
            let portable_dir = portable::data_dir().map(Path::to_path_buf);
            let (config_dir, data_dir) = match &portable_dir {
                Some(dir) => (dir.join("config"), dir.clone()),
                None => (app.path().app_config_dir()?, app.path().app_data_dir()?),
            };
            let config_file = config_dir.join("accounts.json");
            // S/MIME : autorités ajoutées par l'utilisateur (lot D1, dossier créé
            // d'avance pour qu'il le trouve) et certificats des comptes (lot D2).
            smime::set_dir(config_dir.join("smime"));
            let accounts = if portable_dir.is_some() {
                // Comptes chargés au déverrouillage du coffre (mot de passe maître).
                Vec::new()
            } else {
                // Mots de passe lus dans le trousseau (et migrés s'ils étaient en clair).
                let accounts = secrets::load_accounts(&config_file);
                diag::step(&format_args!("comptes lus dans le trousseau ({})", accounts.len()));
                // Entrées du trousseau des versions précédentes : reprises en arrière-plan.
                let known = accounts.clone();
                std::thread::spawn(move || backup::take_over_old_keyring(&known));
                accounts
            };
            let cache_file = data_dir.join("cache.sqlite");
            // Cache chiffré, clé dans le trousseau (lot G4). En mode portable, la clé
            // est dans le coffre, ouvert seulement par le mot de passe maître : cache
            // en mémoire jusque-là, remplacé au déverrouillage (lot G4b,
            // `commands::portable_unlock`). Rien ne le lit avant : l'interface attend
            // le déverrouillage, et aucun compte n'est chargé.
            let opened = if portable_dir.is_some() { Cache::in_memory() } else { cache::open_profile(&cache_file) };
            let cache = opened.unwrap_or_else(|e| {
                crate::diag::report(&format_args!("{e} ; cache en mémoire pour cette session"));
                Cache::in_memory().expect("SQLite en mémoire")
            });
            diag::step("cache ouvert");
            let cache = Arc::new(Mutex::new(cache));
            // Pièces jointes ouvertes : fichiers temporaires (dossier temporaire du PC en mode portable).
            let files_dir = if portable_dir.is_some() {
                // Dossier temporaire du PC, partagé entre utilisateurs sous
                // Linux et macOS : nom tiré au hasard à chaque session, qu'un
                // autre utilisateur ne peut pas préparer à l'avance (relecture
                // du 10 novembre 2026).
                // Sans aléa, pas de nom prévisible dans le dossier partagé : le
                // dossier `data` de la clé, propre à l'utilisateur (relecture
                // complète n° 26).
                match (crate::oauth::random_bytes::<8>(), &portable_dir) {
                    (Ok(bytes), _) => std::env::temp_dir().join(format!("vortexmail-portable-{:016x}", u64::from_le_bytes(bytes))),
                    (Err(e), Some(dir)) => {
                        crate::diag::report(&e);
                        dir.join("tmp")
                    }
                    (Err(_), None) => std::env::temp_dir().join("vortexmail-attachments"),
                }
            } else {
                app.path().app_cache_dir().map_or_else(|_| std::env::temp_dir().join("vortexmail-attachments"), |d| d.join("attachments"))
            };
            attachments::clean_temp(&files_dir);
            diag::step("fichiers temporaires nettoyés");
            let downloads_dir =
                app.path().download_dir().unwrap_or_else(|_| std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from));
            // Déplacements interrompus lors de la session précédente : repris au
            // premier passage de `flush_pending` (dès que le réseau répond).
            let journal = journal::load(Arc::clone(&cache));
            diag::step("journal des déplacements lu");
            // Connexions OAuth2 à refaire : signalées à l'interface (« Reconnecter »).
            auth::set_app(app.handle().clone());
            let mut watchers = Watchers::new(app.handle().clone());
            for config in &accounts {
                watchers.start(config.clone());
            }
            app.manage(AppState {
                background: Arc::new(Router::new(accounts.clone(), Arc::clone(&cache))),
                configs: Arc::new(Mutex::new(accounts.clone())),
                router: Arc::new(Router::new(accounts, Arc::clone(&cache))),
                journal: Arc::new(Mutex::new(journal)),
                watchers: Arc::new(Mutex::new(watchers)),
                cache,
                config_file,
                files_dir,
                downloads_dir,
            });
            // Fenêtre principale (stockage du moteur web dans `data` en mode portable).
            diag::step("surveillance des boîtes lancée");
            windows::create_main(app.handle())?;
            diag::step("fenêtre principale créée");
            // Icône de la zone de notification (lot B3).
            tray::create(app.handle());
            Ok(())
        })
        .on_window_event(tray::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::list_accounts,
            commands::show_reminder,
            commands::notify_new_mail,
            commands::set_keep_in_tray,
            commands::set_language,
            commands::auth_pending,
            commands::portable_status,
            commands::portable_unlock,
            commands::portable_change_password,
            windows::open_window,
            windows::window_bootstrap,
            windows::notify_main,
            windows::save_draft_backup,
            windows::draft_backups,
            windows::clear_draft_backup,
            commands::list_folders,
            commands::sync_folders,
            commands::list_messages,
            commands::sync_messages,
            commands::sync_messages_many,
            commands::get_message,
            commands::set_seen,
            commands::set_flagged,
            commands::move_messages,
            commands::move_to_role,
            commands::send_message,
            commands::open_attachment,
            commands::save_attachment,
            commands::test_account,
            commands::add_account,
            commands::update_account,
            commands::remove_account,
            commands::list_account_configs,
            commands::oauth_sign_in,
            commands::prefetch_step,
            commands::show_images,
            commands::check_signature,
            commands::smime_identity,
            commands::smime_import,
            commands::smime_remove,
            commands::smime_sign_by_default,
            commands::smime_signers,
            commands::smime_missing,
            commands::smime_correspondents,
            commands::smime_forget_correspondent,
            commands::create_folder,
            commands::delete_folder,
            commands::empty_folder,
            commands::export_eml,
            commands::export_mbox,
            commands::import_messages,
            commands::export_pim,
            commands::import_pim,
            commands::detect_account,
            commands::bridge_certificate,
            commands::delete_forever,
            commands::oauth_status,
            commands::google_sign_in,
            commands::google_status,
            commands::google_sign_out,
            commands::export_backup,
            commands::inspect_backup,
            commands::import_backup,
            commands::restart_app,
            commands::o365_sign_in,
            commands::o365_status,
            commands::o365_sign_out,
            commands::open_url,
            commands::save_draft,
            commands::discard_draft,
            commands::open_draft,
            commands::search_local,
            commands::search_messages,
            commands::flush_pending,
            commands::pending_count,
            commands::rename_folder,
            commands::move_folder,
            commands::pim_sync,
            commands::pim_collections,
            commands::contacts_list,
            commands::contact_save,
            commands::events_list,
            commands::event_save,
            commands::pim_delete,
            commands::pim_source,
            commands::address_suggest,
            commands::read_invitation,
            commands::answer_invitation,
            commands::answer_event,
        ])
        .build(tauri::generate_context!())
        .expect("erreur au lancement de Vortex Mail")
        .run(|app, event| on_event(app, &event));
}
