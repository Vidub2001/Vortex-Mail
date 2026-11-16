//! Boîtes `mbox` et messages `.eml` (lot E8a).
//!
//! L'export écrit le format « mboxrd » : chaque message commence par une ligne
//! `From expéditeur date`, et toute ligne du message qui commence par `From `
//! (précédée ou non de `>`) reçoit un `>` de plus, retiré à la lecture. La
//! lecture accepte aussi « mboxo » (Thunderbird, anciens logiciels), dont
//! seules les lignes `From ` sont protégées.

use std::io::{self, Write};

const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Date de la ligne `From ` (format `asctime`, en UTC) : `Mon Nov 16 09:30:00 2026`.
fn asctime(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (year, month, day) = crate::compose::civil_from_days(days);
    let weekday = DAYS[usize::try_from(days.rem_euclid(7)).unwrap_or(0)];
    let month = MONTHS[usize::try_from(month - 1).unwrap_or(0)];
    format!("{weekday} {month} {day:2} {:02}:{:02}:{:02} {year}", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// Ligne `From ` à protéger : `From `, `>From `, `>>From `…
fn is_from_line(line: &[u8]) -> bool {
    let start = line.iter().position(|&b| b != b'>').unwrap_or(line.len());
    line.get(start..).is_some_and(|rest| rest.starts_with(b"From "))
}

/// Lignes du message, sans leur fin (`\n` ou `\r\n`) ; pas de ligne vide
/// finale fabriquée par le dernier retour à la ligne.
fn lines(raw: &[u8]) -> impl Iterator<Item = &[u8]> {
    let raw = raw.strip_suffix(b"\n").unwrap_or(raw);
    raw.split(|&b| b == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l))
}

/// Ajoute un message à une boîte mbox (mboxrd) : ligne `From `, lignes
/// `From ` du message protégées, fins de ligne `\n`, ligne vide de séparation.
/// `sender` est l'adresse de l'expéditeur (sans espace ; `MAILER-DAEMON` si
/// elle manque), `date` la date de réception en secondes depuis 1970.
///
/// # Errors
/// Erreur d'écriture de `out`.
pub fn write_message(out: &mut impl Write, raw: &[u8], sender: &str, date: i64) -> io::Result<()> {
    let sender: String = sender.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect();
    let sender = if sender.is_empty() { "MAILER-DAEMON" } else { sender.as_str() };
    writeln!(out, "From {sender} {}", asctime(date))?;
    for line in lines(raw) {
        if is_from_line(line) {
            out.write_all(b">")?;
        }
        out.write_all(line)?;
        out.write_all(b"\n")?;
    }
    out.write_all(b"\n")
}

/// Messages d'un fichier : une boîte mbox (mboxrd ou mboxo), ou un seul
/// message `.eml` si le fichier ne commence pas par `From `. Chaque message est
/// rendu avec des fins de ligne `\r\n`, comme l'attend IMAP (`APPEND`).
#[must_use]
pub fn split(data: &[u8]) -> Vec<Vec<u8>> {
    let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
    if !data.starts_with(b"From ") {
        let mut message = Vec::with_capacity(data.len() + data.len() / 40);
        for line in lines(data) {
            message.extend_from_slice(line);
            message.extend_from_slice(b"\r\n");
        }
        return if message.iter().all(u8::is_ascii_whitespace) { Vec::new() } else { vec![message] };
    }
    let mut messages: Vec<Vec<u8>> = Vec::new();
    let mut current: Option<Vec<u8>> = None;
    let mut previous_blank = true;
    for line in lines(data) {
        // Nouveau message : ligne `From ` en début de fichier ou après une ligne vide.
        if previous_blank && line.starts_with(b"From ") {
            messages.extend(current.take().map(finish));
            current = Some(Vec::new());
            previous_blank = false;
            continue;
        }
        previous_blank = line.is_empty();
        let Some(message) = current.as_mut() else { continue };
        // `>From ` → `From ` (mboxrd ; en mboxo, seule la forme `>From ` existe).
        let line = if is_from_line(line) && line.first() == Some(&b'>') { &line[1..] } else { line };
        message.extend_from_slice(line);
        message.extend_from_slice(b"\r\n");
    }
    messages.extend(current.map(finish));
    messages.retain(|m| !m.iter().all(u8::is_ascii_whitespace));
    messages
}

/// Message lu : sans la ligne vide qui le séparait du suivant.
fn finish(mut message: Vec<u8>) -> Vec<u8> {
    if message.ends_with(b"\r\n\r\n") {
        message.truncate(message.len() - 2);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &[u8] = b"From: Marie <marie@x.fr>\r\nSubject: Un\r\n\r\nBonjour,\r\nFrom here on, tout va bien.\r\n>From hier, rien.\r\n";
    const SECOND: &[u8] = b"From: Paul <paul@y.fr>\r\nSubject: Deux\r\n\r\nSalut\r\n";

    #[test]
    fn e8a_aller_et_retour() {
        let mut out = Vec::new();
        write_message(&mut out, FIRST, "marie@x.fr", 1_794_821_400).unwrap();
        write_message(&mut out, SECOND, "", 0).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.starts_with("From marie@x.fr Mon Nov 16 09:30:00 2026\nFrom: Marie"), "{text}");
        assert!(text.contains("\n>From here on"), "ligne From protégée : {text}");
        assert!(text.contains("\n>>From hier"), "mboxrd : un > de plus : {text}");
        assert!(text.contains("\n\nFrom MAILER-DAEMON Thu Jan  1 00:00:00 1970\n"), "{text}");
        assert!(!text.contains('\r'), "fins de ligne LF");
        assert_eq!(split(&out), vec![FIRST.to_vec(), SECOND.to_vec()], "relu tel quel");
    }

    #[test]
    fn e8a_mboxo_et_eml() {
        // mboxo (Thunderbird) : « From » d'un corps non protégé s'il ne suit pas une ligne vide.
        let mboxo = b"From - Mon Nov 16 09:00:00 2026\nSubject: A\n\nTexte\nFrom la cuisine\n\nFrom - Tue Nov 17 09:00:00 2026\nSubject: B\n\nFin\n";
        let messages = split(mboxo);
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert_eq!(messages[0], b"Subject: A\r\n\r\nTexte\r\nFrom la cuisine\r\n".to_vec());
        assert_eq!(messages[1], b"Subject: B\r\n\r\nFin\r\n".to_vec());

        // .eml : un seul message, fins de ligne CRLF, marque d'ordre des octets retirée.
        assert_eq!(split(b"\xEF\xBB\xBFSubject: C\n\nCorps\n"), vec![b"Subject: C\r\n\r\nCorps\r\n".to_vec()]);
        assert_eq!(split(SECOND), vec![SECOND.to_vec()], "déjà en CRLF : inchangé");
        assert_eq!(split(b" \r\n\n"), Vec::<Vec<u8>>::new(), "fichier vide");
        assert_eq!(split(b"From x Mon Nov 16 09:00:00 2026\n\n"), Vec::<Vec<u8>>::new(), "boîte vide");
    }
}
