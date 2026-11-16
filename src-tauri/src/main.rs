//! Point d'entrée de Vortex Mail : lance l'application (`vortexmail_lib::run`).

// Évite une console supplémentaire sous Windows en mode release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    vortexmail_lib::run();
}
