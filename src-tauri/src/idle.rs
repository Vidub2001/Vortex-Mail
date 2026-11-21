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

/// Lanceur installé par les paquets (`Vortex Mail.desktop`, d'après
/// `productName`) : GNOME y rattache les notifications, les empile sous
/// l'application et permet de toutes les effacer d'un coup (lot M1).
const DESKTOP_ENTRY: &str = "Vortex Mail";

/// Arguments de `notify-send` (relecture S5). Le titre et le texte viennent
/// d'un mail (nom d'expéditeur, objet) : `--` les empêche d'être lus comme
/// des options (`--help`, `--app-name=…`), et le texte, que les serveurs de
/// notification interprètent comme du balisage, est échappé (pas de lien
/// `<a>` cliquable). Le titre est du texte brut selon la norme : l'échapper
/// afficherait `&amp;`. `options` : options de plus, placées avant `--`
/// (remplacement d'une notification, lot M1).
pub(crate) fn notify_send_args(summary: &str, body: &str, icon: &str, category: &str, options: &[String]) -> Vec<String> {
    let escaped = body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut args = vec![
        "--app-name=Vortex Mail".into(),
        format!("--hint=string:desktop-entry:{DESKTOP_ENTRY}"),
        format!("--icon={icon}"),
        format!("--category={category}"),
    ];
    args.extend_from_slice(options);
    args.extend(["--".into(), summary.into(), escaped]);
    args
}

/// Comment montrer une notification qui en remplace peut-être une autre (lot M1).
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shown {
    /// Nouvelle notification, suivie jusqu'à sa fermeture pour en connaître l'identifiant.
    Tracked,
    /// Remplace la notification encore ouverte qui porte cet identifiant.
    Replacing(u32),
}

/// Options de [`notify_send_args`] selon [`Shown`].
#[cfg(target_os = "linux")]
pub(crate) fn replace_args(shown: Shown) -> Vec<String> {
    match shown {
        // `--wait` garde `notify-send` en vie jusqu'à la fermeture ; `--print-id`
        // donne l'identifiant à réutiliser pour les remplacements.
        Shown::Tracked => vec!["--print-id".into(), "--wait".into()],
        Shown::Replacing(id) => vec![format!("--replace-id={id}")],
    }
}

/// Notification « nouveaux messages » encore affichée pour un compte (lots
/// M1 et P) : le nombre de messages qu'elle annonce, et sous Linux
/// l'identifiant que le serveur de notifications lui a donné.
#[cfg(any(target_os = "linux", windows))]
#[derive(Debug, Clone, Copy)]
struct Open {
    count: u32,
    #[cfg(target_os = "linux")]
    id: u32,
}

/// Notifications « nouveaux messages » affichées, par compte.
#[cfg(any(target_os = "linux", windows))]
static OPEN: std::sync::LazyLock<std::sync::Mutex<HashMap<String, Open>>> = std::sync::LazyLock::new(Default::default);

/// Oublie la notification `id` du compte `key` si c'est encore celle-là.
#[cfg(target_os = "linux")]
fn forget_open(key: &str, id: u32) {
    if let Ok(mut open) = OPEN.lock() {
        if open.get(key).is_some_and(|o| o.id == id) {
            open.remove(key);
        }
    }
}

/// Icône et catégorie freedesktop des notifications « nouveaux messages ».
const MAIL_ICON: &str = "mail-unread";
const MAIL_CATEGORY: &str = "email.arrived";

/// Nouveaux messages d'un compte, à annoncer (lots M1 et P).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Arrivals<'a> {
    /// Compte : une notification par compte.
    pub account: &'a str,
    /// Messages arrivés depuis l'annonce précédente.
    pub added: u32,
    /// Titre et texte pour un message seul : expéditeur ; objet et compte.
    pub summary: &'a str,
    pub body: &'a str,
    /// Nom du compte, texte de la notification de plusieurs messages.
    pub account_name: &'a str,
}

/// Titre et texte d'une notification qui annonce `total` messages : ceux du
/// message seul, ou leur nombre.
pub(crate) fn arrival_text(total: u32, a: &Arrivals<'_>) -> (String, String) {
    if total <= 1 {
        (a.summary.to_owned(), a.body.to_owned())
    } else {
        (tr!("{n} nouveaux messages", n = total), a.account_name.to_owned())
    }
}

/// Lance `notify-send` sans attendre sa fin.
#[cfg(target_os = "linux")]
fn spawn_notify_send(summary: &str, body: &str, icon: &str, category: &str, options: &[String]) {
    let shown = std::process::Command::new("notify-send")
        .args(notify_send_args(summary, body, icon, category, options))
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

/// Notification du bureau : libnotify sous Linux, module de notifications de
/// Tauri ailleurs (Windows, macOS). `icon` et `category` : noms freedesktop (Linux).
pub(crate) fn desktop_notify(app: &AppHandle, summary: &str, body: &str, icon: &str, category: &str) {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        spawn_notify_send(summary, body, icon, category, &[]);
    }
    #[cfg(not(target_os = "linux"))]
    {
        use tauri_plugin_notification::NotificationExt;
        let _ = (icon, category);
        if let Err(e) = app.notification().builder().title(summary).body(body).show() {
            crate::diag::report(&format_args!("notification impossible : {e}"));
        }
    }
}

/// Annonce des nouveaux messages d'un compte : une seule notification par
/// compte, remplacée tant qu'elle reste affichée et qui additionne les
/// messages (« 3 nouveaux messages ») ; une fois fermée, la suivante repart
/// de zéro (lots M1 et P). Linux : `notify-send` suivi jusqu'à la fermeture ;
/// Windows : notification à étiquette, retrouvée dans le centre de
/// notifications ; ailleurs, une notification par annonce.
pub(crate) fn notify_arrivals(app: &AppHandle, a: &Arrivals<'_>) {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        let open = OPEN.lock().ok().and_then(|o| o.get(a.account).copied());
        let total = open.map_or(0, |o| o.count).saturating_add(a.added);
        let (summary, body) = arrival_text(total, a);
        match open {
            Some(o) => {
                spawn_notify_send(&summary, &body, MAIL_ICON, MAIL_CATEGORY, &replace_args(Shown::Replacing(o.id)));
                if let Ok(mut map) = OPEN.lock() {
                    if let Some(entry) = map.get_mut(a.account).filter(|e| e.id == o.id) {
                        entry.count = total;
                    }
                }
            }
            None => track_new(a.account, total, &summary, &body),
        }
    }
    #[cfg(windows)]
    {
        let aumid = app.config().identifier.clone();
        let tag = windows_toast::tag_of(a.account);
        // Notification effacée du centre : le compte repart de zéro.
        let shown = windows_toast::is_shown(&aumid, &tag);
        let before = if shown { OPEN.lock().ok().and_then(|o| o.get(a.account).map(|o| o.count)) } else { None };
        let total = before.unwrap_or(0).saturating_add(a.added);
        let (summary, body) = arrival_text(total, a);
        match windows_toast::show(&aumid, &tag, &summary, &body) {
            Ok(()) => {
                if let Ok(mut map) = OPEN.lock() {
                    map.insert(a.account.to_owned(), Open { count: total });
                }
            }
            Err(e) => {
                crate::diag::report(&format_args!("notification remplaçable impossible : {e}"));
                desktop_notify(app, &summary, &body, MAIL_ICON, MAIL_CATEGORY);
            }
        }
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let (summary, body) = arrival_text(a.added, a);
        desktop_notify(app, &summary, &body, MAIL_ICON, MAIL_CATEGORY);
    }
}

/// Montre une notification et la suit jusqu'à sa fermeture : `notify-send
/// --print-id --wait` écrit l'identifiant, puis se termine quand la
/// notification est fermée (effacée, ou ouverte d'un clic). Un seul
/// processus en attente par compte : les remplacements n'attendent pas.
#[cfg(target_os = "linux")]
fn track_new(account: &str, total: u32, summary: &str, body: &str) {
    use std::io::BufRead;

    let spawned = std::process::Command::new("notify-send")
        .args(notify_send_args(summary, body, MAIL_ICON, MAIL_CATEGORY, &replace_args(Shown::Tracked)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            crate::diag::report(&format_args!("notification impossible (notify-send) : {e}"));
            return;
        }
    };
    let stdout = child.stdout.take();
    let key = account.to_owned();
    thread::spawn(move || {
        let mut line = String::new();
        let read = stdout.map(|out| std::io::BufReader::new(out).read_line(&mut line));
        let id = match read {
            Some(Ok(n)) if n > 0 => line.trim().parse::<u32>().ok(),
            _ => None,
        };
        let Some(id) = id else {
            let _ = child.wait();
            return;
        };
        if let Ok(mut open) = OPEN.lock() {
            open.insert(key.clone(), Open { count: total, id });
        }
        let _ = child.wait();
        forget_open(&key, id);
    });
}

/// Notifications Windows à étiquette (lot P) : une notification de même
/// étiquette et de même groupe remplace la précédente dans le centre de
/// notifications, au lieu de s'y ajouter.
#[cfg(windows)]
mod windows_toast {
    use std::fmt::Write as _;

    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    /// Groupe des notifications « nouveaux messages ».
    const GROUP: &str = "nouveaux-messages";

    /// Étiquette d'un compte : courte (Windows en accepte 64 caractères au
    /// plus) et stable, tirée de son identifiant.
    pub(super) fn tag_of(account: &str) -> String {
        let hash = account.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
        format!("compte-{hash:016x}")
    }

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }

    /// Fil sans environnement `WinRT` (tâche de fond) : préparé une fois.
    fn init_winrt() {
        // Déjà prêt, ou prêt dans un autre mode : l'appel suivant le dira.
        // SAFETY: RoInitialize prépare WinRT pour le fil courant, sans précondition ;
        // un fil déjà initialisé rend une erreur, ignorée.
        let _ = unsafe { windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
    }

    /// La notification de cette étiquette est-elle encore dans le centre de notifications ?
    pub(super) fn is_shown(aumid: &str, tag: &str) -> bool {
        init_winrt();
        let Ok(history) = ToastNotificationManager::History() else { return false };
        let Ok(list) = history.GetHistoryWithId(&HSTRING::from(aumid)) else { return false };
        list.into_iter().any(|t| t.Tag().is_ok_and(|x| x == tag) && t.Group().is_ok_and(|g| g == GROUP))
    }

    /// Montre (ou remplace) la notification de cette étiquette.
    pub(super) fn show(aumid: &str, tag: &str, title: &str, body: &str) -> windows::core::Result<()> {
        init_winrt();
        let mut xml = format!("<toast><visual><binding template=\"ToastGeneric\"><text>{}</text>", escape(title));
        for line in body.lines().take(2) {
            let _ = write!(xml, "<text>{}</text>", escape(line));
        }
        xml.push_str("</binding></visual></toast>");
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        toast.SetTag(&HSTRING::from(tag))?;
        toast.SetGroup(&HSTRING::from(GROUP))?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(aumid))?.Show(&toast)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn p_etiquette_courte_et_stable() {
            let tag = super::tag_of("compte-très-long-0123456789-0123456789-0123456789-0123456789");
            assert_eq!(tag, super::tag_of("compte-très-long-0123456789-0123456789-0123456789-0123456789"), "stable");
            assert!(tag.len() <= 64, "{tag}");
            assert_ne!(super::tag_of("a"), super::tag_of("b"));
            assert_eq!(super::escape("<a & \"b\">"), "&lt;a &amp; &quot;b&quot;&gt;");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::notify_send_args;
    #[cfg(target_os = "linux")]
    use super::{replace_args, Shown};

    #[test]
    fn m1_notification_rattachee_au_lanceur() {
        let args = notify_send_args("Alice", "Bonjour", "mail-unread", "email.arrived", &[]);
        let sep = args.iter().position(|a| a == "--").expect("séparateur --");
        assert!(args[..sep].contains(&"--hint=string:desktop-entry:Vortex Mail".to_owned()), "indication desktop-entry absente : {args:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn m1_remplacement_ou_suivi() {
        assert_eq!(replace_args(Shown::Tracked), ["--print-id", "--wait"]);
        assert_eq!(replace_args(Shown::Replacing(42)), ["--replace-id=42"]);
        // Options avant `--` : le titre et le texte restent après lui.
        let args = notify_send_args("--wait", "x", "mail-unread", "email.arrived", &replace_args(Shown::Replacing(42)));
        let sep = args.iter().position(|a| a == "--").expect("séparateur --");
        assert!(args[..sep].contains(&"--replace-id=42".to_owned()), "{args:?}");
        assert_eq!(&args[sep + 1..], ["--wait", "x"]);
    }

    #[test]
    fn p_texte_d_un_message_puis_du_nombre() {
        let a = super::Arrivals { account: "a", added: 1, summary: "Alice", body: "Réunion\nPerso", account_name: "Perso" };
        assert_eq!(super::arrival_text(1, &a), ("Alice".to_owned(), "Réunion\nPerso".to_owned()));
        let (summary, body) = super::arrival_text(3, &a);
        assert!(summary.starts_with('3'), "{summary}");
        assert_eq!(body, "Perso");
    }

    #[test]
    fn arguments_de_notify_send() {
        let args = notify_send_args("--help", "<a href=\"https://x\">Vérifiez</a> & co", "mail-unread", "email.arrived", &[]);
        let sep = args.iter().position(|a| a == "--").expect("séparateur --");
        // Titre et texte après `--` : jamais lus comme des options.
        assert_eq!(&args[sep + 1..], ["--help", "&lt;a href=\"https://x\"&gt;Vérifiez&lt;/a&gt; &amp; co"]);
        assert!(args[..sep].iter().all(|a| a.starts_with("--")));
    }
}
