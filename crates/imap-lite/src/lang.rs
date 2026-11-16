//! Langue des messages d'erreur de ce module (réglée par l'application).

use std::sync::atomic::{AtomicU8, Ordering};

static LANG: AtomicU8 = AtomicU8::new(0);

/// « fr » (par défaut), « en » ou « es ».
pub fn set_language(code: &str) {
    LANG.store(
        match code {
            "en" => 1,
            "es" => 2,
            _ => 0,
        },
        Ordering::Relaxed,
    );
}

/// Le texte de la langue courante.
pub(crate) fn pick<'a>(fr: &'a str, en: &'a str, es: &'a str) -> &'a str {
    match LANG.load(Ordering::Relaxed) {
        1 => en,
        2 => es,
        _ => fr,
    }
}
