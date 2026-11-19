//! Nouvelle version publiée (lot K1, demande de Fabrice du 18 novembre 2026) :
//! la dernière version de GitHub est comparée à celle qui tourne, au plus une
//! fois par jour, si le réglage « Signaler les nouvelles versions » est actif.
//!
//! GitHub apprend l'adresse IP et l'heure, rien d'autre : la requête ne porte
//! ni compte, ni identifiant (voir `docs/MODELE-MENACE.md`). Rien n'est
//! téléchargé ni installé : l'interface propose seulement la page de la version.

use serde::Serialize;

use crate::{http, json};

/// Dernière version publiée (ni brouillon, ni préversion : GitHub les exclut).
const LATEST: &str = "https://api.github.com/repos/Vidub2001/Vortex-Mail/releases/latest";
/// Seules pages de version que l'interface peut ouvrir.
const RELEASES: &str = "https://github.com/Vidub2001/Vortex-Mail/releases/tag/";
/// La réponse fait quelques Kio (notes de version comprises).
const MAX_RESPONSE: usize = 512 * 1024;

/// Version plus récente que celle qui tourne, et sa page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NewVersion {
    pub version: String,
    pub url: String,
}

/// Interroge GitHub. `Ok(None)` : déjà à jour (ou aucune version publiée).
///
/// # Errors
///
/// Réseau, réponse refusée par GitHub (limite de requêtes), ou illisible.
pub(crate) fn check(current: &str) -> Result<Option<NewVersion>, String> {
    let response = http::get(LATEST, MAX_RESPONSE)?;
    match response.status {
        200 => newer_than(&String::from_utf8_lossy(&response.body), current),
        // Aucune version publiée.
        404 => Ok(None),
        status => Err(tr!("GitHub a répondu {status}", status = status)),
    }
}

/// Lit la réponse de GitHub et la compare à `current`.
fn newer_than(body: &str, current: &str) -> Result<Option<NewVersion>, String> {
    let release = json::parse(body)?;
    if release.get("draft").as_bool() || release.get("prerelease").as_bool() {
        return Ok(None);
    }
    let tag = release.get("tag_name").text();
    // Étiquette sûre seulement : elle finit dans une adresse ouverte dans le navigateur.
    if tag.is_empty() || tag.len() > 40 || !tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
        return Ok(None);
    }
    let version = tag.strip_prefix('v').unwrap_or(tag);
    match (parse(version), parse(current)) {
        (Some(published), Some(running)) if published > running => {
            Ok(Some(NewVersion { version: version.to_string(), url: format!("{RELEASES}{tag}") }))
        }
        _ => Ok(None),
    }
}

/// `1.2.3` → `(1, 2, 3)` ; une préversion (`1.2.3-rc.1`) ou autre chose : `None`.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|p| if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) { None } else { p.parse().ok() });
    let triple = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(triple)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> String {
        format!(r#"{{"tag_name": "{tag}", "draft": false, "prerelease": false, "html_url": "https://ailleurs.example/piege"}}"#)
    }

    #[test]
    fn k1_version_plus_recente_signalee() {
        let found = newer_than(&release("v0.3.5"), "0.3.4").unwrap();
        assert_eq!(
            found,
            Some(NewVersion { version: "0.3.5".into(), url: "https://github.com/Vidub2001/Vortex-Mail/releases/tag/v0.3.5".into() }),
            "page de GitHub, jamais l'adresse donnée par la réponse"
        );
        assert!(newer_than(&release("v0.10.0"), "0.9.9").unwrap().is_some(), "comparaison numérique, pas alphabétique");
        assert!(newer_than(&release("v1.0.0"), "0.3.4").unwrap().is_some());
    }

    #[test]
    fn k1_rien_si_deja_a_jour_ou_reponse_douteuse() {
        for (tag, current) in [("v0.3.4", "0.3.4"), ("v0.3.3", "0.3.4"), ("v0.4.0-rc.1", "0.3.4"), ("v0.4", "0.3.4"), ("", "0.3.4")] {
            assert_eq!(newer_than(&release(tag), current).unwrap(), None, "{tag} contre {current}");
        }
        assert_eq!(newer_than(&release(r#"v9.9.9\"onclick"#), "0.3.4").unwrap(), None, "étiquette piégée");
        let draft = r#"{"tag_name": "v9.0.0", "draft": true}"#;
        assert_eq!(newer_than(draft, "0.3.4").unwrap(), None);
        assert!(newer_than("pas du json", "0.3.4").is_err());
    }

    #[test]
    fn versions() {
        assert_eq!(parse("0.3.4"), Some((0, 3, 4)));
        assert_eq!(parse("10.20.30"), Some((10, 20, 30)));
        for bad in ["0.3", "0.3.4.5", "0.3.x", "0..4", "+1.2.3", "1.2.3-rc"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }
}
