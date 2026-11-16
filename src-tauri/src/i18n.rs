//! Langue des textes produits par l'application (messages d'erreur,
//! notifications, pages de retour du navigateur). Le français est la langue
//! source : chaque texte est sa propre clé, traduite dans `locales/en.rs` et
//! `locales/es.rs`. Un texte sans traduction reste en français.
//!
//! - `tr!("Texte")` : texte traduit (`&'static str`) ;
//! - `tr!("Connexion refusée ({t})", t = détail)` : modèle traduit puis rempli (`String`).

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[path = "locales/en.rs"]
mod en;
#[path = "locales/es.rs"]
mod es;

/// 0 français, 1 anglais, 2 espagnol (réglée par l'interface).
static LANG: AtomicU8 = AtomicU8::new(0);

pub(crate) fn set_language(code: &str) {
    let n = match code {
        "en" => 1,
        "es" => 2,
        _ => 0,
    };
    LANG.store(n, Ordering::Relaxed);
    mail_core::set_language(code);
    imap_lite::set_language(code);
}

/// Code de la langue courante.
pub(crate) fn lang() -> &'static str {
    match LANG.load(Ordering::Relaxed) {
        1 => "en",
        2 => "es",
        _ => "fr",
    }
}

fn table(entries: &'static [(&'static str, &'static str)]) -> HashMap<&'static str, &'static str> {
    entries.iter().copied().collect()
}

/// Texte traduit dans la langue courante (le texte français s'il n'a pas de traduction).
pub(crate) fn tr(fr: &'static str) -> &'static str {
    static EN: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    static ES: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let map = match LANG.load(Ordering::Relaxed) {
        1 => EN.get_or_init(|| table(en::EN)),
        2 => ES.get_or_init(|| table(es::ES)),
        _ => return fr,
    };
    map.get(fr).copied().unwrap_or(fr)
}

/// Remplit les `{nom}` d'un modèle.
pub(crate) fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('}') {
            let name = &after[..end];
            if let Some((_, v)) = args.iter().find(|(k, _)| *k == name) {
                out.push_str(&v.to_string());
            } else {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// `tr!("Texte")` ou `tr!("Modèle {x}", x = valeur)`.
#[macro_export]
macro_rules! tr {
    ($text:literal) => {
        $crate::i18n::tr($text)
    };
    ($text:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($text), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_filled() {
        assert_eq!(fill("Connexion refusée ({t}) {x}", &[("t", &"NO"), ("x", &3)]), "Connexion refusée (NO) 3");
        assert_eq!(fill("{inconnu} reste", &[]), "{inconnu} reste");
        assert_eq!(fill("accolade { seule", &[]), "accolade { seule");
        // Français par défaut : le texte source.
        assert_eq!(tr("Texte sans traduction"), "Texte sans traduction");
    }

    fn placeholders(s: &str) -> Vec<&str> {
        let mut out: Vec<&str> = s.match_indices('{').filter_map(|(i, _)| s[i + 1..].split_once('}').map(|(n, _)| n)).collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn translations_keep_their_placeholders() {
        for (table, name) in [(en::EN, "en"), (es::ES, "es")] {
            let mut seen = std::collections::HashSet::new();
            for (fr, text) in table {
                assert!(seen.insert(*fr), "{name} : clé en double « {fr} »");
                assert_eq!(placeholders(fr), placeholders(text), "{name} : « {fr} »");
            }
        }
    }
}
