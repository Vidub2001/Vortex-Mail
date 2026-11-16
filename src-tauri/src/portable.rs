//! Mode portable (pensé pour Windows) : un fichier `portable.txt` à côté de
//! l'exécutable, et toutes les données vont dans le dossier `data` voisin —
//! comptes, cache du courrier, stockage du moteur web — au lieu des dossiers
//! de l'utilisateur. Les mots de passe et jetons ne vont pas dans le trousseau
//! du système (lié au PC) mais dans un coffre chiffré par un mot de passe maître
//! (`data/secrets.vault`, voir `secrets.rs`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Fichiers dont la présence à côté de l'exécutable active le mode portable.
const MARKERS: [&str; 2] = ["portable.txt", "portable"];

/// Dossier des données en mode portable, `None` en installation normale.
pub(crate) fn data_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?;
        MARKERS.iter().any(|m| dir.join(m).is_file()).then(|| dir.join("data"))
    })
    .as_deref()
}

pub(crate) fn is_portable() -> bool {
    data_dir().is_some()
}

/// Stockage du moteur web (réglages de l'interface) en mode portable.
pub(crate) fn webview_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("webview"))
}
