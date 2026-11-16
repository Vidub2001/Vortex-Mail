//! Journal de diagnostic, écrit sur la sortie d'erreur.
//!
//! Seul point d'écriture du journal (lot 10c) : les messages d'erreur viennent
//! souvent des serveurs (IMAP, SMTP, HTTP), donc de l'extérieur. Avant
//! d'atteindre le terminal, ils sont débarrassés des caractères de contrôle
//! (séquences d'échappement, retours à la ligne qui fabriqueraient une fausse
//! ligne de journal) et des caractères de mise en forme bidirectionnelle, puis
//! raccourcis.

use std::fmt::Display;
use std::sync::OnceLock;
use std::time::Instant;

/// Longueur maximale d'un message du journal, en caractères.
const MAX_CHARS: usize = 2_000;

/// Écrit `message` dans le journal, précédé de « Vortex Mail : ».
///
/// Accepte toute valeur affichable, empruntée : une erreur
/// (`diag::report(&e)`), un texte fixe ou construit sur place
/// (`diag::report(&format_args!("… {e}"))`).
///
/// Ne panique jamais, même sortie d'erreur fermée (application lancée sans
/// terminal) : elle est appelée depuis des destructeurs (lot E5).
pub(crate) fn report(message: &(impl Display + ?Sized)) {
    use std::io::Write as _;
    let line = format!("Vortex Mail : {}", clean(&message.to_string()));
    let _ignored = writeln!(std::io::stderr().lock(), "{line}");
}

/// Mesure du démarrage et des commandes (lot E2) : début de la mesure, si la
/// variable d'environnement `VORTEX_MESURE` est définie ; `None` sinon.
static MEASURE: OnceLock<Option<Instant>> = OnceLock::new();

/// Lance la mesure si `VORTEX_MESURE` est définie (au tout début de `run`).
pub(crate) fn start_measure() {
    MEASURE.get_or_init(|| std::env::var_os("VORTEX_MESURE").map(|_| Instant::now()));
}

/// Début de la mesure, si elle est active.
fn measure_origin() -> Option<Instant> {
    MEASURE.get().copied().flatten()
}

/// La mesure est-elle active ?
pub(crate) fn measuring() -> bool {
    measure_origin().is_some()
}

/// Étape de la mesure, avec le temps écoulé depuis le lancement (rien sans `VORTEX_MESURE`).
///
/// Appelée aussi depuis `Drop` ([`Timed`]) : une sortie d'erreur fermée est
/// ignorée, là où `eprintln!` paniquerait.
pub(crate) fn step(what: &(impl Display + ?Sized)) {
    use std::io::Write as _;
    if let Some(origin) = measure_origin() {
        let line = format!("Vortex Mail [{:>6} ms] {}", origin.elapsed().as_millis(), clean(&what.to_string()));
        let _ignored = writeln!(std::io::stderr().lock(), "{line}");
    }
}

/// Durée d'une commande, écrite quand le garde est détruit (rien sans `VORTEX_MESURE`).
#[derive(Debug)]
#[must_use = "la durée est écrite quand le garde est détruit"]
pub(crate) struct Timed {
    what: &'static str,
    since: Option<Instant>,
}

/// Mesure la durée de `what` jusqu'à la fin de la portée du garde rendu.
pub(crate) fn timed(what: &'static str) -> Timed {
    let since = measuring().then(Instant::now);
    if since.is_some() {
        step(&format_args!("{what} : début"));
    }
    Timed { what, since }
}

impl Drop for Timed {
    fn drop(&mut self) {
        if let Some(since) = self.since {
            step(&format_args!("{} : fin en {} ms", self.what, since.elapsed().as_millis()));
        }
    }
}

/// Remplace les caractères dangereux pour un terminal et coupe les messages trop longs.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_CHARS + 1));
    for (count, c) in text.chars().enumerate() {
        if count == MAX_CHARS {
            out.push('…');
            break;
        }
        out.push(if c.is_control() || is_layout_control(c) { '\u{FFFD}' } else { c });
    }
    out
}

/// Caractères qui changent la mise en page sans être des caractères de
/// contrôle : séparateurs de ligne et de paragraphe Unicode, et caractères qui
/// inversent l'ordre d'affichage (« Trojan Source », CVE-2021-42574).
fn is_layout_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caracteres_de_controle_remplaces() {
        assert_eq!(
            clean("NO [ALERT]\r\n\u{1b}[31mfaux\u{202E}txt\u{2028}fin"),
            "NO [ALERT]\u{FFFD}\u{FFFD}\u{FFFD}[31mfaux\u{FFFD}txt\u{FFFD}fin",
            "ni retour à la ligne, ni séquence d'échappement, ni inversion d'affichage"
        );
        assert_eq!(clean("Accès refusé : « INBOX »"), "Accès refusé : « INBOX »", "texte ordinaire inchangé");
    }

    #[test]
    fn messages_trop_longs_coupes() {
        let long = "é".repeat(MAX_CHARS + 10);
        let cut = clean(&long);
        assert_eq!(cut.chars().count(), MAX_CHARS + 1, "coupé à la limite, points de suspension compris");
        assert!(cut.ends_with('…'), "la coupure se voit");
        assert_eq!(clean(&"a".repeat(MAX_CHARS)), "a".repeat(MAX_CHARS), "pas de coupure à la limite exacte");
    }
}
