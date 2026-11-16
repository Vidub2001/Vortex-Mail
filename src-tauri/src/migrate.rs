//! Reprise des données des versions précédentes, nommées « `FluxMail` » :
//! dossiers de l'identifiant `fr.fluxmail.client` (comptes, cache du courrier,
//! réglages et stockage du moteur web) renommés en `fr.vortexmail.client`.
//! Fait au tout début du démarrage, avant que Tauri n'ouvre ces dossiers.

use std::path::{Path, PathBuf};

const OLD_ID: &str = "fr.fluxmail.client";
const NEW_ID: &str = "fr.vortexmail.client";

/// Dossiers de base où Tauri range les dossiers de l'application (config, données, cache).
fn base_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = env("HOME").or_else(|| env("USERPROFILE"));
    if cfg!(windows) {
        out.extend(env("APPDATA"));
        out.extend(env("LOCALAPPDATA"));
    } else if cfg!(target_os = "macos") {
        if let Some(h) = &home {
            out.push(h.join("Library/Application Support"));
            out.push(h.join("Library/Caches"));
            out.push(h.join("Library/WebKit"));
        }
    } else if let Some(h) = &home {
        out.push(env("XDG_CONFIG_HOME").unwrap_or_else(|| h.join(".config")));
        out.push(env("XDG_DATA_HOME").unwrap_or_else(|| h.join(".local/share")));
        out.push(env("XDG_CACHE_HOME").unwrap_or_else(|| h.join(".cache")));
    }
    out.sort();
    out.dedup();
    out
}

fn is_empty_dir(p: &Path) -> bool {
    std::fs::read_dir(p).is_ok_and(|mut d| d.next().is_none())
}

/// Renomme `old` en `new` si `new` n'existe pas encore (ou est vide). Jamais
/// d'écrasement : si les deux contiennent des données, l'ancien reste en place.
pub(crate) fn move_dir(old: &Path, new: &Path) -> Result<bool, String> {
    if !old.is_dir() {
        return Ok(false);
    }
    if new.exists() {
        if !is_empty_dir(new) {
            return Ok(false);
        }
        std::fs::remove_dir(new).map_err(|e| format!("{} : {e}", new.display()))?;
    }
    std::fs::rename(old, new).map_err(|e| format!("{} → {} : {e}", old.display(), new.display()))?;
    Ok(true)
}

/// Au démarrage : reprend les dossiers de l'ancien identifiant.
pub(crate) fn rename_data_dirs() {
    for base in base_dirs() {
        match move_dir(&base.join(OLD_ID), &base.join(NEW_ID)) {
            Ok(true) => crate::diag::report(&tr!("données reprises dans {path}", path = base.join(NEW_ID).display())),
            Ok(false) => {}
            Err(e) => crate::diag::report(&tr!("reprise des anciennes données impossible : {e}", e = e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_folder_is_renamed_never_overwritten() {
        let root = std::env::temp_dir().join(format!("vortex-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (old, new) = (root.join(OLD_ID), root.join(NEW_ID));
        std::fs::create_dir_all(old.join("localstorage")).unwrap();
        std::fs::write(old.join("cache.sqlite"), b"cache").unwrap();
        // Nouveau dossier vide (créé par une version neuve) : remplacé.
        std::fs::create_dir_all(&new).unwrap();
        assert!(move_dir(&old, &new).unwrap());
        assert!(!old.exists());
        assert_eq!(std::fs::read(new.join("cache.sqlite")).unwrap(), b"cache");
        assert!(new.join("localstorage").is_dir());
        // Plus d'ancien dossier : rien à faire.
        assert!(!move_dir(&old, &new).unwrap());
        // Les deux remplis : rien n'est touché.
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("cache.sqlite"), b"ancien").unwrap();
        assert!(!move_dir(&old, &new).unwrap());
        assert_eq!(std::fs::read(new.join("cache.sqlite")).unwrap(), b"cache");
        assert_eq!(std::fs::read(old.join("cache.sqlite")).unwrap(), b"ancien");
        let _ = std::fs::remove_dir_all(&root);
    }
}
