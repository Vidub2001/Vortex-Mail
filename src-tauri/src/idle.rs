//! Nouveaux messages en temps réel : pour chaque compte, une connexion dédiée
//! reste en écoute IMAP IDLE (RFC 2177) sur la boîte de réception.
//!
//! À chaque changement signalé par le serveur, événement `mail-changed` vers
//! l'interface : elle resynchronise l'affichage, fait passer les règles, puis
//! annonce les nouveaux messages restés dans la boîte (lot B4 : plus de
//! notification ici, avant les règles, pour un message qu'elles suppriment).
//!
//! La connexion se rétablit seule après une coupure ou une mise en veille.
//! Un serveur sans IDLE garde la synchronisation périodique de l'interface.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::config::AccountConfig;
use crate::transport;

/// IDLE est renouvelé régulièrement : la RFC demande moins de 29 minutes, et
/// certains routeurs coupent plus tôt une connexion silencieuse.
const RENEW: Duration = Duration::from_mins(9);
/// Attente avant une nouvelle tentative de connexion.
const RETRY: Duration = Duration::from_secs(60);

pub(crate) const EVENT: &str = "mail-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MailChanged {
    pub account_id: String,
    pub folder_id: String,
}

/// Fils d'écoute en cours, un par compte.
pub struct Watchers {
    app: AppHandle,
    running: HashMap<String, Arc<AtomicBool>>,
}

impl std::fmt::Debug for Watchers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut accounts: Vec<&String> = self.running.keys().collect();
        accounts.sort();
        f.debug_struct("Watchers").field("accounts", &accounts).finish_non_exhaustive()
    }
}

impl Watchers {
    pub fn new(app: AppHandle) -> Self {
        Self { app, running: HashMap::new() }
    }

    /// Démarre (ou redémarre) l'écoute d'un compte.
    pub fn start(&mut self, config: AccountConfig) {
        self.stop(&config.id);
        let stop = Arc::new(AtomicBool::new(false));
        self.running.insert(config.id.clone(), Arc::clone(&stop));
        let app = self.app.clone();
        let spawned = thread::Builder::new().name(format!("idle-{}", config.id)).spawn(move || watch(&app, &config, &stop));
        if let Err(e) = spawned {
            crate::diag::report(&format_args!("écoute impossible à démarrer : {e}"));
        }
    }

    /// Arrête l'écoute d'un compte (effectif au plus tard à la fin du délai de lecture).
    pub fn stop(&mut self, account_id: &str) {
        if let Some(flag) = self.running.remove(account_id) {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

fn stopped(stop: &AtomicBool) -> bool {
    stop.load(Ordering::Relaxed)
}

fn pause(stop: &AtomicBool, duration: Duration) {
    let tick = Duration::from_millis(500);
    let mut waited = Duration::ZERO;
    while waited < duration && !stopped(stop) {
        thread::sleep(tick);
        waited += tick;
    }
}

fn watch(app: &AppHandle, config: &AccountConfig, stop: &AtomicBool) {
    let folder_id = format!("{}:INBOX", config.id);
    while !stopped(stop) {
        let mut session = match transport::open_imap(&config.imap) {
            Ok((session, _)) => session,
            Err(e) => {
                crate::diag::report(&format_args!("écoute de « {} » impossible : {e}", config.name));
                pause(stop, RETRY);
                continue;
            }
        };
        if !session.has_capability("IDLE") {
            crate::diag::report(&format_args!("« {} » ne propose pas IMAP IDLE ; synchronisation périodique seulement", config.name));
            let _ = session.logout();
            return;
        }
        if let Err(e) = session.select("INBOX") {
            crate::diag::report(&format_args!("« {} » : {e}", config.name));
            pause(stop, RETRY);
            continue;
        }

        loop {
            if stopped(stop) {
                let _ = session.logout();
                return;
            }
            match session.idle(RENEW, || !stopped(stop)) {
                Ok(false) => {} // délai atteint : IDLE est simplement relancé
                Ok(true) => {
                    let event = MailChanged { account_id: config.id.clone(), folder_id: folder_id.clone() };
                    if let Err(e) = app.emit(EVENT, event) {
                        crate::diag::report(&e);
                    }
                }
                Err(e) => {
                    crate::diag::report(&format_args!("écoute de « {} » interrompue : {e}", config.name));
                    break; // reconnexion
                }
            }
        }
        pause(stop, Duration::from_secs(5));
    }
}

/// Arguments de `notify-send` (relecture S5). Le titre et le texte viennent
/// d'un mail (nom d'expéditeur, objet) : `--` les empêche d'être lus comme
/// des options (`--help`, `--app-name=…`), et le texte, que les serveurs de
/// notification interprètent comme du balisage, est échappé (pas de lien
/// `<a>` cliquable). Le titre est du texte brut selon la norme : l'échapper
/// afficherait `&amp;`.
pub(crate) fn notify_send_args(summary: &str, body: &str, icon: &str, category: &str) -> Vec<String> {
    let escaped = body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    vec!["--app-name=Vortex Mail".into(), format!("--icon={icon}"), format!("--category={category}"), "--".into(), summary.into(), escaped]
}

/// Notification du bureau : libnotify sous Linux, module de notifications de
/// Tauri ailleurs (Windows, macOS). `icon` et `category` : noms freedesktop (Linux).
pub(crate) fn desktop_notify(app: &AppHandle, summary: &str, body: &str, icon: &str, category: &str) {
    #[cfg(target_os = "linux")]
    {
        let shown = std::process::Command::new("notify-send")
            .args(notify_send_args(summary, body, icon, category))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match shown {
            Ok(mut child) => {
                thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => crate::diag::report(&format_args!("notification impossible (notify-send) : {e}")),
        }
    }
    #[cfg(target_os = "linux")]
    let _ = app;
    #[cfg(not(target_os = "linux"))]
    let _ = (icon, category);
    #[cfg(not(target_os = "linux"))]
    {
        use tauri_plugin_notification::NotificationExt;
        if let Err(e) = app.notification().builder().title(summary).body(body).show() {
            crate::diag::report(&format_args!("notification impossible : {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::notify_send_args;

    #[test]
    fn arguments_de_notify_send() {
        let args = notify_send_args("--help", "<a href=\"https://x\">Vérifiez</a> & co", "mail-unread", "email.arrived");
        let sep = args.iter().position(|a| a == "--").expect("séparateur --");
        // Titre et texte après `--` : jamais lus comme des options.
        assert_eq!(&args[sep + 1..], ["--help", "&lt;a href=\"https://x\"&gt;Vérifiez&lt;/a&gt; &amp; co"]);
        assert!(args[..sep].iter().all(|a| a.starts_with("--")));
    }
}
