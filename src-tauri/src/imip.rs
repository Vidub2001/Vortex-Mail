//! Réponse à une invitation (lot H1) : message iTIP `REPLY` (RFC 5546 §3.2.3),
//! envoyé à l'organisateur par courrier (iMIP, RFC 6047).
//!
//! La réponse reprend le rendez-vous tel que l'organisateur l'a envoyé (horaire,
//! lieu, description, fuseaux), comme le font Google Agenda et Thunderbird :
//! Google Agenda ignore une réponse réduite aux seules propriétés exigées par
//! la RFC (essai du 15 novembre 2026). Seule la ligne `ATTENDEE` du compte y
//! figure, avec sa réponse (`PARTSTAT`) : jamais les autres invités, ni les
//! alarmes de l'invitation.

use std::fmt::Write as _;

use crate::pimfmt::{fold, parse_line, unfold, vevent_blocks, Line};

/// Longueur gardée d'un nom affiché (`CN`).
const MAX_NAME: usize = 200;

/// Réponse de l'invité.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartStat {
    Accepted,
    Tentative,
    Declined,
}

impl PartStat {
    /// Réponse choisie dans l'interface (`accepted`, `tentative`, `declined`).
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "accepted" => Some(Self::Accepted),
            "tentative" => Some(Self::Tentative),
            "declined" => Some(Self::Declined),
            _ => None,
        }
    }

    /// Nom de la réponse dans l'interface et dans le cache.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Tentative => "tentative",
            Self::Declined => "declined",
        }
    }

    /// Action de l'API d'Outlook.com qui donne cette réponse (lot H1b).
    pub(crate) fn graph_action(self) -> &'static str {
        match self {
            Self::Accepted => "accept",
            Self::Tentative => "tentativelyAccept",
            Self::Declined => "decline",
        }
    }

    /// Valeur iCalendar de `PARTSTAT`.
    pub(crate) fn ical(self) -> &'static str {
        match self {
            Self::Accepted => "ACCEPTED",
            Self::Tentative => "TENTATIVE",
            Self::Declined => "DECLINED",
        }
    }
}

/// Réponse prête à partir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reply {
    /// Calendrier `METHOD:REPLY`.
    pub ics: String,
    /// Adresse de l'organisateur, destinataire de la réponse.
    pub organizer: String,
    /// Titre du rendez-vous, pour l'objet du message.
    pub summary: String,
    /// Lieu du rendez-vous, pour le corps du message.
    pub location: String,
    /// `UID`, `RECURRENCE-ID` (vide pour la série) et `SEQUENCE` du rendez-vous
    /// auquel on répond : la réponse est gardée sous ces clés.
    pub uid: String,
    pub recurrence: String,
    pub sequence: i64,
    /// Invitation d'origine où la ligne `ATTENDEE` du compte porte la réponse,
    /// pour le rendez-vous gardé dans l'agenda (inchangée si le compte n'y est pas).
    pub updated: String,
}

/// Adresse d'une valeur `mailto:` (casse du préfixe indifférente).
fn mailto(value: &str) -> &str {
    let value = value.trim();
    match value.get(..7) {
        Some(prefix) if prefix.eq_ignore_ascii_case("mailto:") => value.get(7..).unwrap_or_default().trim(),
        _ => value,
    }
}

/// Lignes d'un bloc `VEVENT` hors de ses sous-blocs (`VALARM`…), telles quelles
/// (dépliées). Une alarme par courrier a aussi des lignes `ATTENDEE`.
fn top_level(block: &[String]) -> Vec<&str> {
    let mut depth: usize = 0;
    let mut out = Vec::new();
    for line in block.iter().skip(1) {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:") {
            depth += 1;
        } else if upper.starts_with("END:") {
            if depth == 0 {
                break;
            }
            depth -= 1;
        } else if depth == 0 {
            out.push(line.as_str());
        }
    }
    out
}

/// Ligne recopiée seulement si elle ne contient aucun caractère de contrôle.
fn clean(line: &str) -> Option<&str> {
    (!line.chars().any(char::is_control)).then_some(line)
}

/// Valeur de paramètre sans guillemets ni caractères de contrôle, entre guillemets.
fn quoted(value: &str) -> String {
    let kept: String = value.chars().filter(|c| !c.is_control() && *c != '"').take(MAX_NAME).collect();
    format!("\"{}\"", kept.trim())
}

/// Ligne `ATTENDEE` de `email` dans ces lignes, si le compte est invité.
fn attendee(lines: &[&str], email: &str) -> Option<Line> {
    lines.iter().filter_map(|raw| parse_line(raw)).find(|l| l.name == "ATTENDEE" && mailto(&l.value).eq_ignore_ascii_case(email))
}

/// Paramètres d'une ligne reformés, `PARTSTAT` et `RSVP` remplacés par la réponse.
fn with_answer(l: &Line, answer: PartStat) -> String {
    let mut out = String::from("ATTENDEE");
    for (k, v) in &l.params {
        if k != "PARTSTAT" && k != "RSVP" && !k.chars().any(char::is_control) {
            out.push(';');
            out.push_str(k);
            out.push('=');
            if v.contains([':', ';', ',']) || k == "CN" {
                out.push_str(&quoted(v));
            } else {
                out.extend(v.chars().filter(|c| !c.is_control() && *c != '"'));
            }
        }
    }
    out.push_str(";PARTSTAT=");
    out.push_str(answer.ical());
    out.push(':');
    out.extend(l.value.chars().filter(|c| !c.is_control()));
    out
}

/// Blocs `VTIMEZONE` du calendrier, recopiés tels quels.
fn timezones(raw: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in unfold(raw) {
        let upper = line.to_ascii_uppercase();
        if upper == "BEGIN:VTIMEZONE" {
            inside = true;
        }
        if inside {
            if let Some(line) = clean(&line) {
                out.push_str(&fold(line));
            }
        }
        if upper == "END:VTIMEZONE" {
            inside = false;
        }
    }
    out
}

/// L'invitation où les lignes `ATTENDEE` de `email` portent la réponse.
pub(crate) fn updated(raw: &str, email: &str, answer: PartStat) -> String {
    let mut out = String::with_capacity(raw.len() + 64);
    for line in unfold(raw) {
        match parse_line(&line) {
            Some(l) if l.name == "ATTENDEE" && mailto(&l.value).eq_ignore_ascii_case(email) => {
                out.push_str(&fold(&with_answer(&l, answer)));
            }
            _ => out.push_str(&fold(&line)),
        }
    }
    out
}

/// Réponse `answer` du compte `email` (nom affiché `name`) à l'invitation `raw`,
/// datée de `stamp` (`AAAAMMJJTHHMMSSZ`).
///
/// La réponse porte sur la série (bloc sans `RECURRENCE-ID`), sinon sur la
/// seule occurrence envoyée. Un compte absent de la liste des invités (invitation
/// reçue par une liste de diffusion) répond avec sa propre adresse.
///
/// # Errors
/// Invitation sans rendez-vous, ou sans organisateur à qui répondre.
pub(crate) fn reply(raw: &str, email: &str, name: &str, answer: PartStat, stamp: &str) -> Result<Reply, String> {
    let blocks = vevent_blocks(raw);
    let block = blocks
        .iter()
        .find(|b| top_level(b).iter().filter_map(|l| parse_line(l)).all(|l| l.name != "RECURRENCE-ID"))
        .or_else(|| blocks.first())
        .ok_or_else(|| tr!("Aucun rendez-vous dans ce fichier.").to_string())?;
    let lines = top_level(block);
    let find = |name: &str| lines.iter().copied().find(|l| parse_line(l).is_some_and(|p| p.name == name)).and_then(clean);

    let organizer_line = find("ORGANIZER").ok_or_else(|| tr!("Invitation sans organisateur : aucune réponse possible.").to_string())?;
    let organizer = parse_line(organizer_line).map_or_default(|l| mailto(&l.value).to_string());
    if !mail_core::compose::is_valid_email(&organizer) {
        return Err(tr!("Invitation sans organisateur : aucune réponse possible.").into());
    }
    // Sans `UID`, l'organisateur ne saurait pas à quoi se rapporte la réponse.
    find("UID").ok_or_else(|| tr!("Aucun rendez-vous dans ce fichier.").to_string())?;
    let summary = find("SUMMARY").and_then(parse_line).map_or_default(|l| crate::pimfmt::unescape(&l.value));

    let me = if let Some(l) = attendee(&lines, email) {
        with_answer(&l, answer)
    } else {
        let cn = if name.trim().is_empty() { String::new() } else { format!(";CN={}", quoted(name)) };
        format!("ATTENDEE{cn};PARTSTAT={}:mailto:{email}", answer.ical())
    };

    let mut ics = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Vortex Mail//FR\r\nMETHOD:REPLY\r\n");
    ics.push_str(&timezones(raw));
    ics.push_str("BEGIN:VEVENT\r\n");
    // Le rendez-vous tel quel, sans les autres invités ni l'ancien `DTSTAMP` ;
    // les sous-blocs (alarmes) sont déjà écartés par `top_level`.
    for line in lines.iter().copied().filter_map(clean) {
        let name = parse_line(line).map_or_default(|l| l.name);
        if name != "ATTENDEE" && name != "DTSTAMP" {
            ics.push_str(&fold(line));
        }
    }
    ics.push_str(&fold(&format!("DTSTAMP:{stamp}")));
    ics.push_str(&fold(&me));
    ics.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");

    let value = |name: &str| find(name).and_then(parse_line).map_or_default(|l| l.value.trim().to_string());
    let location = find("LOCATION").and_then(parse_line).map_or_default(|l| crate::pimfmt::unescape(&l.value));
    let sequence = value("SEQUENCE").parse::<i64>().unwrap_or(0).max(0);
    Ok(Reply {
        ics,
        organizer,
        summary,
        location,
        uid: value("UID"),
        recurrence: value("RECURRENCE-ID"),
        sequence,
        updated: updated(raw, email, answer),
    })
}

/// Objet et corps HTML du message de réponse (lot H1), comme ceux de Google
/// Agenda : la réponse en évidence, puis le titre, la date et le lieu du
/// rendez-vous. `who` : nom affiché, ou adresse du compte ; `when` : date et
/// heure mises en forme par l'interface, dans sa langue.
pub(crate) fn message(reply: &Reply, answer: PartStat, who: &str, when: &str) -> (String, String) {
    let title = if reply.summary.trim().is_empty() { tr!("(sans titre)").to_string() } else { reply.summary.clone() };
    let (subject, label, sentence, colors) = match answer {
        PartStat::Accepted => (
            tr!("Accepté : {title}", title = title),
            tr!("Accepté"),
            tr!("{name} a accepté votre invitation.", name = who),
            ("#dff6dd", "#107c10"),
        ),
        PartStat::Tentative => (
            tr!("Provisoire : {title}", title = title),
            tr!("Provisoire"),
            tr!("{name} a accepté provisoirement votre invitation.", name = who),
            ("#fff4ce", "#8a5a00"),
        ),
        PartStat::Declined => (
            tr!("Refusé : {title}", title = title),
            tr!("Refusé"),
            tr!("{name} a refusé votre invitation.", name = who),
            ("#fde7e9", "#a4262c"),
        ),
    };
    let esc = crate::xml::escape;
    let mut rows = String::new();
    for (name, value) in [(tr!("Rendez-vous"), title.as_str()), (tr!("Date"), when), (tr!("Lieu"), reply.location.as_str())] {
        let value = value.trim();
        if !value.is_empty() {
            let _infallible = write!(
                rows,
                "<tr><td style=\"color:#605e5c;padding:4px 16px 4px 0;vertical-align:top\">{}</td>\
                 <td style=\"padding:4px 0\">{}</td></tr>",
                esc(name),
                esc(value)
            );
        }
    }
    let (background, color) = colors;
    let html = format!(
        "<p style=\"margin:0 0 12px\"><span style=\"display:inline-block;padding:3px 12px;border-radius:12px;\
         background:{background};color:{color};font-weight:600\">{}</span></p>\
         <p style=\"margin:0 0 12px\">{}</p>\
         <table style=\"border-collapse:collapse\">{rows}</table>",
        esc(label),
        esc(&sentence)
    );
    (subject, html)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITATION: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\n\
        BEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
        BEGIN:VEVENT\r\nUID:reunion-42@exemple.fr\r\nSEQUENCE:2\r\nDTSTART;TZID=Europe/Paris:20261120T100000\r\n\
        SUMMARY:Point budget\\, T4\r\nORGANIZER;CN=Marie:mailto:marie@exemple.fr\r\n\
        ATTENDEE;CN=Paul;PARTSTAT=NEEDS-ACTION:mailto:paul@exemple.fr\r\n\
        ATTENDEE;CN=\"Joffre, Fabrice\";ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:Moi@Exemple.fr\r\n\
        BEGIN:VALARM\r\nACTION:EMAIL\r\nATTENDEE:mailto:moi@exemple.fr\r\nEND:VALARM\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn h1_reponse_acceptee() {
        let r = reply(INVITATION, "moi@exemple.fr", "Fabrice", PartStat::Accepted, "20261115T180000Z").unwrap();
        assert_eq!(r.organizer, "marie@exemple.fr", "organisateur");
        assert_eq!(r.summary, "Point budget, T4", "titre");
        assert_eq!((r.uid.as_str(), r.recurrence.as_str(), r.sequence), ("reunion-42@exemple.fr", "", 2), "clés de la réponse");
        let ics = r.ics.replace("\r\n ", "");
        assert!(ics.contains("METHOD:REPLY"), "{ics}");
        assert!(ics.contains("UID:reunion-42@exemple.fr"), "UID");
        assert!(ics.contains("SEQUENCE:2"), "SEQUENCE");
        assert!(ics.contains("DTSTAMP:20261115T180000Z"), "DTSTAMP");
        assert!(ics.contains("ORGANIZER;CN=Marie:mailto:marie@exemple.fr"), "organisateur recopié");
        assert!(ics.contains("ATTENDEE;CN=\"Joffre, Fabrice\";ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:Moi@Exemple.fr"), "{ics}");
        assert_eq!(ics.matches("ATTENDEE").count(), 1, "seulement le compte, pas les autres invités ni l'alarme");
        assert!(!ics.contains("RSVP"), "plus de demande de réponse");
        assert!(ics.contains("BEGIN:VTIMEZONE"), "fuseaux recopiés");
        assert!(ics.contains("DTSTART;TZID=Europe/Paris:20261120T100000"), "horaire repris, comme Google et Thunderbird");
        assert!(!ics.contains("ACTION:EMAIL"), "pas les alarmes de l'invitation");
        assert_eq!(ics.matches("DTSTAMP").count(), 1, "un seul DTSTAMP, le nôtre");
    }

    #[test]
    fn h1_message_mis_en_forme() {
        let r = reply(
            &INVITATION.replace("SUMMARY:", "LOCATION:Salle <B>\r\nSUMMARY:"),
            "moi@exemple.fr",
            "",
            PartStat::Accepted,
            "20261115T180000Z",
        )
        .unwrap();
        let (subject, html) = message(&r, PartStat::Accepted, "Fabrice <F>", "jeudi 20/11/2026 10:00 – 11:00");
        assert_eq!(subject, "Accepté : Point budget, T4", "objet");
        assert!(html.contains("Fabrice &lt;F&gt; a accepté votre invitation."), "nom échappé : {html}");
        assert!(html.contains("jeudi 20/11/2026 10:00 – 11:00"), "date");
        assert!(html.contains("Salle &lt;B&gt;"), "lieu échappé");
        let (_, empty) = message(&r, PartStat::Declined, "x", "");
        assert!(!empty.contains(">Date<"), "ligne vide omise");
    }

    #[test]
    fn h1_agenda_garde_la_reponse() {
        let r = reply(INVITATION, "moi@exemple.fr", "", PartStat::Declined, "20261115T180000Z").unwrap();
        let updated = r.updated.replace("\r\n ", "");
        assert!(updated.contains("PARTSTAT=DECLINED:mailto:Moi@Exemple.fr"), "{updated}");
        assert!(updated.contains("CN=Paul;PARTSTAT=NEEDS-ACTION:mailto:paul@exemple.fr"), "autre invité inchangé");
        assert!(updated.contains("DTSTART;TZID=Europe/Paris:20261120T100000"), "rendez-vous complet");
    }

    #[test]
    fn h1_compte_absent_de_la_liste() {
        let r = reply(INVITATION, "liste@exemple.fr", "Liste \"club\"\r\nBcc: x", PartStat::Tentative, "20261115T180000Z").unwrap();
        let ics = r.ics.replace("\r\n ", "");
        assert!(ics.contains("ATTENDEE;CN=\"Liste clubBcc: x\";PARTSTAT=TENTATIVE:mailto:liste@exemple.fr"), "{ics}");
        assert_eq!(r.updated, r.updated.replace("TENTATIVE", ""), "invitation inchangée");
    }

    #[test]
    fn h1_occurrence_et_refus_sans_organisateur() {
        let single = INVITATION.replace("SEQUENCE:2\r\n", "RECURRENCE-ID;TZID=Europe/Paris:20261127T100000\r\n");
        let r = reply(&single, "moi@exemple.fr", "", PartStat::Accepted, "20261115T180000Z").unwrap();
        assert!(r.ics.contains("RECURRENCE-ID;TZID=Europe/Paris:20261127T100000"), "occurrence seule");
        let orphan = INVITATION.replace("ORGANIZER;CN=Marie:mailto:marie@exemple.fr\r\n", "");
        assert!(reply(&orphan, "moi@exemple.fr", "", PartStat::Accepted, "20261115T180000Z").is_err(), "sans organisateur");
        let bad = INVITATION.replace("mailto:marie@exemple.fr", "mailto:pas une adresse");
        assert!(reply(&bad, "moi@exemple.fr", "", PartStat::Accepted, "20261115T180000Z").is_err(), "organisateur illisible");
        assert!(reply("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n", "a@b.fr", "", PartStat::Accepted, "x").is_err(), "vide");
    }
}
