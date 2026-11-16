//! Zone de notification (lot B3) : icône et menu de Vortex Mail ; fermer la
//! fenêtre principale la cache au lieu de quitter, si le réglage le demande
//! (par défaut : oui). L'écoute IMAP IDLE, les règles et les rappels, qui
//! tournent dans la fenêtre principale, continuent donc fenêtre fermée.
//!
//! Sous Linux, l'icône passe par `libayatana-appindicator3` ; le clic sur
//! l'icône y ouvre toujours le menu (limite du protocole), d'où l'entrée
//! « Ouvrir Vortex Mail ».

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuEvent, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Window, WindowEvent};

use crate::windows::MAIN;

/// Identifiant de l'icône (une seule).
const TRAY_ID: &str = "main";
/// Événement vers la fenêtre principale : « Nouveau message » choisi dans le menu.
const COMPOSE_EVENT: &str = "tray-compose";

/// Réglage « garder dans la zone de notification » (Paramètres › Général).
static KEEP_IN_TRAY: AtomicBool = AtomicBool::new(true);
/// Icône réellement affichée : sans elle, cacher la fenêtre rendrait
/// l'application invisible et impossible à quitter, donc fermer quitte.
static TRAY_SHOWN: AtomicBool = AtomicBool::new(false);

/// Réglage transmis par l'interface au démarrage et à chaque changement.
pub(crate) fn set_keep(keep: bool) {
    KEEP_IN_TRAY.store(keep, Ordering::Relaxed);
}

/// Crée l'icône ; en cas d'échec (bureau sans zone de notification), fermer
/// la fenêtre quittera l'application comme avant.
pub(crate) fn create(app: &AppHandle) {
    match build(app) {
        Ok(_) => TRAY_SHOWN.store(true, Ordering::Relaxed),
        Err(e) => crate::diag::report(&format_args!("zone de notification indisponible : {e}")),
    }
}

/// Menu de l'icône, dans la langue choisie (`tr!`).
fn menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let open = MenuItem::with_id(app, "open", tr!("Ouvrir Vortex Mail"), true, None::<&str>)?;
    let compose = MenuItem::with_id(app, "compose", tr!("Nouveau message"), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", tr!("Quitter"), true, None::<&str>)?;
    Menu::with_items(app, &[&open, &compose, &quit])
}

/// Langue changée (ou transmise par l'interface au démarrage, après la
/// création de l'icône) : menu refait dans cette langue.
pub(crate) fn relabel(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    if let Err(e) = menu(app).and_then(|m| tray.set_menu(Some(m))) {
        crate::diag::report(&e);
    }
}

fn build(app: &AppHandle) -> tauri::Result<TrayIcon> {
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Vortex Mail")
        .menu(&menu(app)?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| on_menu(app, &event))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)
}

fn on_menu(app: &AppHandle, event: &MenuEvent) {
    match event.id().as_ref() {
        "open" => show_main(app),
        "compose" => {
            show_main(app);
            if let Err(e) = app.emit_to(MAIN, COMPOSE_EVENT, ()) {
                crate::diag::report(&e);
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

/// Montre la fenêtre principale, cachée ou réduite, au premier plan.
pub(crate) fn show_main(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN) else { return };
    for step in [window.show(), window.unminimize(), window.set_focus()] {
        if let Err(e) = step {
            crate::diag::report(&e);
        }
    }
}

/// Fermeture de la fenêtre principale : cachée au lieu d'être fermée quand le
/// réglage le demande et que l'icône est là. Les autres fenêtres se ferment
/// normalement.
pub(crate) fn on_window_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else { return };
    if window.label() == MAIN && KEEP_IN_TRAY.load(Ordering::Relaxed) && TRAY_SHOWN.load(Ordering::Relaxed) {
        api.prevent_close();
        if let Err(e) = window.hide() {
            crate::diag::report(&e);
        }
    }
}
