//! Formats des contacts (vCard, RFC 6350) et des rendez-vous (iCalendar,
//! RFC 5545) : lecture des propriétés utiles et écriture.
//!
//! À l'écriture d'un élément venu d'un serveur, les propriétés que Vortex Mail
//! ne gère pas (photo, anniversaire, alarmes…) sont conservées telles quelles.

use serde::{Deserialize, Serialize};

// ================================================================ lignes de contenu

/// Une propriété : `NOM;PARAM=x:valeur`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub value: String,
}

impl Line {
    pub(crate) fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Plafonds de lecture, sur le modèle du lecteur iCalendar de `MailCraft` : un
/// fichier reçu (invitation, réponse d'un serveur) ne doit pas pouvoir
/// réclamer un temps ou une mémoire sans rapport avec un vrai agenda.
///
/// Ligne dépliée la plus longue lue (1 Mo) ; une ligne plus longue est
/// ignorée à la lecture, mais recopiée telle quelle à l'écriture.
const MAX_LINE: usize = 1 << 20;
/// Rendez-vous lus dans un même fichier (série et occurrences modifiées).
const MAX_COMPONENTS: usize = 20_000;
/// Dates exclues (`EXDATE`) lues pour un même rendez-vous.
const MAX_EXDATES: usize = 5_000;
/// Adresses ou numéros lus pour un même contact.
const MAX_VALUES: usize = 100;
/// Invités lus pour un même rendez-vous (lot H1).
const MAX_ATTENDEES: usize = 500;

/// Lignes dépliées, sans celles qui dépassent [`MAX_LINE`].
fn readable_lines(text: &str) -> impl Iterator<Item = Line> {
    unfold(text).into_iter().filter(|line| line.len() <= MAX_LINE).filter_map(|line| parse_line(&line))
}

/// Lignes « dépliées » (une ligne commençant par un espace prolonge la précédente).
pub(crate) fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if (line.starts_with(' ') || line.starts_with('\t')) && !out.is_empty() {
            out.last_mut().expect("ligne précédente").push_str(&line[1..]);
        } else if !line.is_empty() {
            out.push(line.to_string());
        }
    }
    out
}

pub(crate) fn parse_line(line: &str) -> Option<Line> {
    // Le `:` qui sépare la valeur est le premier hors guillemets.
    let mut quote = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quote = !quote,
            ':' if !quote => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim();
    // Groupe éventuel (`item1.EMAIL`) ignoré.
    let name = name.rsplit('.').next().unwrap_or(name).to_ascii_uppercase();
    let params = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.trim().to_ascii_uppercase(), v.trim().trim_matches('"').to_string()))
        })
        .collect();
    Some(Line { name, params, value: value.to_string() })
}

pub(crate) fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn escape(v: &str) -> String {
    v.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace("\r\n", "\\n").replace('\n', "\\n")
}

/// Ligne repliée à 75 octets, sans couper un caractère UTF-8.
pub(crate) fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in line.chars() {
        let len = c.len_utf8();
        if count + len > 75 {
            out.push_str("\r\n ");
            count = 1;
        }
        out.push(c);
        count += len;
    }
    out.push_str("\r\n");
    out
}

/// Lignes d'origine d'un bloc à conserver, hors propriétés réécrites.
fn kept_lines(raw: Option<&str>, block: &str, rewritten: &[&str]) -> Vec<String> {
    let Some(raw) = raw else { return Vec::new() };
    let mut depth = 0;
    let mut inside = false;
    let mut out = Vec::new();
    for line in unfold(raw) {
        let upper = line.to_ascii_uppercase();
        if upper == format!("BEGIN:{block}") && !inside {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if upper.starts_with("BEGIN:") {
            depth += 1;
        }
        if upper == format!("END:{block}") && depth == 0 {
            break;
        }
        if upper.starts_with("END:") {
            depth -= 1;
            out.push(line);
            continue;
        }
        if depth > 0 {
            out.push(line); // sous-bloc (alarme…) conservé
            continue;
        }
        let keep = parse_line(&line).is_some_and(|l| !rewritten.contains(&l.name.as_str()));
        if keep {
            out.push(line);
        }
    }
    out
}

// ================================================================ vCard

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Contact {
    /// Identifiant interne (source + emplacement).
    pub id: String,
    /// Compte (`imap…`) ou `local` (sur cet ordinateur).
    pub source: String,
    pub uid: String,
    /// Nom affiché (FN).
    pub name: String,
    /// Prénom et nom de famille (N), pour l'ordre d'affichage et le tri.
    pub given_name: String,
    pub family_name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub organization: String,
    pub note: String,
}

pub(crate) fn parse_vcard(text: &str) -> Option<Contact> {
    let mut c = Contact::default();
    let mut structured = String::new();
    let mut seen = false;
    for l in readable_lines(text) {
        match l.name.as_str() {
            "BEGIN" if l.value.eq_ignore_ascii_case("VCARD") => seen = true,
            "FN" => c.name = unescape(&l.value).trim().to_string(),
            "N" => {
                // Nom;Prénom;Autres;Civilité;Suffixe → « Prénom Nom ».
                let parts: Vec<String> = l.value.split(';').map(unescape).collect();
                c.given_name = parts.get(1).map_or_default(|s| s.trim().to_string());
                c.family_name = parts.first().map_or_default(|s| s.trim().to_string());
                structured = format!("{} {}", c.given_name, c.family_name).trim().to_string();
            }
            "EMAIL" => {
                let v = unescape(&l.value).trim().to_string();
                if !v.is_empty() && c.emails.len() < MAX_VALUES {
                    c.emails.push(v);
                }
            }
            "TEL" => {
                let v = unescape(&l.value).trim().trim_start_matches("tel:").to_string();
                if !v.is_empty() && c.phones.len() < MAX_VALUES {
                    c.phones.push(v);
                }
            }
            "ORG" => c.organization = l.value.split(';').map(unescape).find(|s| !s.is_empty()).unwrap_or_default(),
            "NOTE" => c.note = unescape(&l.value),
            "UID" => c.uid = l.value.trim().to_string(),
            _ => {}
        }
    }
    if !seen {
        return None;
    }
    if c.name.is_empty() {
        c.name = if structured.is_empty() { c.emails.first().cloned().unwrap_or_default() } else { structured };
    }
    Some(c)
}

/// Composants d'une valeur structurée (`N`, `ORG`), coupés aux `;` non
/// précédés d'une barre oblique inverse, encore échappés.
fn components(value: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut escaped = false;
    for ch in value.chars() {
        if ch == ';' && !escaped {
            out.push(String::new());
            continue;
        }
        escaped = ch == '\\' && !escaped;
        out.last_mut().expect("au moins un composant").push(ch);
    }
    out
}

/// Valeur comparable d'un `EMAIL` ou d'un `TEL` (sans `tel:` ni espaces autour).
fn vcard_value(l: &Line) -> String {
    unescape(&l.value).trim().trim_start_matches("tel:").trim().to_string()
}

/// Lignes d'origine d'une propriété (préfixe de groupe et paramètres compris),
/// avec leur valeur comparable.
fn originals(lines: &[String], name: &str) -> Vec<(String, String)> {
    lines.iter().filter_map(|raw| parse_line(raw).filter(|l| l.name == name).map(|l| (raw.clone(), vcard_value(&l)))).collect()
}

/// Lignes `EMAIL` ou `TEL` : celle d'origine pour chaque valeur inchangée
/// (type, préférence, groupe gardés), une ligne neuve pour une valeur ajoutée.
fn values_lines(values: &[String], mut before: Vec<(String, String)>, fresh: impl Fn(&str) -> String) -> Vec<String> {
    let mut out = Vec::new();
    for value in values.iter().map(String::as_str).map(str::trim).filter(|v| !v.is_empty()) {
        if let Some(i) = before.iter().position(|(_, old)| old.eq_ignore_ascii_case(value)) {
            out.push(before.remove(i).0);
        } else {
            out.push(fresh(value));
        }
    }
    out
}

/// vCard du contact ; `raw` : version d'origine. Relecture du 12 novembre
/// (n° 5) : la fiche d'origine sert de base. Sa version (3.0 ou 4.0) est
/// gardée ; les numéros et adresses inchangés gardent leur ligne (type
/// `CELL` ou `WORK`, `PREF`, groupe `item1.`) ; les composants de `N` et
/// d'`ORG` que l'interface ne montre pas sont gardés ; aucun prénom ni nom
/// n'est déduit du nom affiché (n° 43).
pub(crate) fn write_vcard(c: &Contact, raw: Option<&str>) -> String {
    let all = kept_lines(raw, "VCARD", &[]);
    let first = |name: &str| all.iter().find_map(|raw| parse_line(raw).filter(|l| l.name == name).map(|l| (raw.clone(), l)));
    let v4 = first("VERSION").is_some_and(|(_, l)| l.value.trim() == "4.0");
    let mut out = format!("BEGIN:VCARD\r\nVERSION:{}\r\n", if v4 { "4.0" } else { "3.0" });
    out.push_str(&fold(&format!("UID:{}", c.uid)));
    out.push_str(&fold(&format!("FN:{}", escape(&c.name))));

    let (given, family) = (c.given_name.trim(), c.family_name.trim());
    match first("N") {
        Some((line, n)) => {
            let mut parts = components(&n.value);
            parts.resize(parts.len().max(5), String::new());
            if unescape(&parts[0]).trim() == family && unescape(&parts[1]).trim() == given {
                out.push_str(&fold(&line));
            } else {
                parts[0] = escape(family);
                parts[1] = escape(given);
                out.push_str(&fold(&format!("N:{}", parts.join(";"))));
            }
        }
        // N est obligatoire en 3.0 (vide s'il n'y a ni prénom ni nom), facultatif en 4.0.
        None if !given.is_empty() || !family.is_empty() || !v4 => {
            out.push_str(&fold(&format!("N:{};{};;;", escape(family), escape(given))));
        }
        None => {}
    }
    let email = |v: &str| if v4 { format!("EMAIL:{v}") } else { format!("EMAIL;TYPE=INTERNET:{v}") };
    for line in values_lines(&c.emails, originals(&all, "EMAIL"), email) {
        out.push_str(&fold(&line));
    }
    for line in values_lines(&c.phones, originals(&all, "TEL"), |v| format!("TEL:{}", escape(v))) {
        out.push_str(&fold(&line));
    }
    let organization = c.organization.trim();
    match first("ORG") {
        Some((line, org)) if !organization.is_empty() => {
            let mut parts = components(&org.value);
            let shown = parts.iter().position(|p| !unescape(p).is_empty()).unwrap_or(0);
            if unescape(&parts[shown]) == organization {
                out.push_str(&fold(&line));
            } else {
                parts[shown] = escape(organization);
                out.push_str(&fold(&format!("ORG:{}", parts.join(";"))));
            }
        }
        None if !organization.is_empty() => out.push_str(&fold(&format!("ORG:{}", escape(organization)))),
        _ => {}
    }
    if !c.note.trim().is_empty() {
        out.push_str(&fold(&format!("NOTE:{}", escape(&c.note))));
    }
    let rewritten = ["VERSION", "UID", "FN", "N", "EMAIL", "TEL", "ORG", "NOTE", "REV", "PRODID"];
    for line in kept_lines(raw, "VCARD", &rewritten) {
        out.push_str(&fold(&line));
    }
    out.push_str("PRODID:-//Vortex Mail//FR\r\nEND:VCARD\r\n");
    out
}

// ================================================================ iCalendar

/// Date ou date-heure iCalendar, telle quelle : `20261103`, `20261103T140000`,
/// `20261103T130000Z`, avec son fuseau (`TZID`) éventuel. La conversion des
/// fuseaux et le calcul des répétitions se font dans l'interface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct When {
    pub value: String,
    pub tzid: String,
}

impl When {
    pub(crate) fn is_date(&self) -> bool {
        self.value.len() == 8
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Event {
    pub id: String,
    pub source: String,
    /// Agenda (collection) où l'événement est rangé.
    pub calendar: String,
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    pub start: When,
    pub end: When,
    /// `PT1H`… quand l'événement n'a pas de fin explicite.
    pub duration: String,
    pub all_day: bool,
    /// Règle de répétition brute (`FREQ=WEEKLY;BYDAY=MO`).
    pub rrule: String,
    pub exdates: Vec<String>,
    /// Occurrence modifiée d'une série (même `uid`).
    pub recurrence_id: String,
    pub organizer: String,
    /// Rappel : minutes avant le début (première alarme), `None` : aucun.
    pub reminder: Option<i64>,
    /// Numéro de version fixé par l'organisateur (`SEQUENCE`, RFC 5545 §3.8.7.4).
    pub sequence: i64,
    /// `STATUS` (`CONFIRMED`, `TENTATIVE`, `CANCELLED`), en majuscules.
    pub status: String,
    /// Invités (`ATTENDEE`), lus seulement : le rendez-vous enregistré garde
    /// ses lignes d'origine (lot H1).
    pub attendees: Vec<Attendee>,
    /// Adresse de l'organisateur (`organizer` porte son nom quand il en a un),
    /// pour savoir si le compte est l'organisateur ou un invité (lot H1b).
    pub organizer_email: String,
}

/// Invité d'un rendez-vous et sa réponse.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Attendee {
    pub email: String,
    /// Nom affiché (`CN`), vide s'il n'y en a pas.
    pub name: String,
    /// `PARTSTAT` en majuscules (`ACCEPTED`, `DECLINED`, `TENTATIVE`,
    /// `NEEDS-ACTION`…) ; `NEEDS-ACTION` s'il n'y en a pas (RFC 5545 §3.2.12).
    pub partstat: String,
}

fn when(l: &Line) -> When {
    When { value: l.value.trim().split(',').next().unwrap_or("").to_string(), tzid: l.param("TZID").unwrap_or("").to_string() }
}

/// Événements d'un calendrier (la série et ses occurrences modifiées).
pub(crate) fn parse_ical(text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut current: Option<Event> = None;
    // Sous-blocs (VALARM) dans un VEVENT ; un `END` en trop (« END:VTODO »
    // dans un VEVENT) ne le fait pas descendre sous zéro, sinon l'alarme qui
    // suit serait lue comme le rendez-vous (sa DESCRIPTION remplacerait la sienne).
    let mut depth: usize = 0;
    let mut in_alarm = false;
    for l in readable_lines(text) {
        match (l.name.as_str(), l.value.to_ascii_uppercase().as_str()) {
            ("BEGIN", "VEVENT") => {
                current = Some(Event::default());
                depth = 0;
                continue;
            }
            ("BEGIN", "VALARM") if current.is_some() && depth == 0 => {
                depth += 1;
                in_alarm = true;
                continue;
            }
            ("END", "VEVENT") => {
                if let Some(mut e) = current.take() {
                    e.all_day = e.start.is_date();
                    events.push(e);
                    if events.len() >= MAX_COMPONENTS {
                        break;
                    }
                }
                continue;
            }
            ("BEGIN", _) if current.is_some() => {
                depth += 1;
                continue;
            }
            ("END", _) if current.is_some() => {
                depth = depth.saturating_sub(1);
                in_alarm = false;
                continue;
            }
            _ => {}
        }
        let Some(e) = current.as_mut() else { continue };
        if in_alarm && depth == 1 && l.name == "TRIGGER" && e.reminder.is_none() {
            e.reminder = alarm_minutes(&l);
            continue;
        }
        if depth > 0 {
            continue;
        }
        match l.name.as_str() {
            "UID" => e.uid = l.value.trim().to_string(),
            "SUMMARY" => e.summary = unescape(&l.value),
            "LOCATION" => e.location = unescape(&l.value),
            "DESCRIPTION" => e.description = unescape(&l.value),
            "DTSTART" => e.start = when(&l),
            "DTEND" => e.end = when(&l),
            "DURATION" => e.duration = l.value.trim().to_string(),
            "RRULE" => e.rrule = l.value.trim().to_string(),
            "EXDATE" => {
                let room = MAX_EXDATES.saturating_sub(e.exdates.len());
                e.exdates.extend(l.value.split(',').map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).take(room));
            }
            "RECURRENCE-ID" => e.recurrence_id = l.value.trim().to_string(),
            "SEQUENCE" => e.sequence = l.value.trim().parse::<i64>().unwrap_or(0).max(0),
            "STATUS" => e.status = l.value.trim().to_ascii_uppercase(),
            "ATTENDEE" if e.attendees.len() < MAX_ATTENDEES => {
                let value = l.value.trim();
                let email =
                    value.get(..7).filter(|p| p.eq_ignore_ascii_case("mailto:")).map_or(value, |_| value.get(7..).unwrap_or_default());
                e.attendees.push(Attendee {
                    email: email.trim().to_string(),
                    name: l.param("CN").unwrap_or_default().to_string(),
                    partstat: l.param("PARTSTAT").unwrap_or("NEEDS-ACTION").trim().to_ascii_uppercase(),
                });
            }
            "ORGANIZER" => {
                let value = l.value.trim();
                e.organizer_email = value
                    .get(..7)
                    .filter(|p| p.eq_ignore_ascii_case("mailto:"))
                    .map_or(value, |_| value.get(7..).unwrap_or_default())
                    .trim()
                    .to_string();
                e.organizer = l
                    .param("CN")
                    .map_or_else(|| l.value.trim().trim_start_matches("mailto:").trim_start_matches("MAILTO:").to_string(), str::to_string);
            }
            _ => {}
        }
    }
    events
}

/// `METHOD` d'un calendrier reçu (`REQUEST`, `CANCEL`, `REPLY`…, RFC 5546),
/// en majuscules ; vide s'il n'y en a pas (simple publication).
pub(crate) fn calendar_method(text: &str) -> String {
    for l in readable_lines(text) {
        match l.name.as_str() {
            // Propriété du calendrier : avant ses composants.
            "BEGIN" if !l.value.trim().eq_ignore_ascii_case("VCALENDAR") => break,
            "METHOD" => return l.value.trim().to_ascii_uppercase(),
            _ => {}
        }
    }
    String::new()
}

/// Minutes avant le début d'un `TRIGGER` relatif au début (`-PT15M`, `-P1D`, `PT0S`).
/// Rappel le plus long accepté, en minutes (un an) : au-delà, la valeur est
/// aberrante et le rappel est ignoré, comme une valeur illisible.
const MAX_REMINDER_MINUTES: i64 = 366 * 24 * 60;

fn alarm_minutes(l: &Line) -> Option<i64> {
    if l.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME"))
        || l.param("RELATED").is_some_and(|v| v.eq_ignore_ascii_case("END"))
    {
        return None;
    }
    let v = l.value.trim();
    let (sign, rest) = match v.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, v.strip_prefix('+').unwrap_or(v)),
    };
    let rest = rest.strip_prefix('P')?;
    let (date, time) = rest.split_once('T').unwrap_or((rest, ""));
    let mut total = 0i64;
    let mut num = String::new();
    for (part, units) in [(date, [('W', 7 * 24 * 60), ('D', 24 * 60), ('\0', 0)]), (time, [('H', 60), ('M', 1), ('S', 0)])] {
        for ch in part.chars() {
            if ch.is_ascii_digit() {
                num.push(ch);
                continue;
            }
            let n: i64 = num.parse().ok()?;
            num.clear();
            let (_, factor) = units.iter().find(|(u, _)| *u == ch)?;
            // Calculs vérifiés : une invitation reçue peut annoncer
            // « -P9999999999999999W » (relecture S8).
            let minutes = if ch == 'S' { n / 60 } else { n.checked_mul(*factor)? };
            total = total.checked_add(minutes)?;
        }
    }
    if total > MAX_REMINDER_MINUTES {
        return None;
    }
    // Avant le début : déclencheur négatif ; un rappel « après » n'a pas de sens ici.
    Some((-sign * total).max(0))
}

/// Alarme écrite par Vortex Mail.
fn alarm_block(minutes: i64) -> String {
    let trigger = if minutes == 0 {
        "PT0S".to_string()
    } else if minutes % (7 * 24 * 60) == 0 {
        format!("-P{}W", minutes / (7 * 24 * 60))
    } else if minutes % (24 * 60) == 0 {
        format!("-P{}D", minutes / (24 * 60))
    } else if minutes % 60 == 0 {
        format!("-PT{}H", minutes / 60)
    } else {
        format!("-PT{minutes}M")
    };
    format!("BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Rappel\r\nTRIGGER:{trigger}\r\nEND:VALARM\r\n")
}

/// Lignes sans les blocs VALARM.
fn without_alarms(lines: Vec<String>) -> Vec<String> {
    let mut inside = false;
    lines
        .into_iter()
        .filter(|line| {
            let upper = line.to_ascii_uppercase();
            if upper == "BEGIN:VALARM" {
                inside = true;
                return false;
            }
            if upper == "END:VALARM" {
                inside = false;
                return false;
            }
            !inside
        })
        .collect()
}

fn write_when(name: &str, w: &When) -> String {
    if w.is_date() {
        fold(&format!("{name};VALUE=DATE:{}", w.value))
    } else if !w.tzid.is_empty() && !w.value.ends_with('Z') {
        fold(&format!("{name};TZID={}:{}", w.tzid, w.value))
    } else {
        fold(&format!("{name}:{}", w.value))
    }
}

/// Calendrier contenant l'événement ; `raw` : version d'origine (propriétés
/// inconnues, fuseaux et occurrences modifiées conservés). `stamp` : `DTSTAMP`
/// UTC (`AAAAMMJJTHHMMSSZ`).
///
/// - Série (`recurrence_id` vide) : la série est réécrite ; ses occurrences
///   modifiées sont recopiées telles quelles (relecture F2), sauf celles
///   supprimées par un `EXDATE` (F8) et sauf si le début de la série change
///   (leur `RECURRENCE-ID` ne désignerait plus aucune occurrence).
/// - Occurrence modifiée (`recurrence_id` rempli) : seule elle est réécrite ;
///   la série et les autres occurrences restent telles quelles.
/// - Chaque fuseau utilisé a son bloc `VTIMEZONE` : celui d'origine, sinon un
///   bloc calculé (relecture F1).
pub(crate) fn write_ical(e: &Event, raw: Option<&str>, stamp: &str) -> String {
    let ours: Vec<Vec<String>> = raw
        .map_or_default(vevent_blocks)
        .into_iter()
        .filter(|b| block_lines(b).any(|l| l.name == "UID" && l.value.trim() == e.uid))
        .collect();
    let master = ours.iter().find(|b| recurrence_of(b).is_none()).map(|b| b.join("\r\n"));
    let overrides: Vec<(When, &Vec<String>)> = ours.iter().filter_map(|b| recurrence_of(b).map(|rid| (rid, b))).collect();

    let mut events = String::new();
    if e.recurrence_id.is_empty() {
        events.push_str(&event_block(e, master.as_deref(), stamp));
        let master_start = master.as_deref().and_then(|m| parse_ical(m).into_iter().next()).map(|m| m.start);
        let keep = !e.rrule.is_empty() && master_start.is_some_and(|s| crate::tz::same_instant(&s, &e.start));
        for (rid, block) in overrides.iter().filter(|_| keep) {
            let deleted = e.exdates.iter().any(|x| crate::tz::same_instant(rid, &exdate_when(x, &e.start)));
            if !deleted {
                events.push_str(&verbatim(block));
            }
        }
    } else {
        let target = When { value: e.recurrence_id.clone(), tzid: e.start.tzid.clone() };
        let is_target = |rid: &When| rid.value.trim() == e.recurrence_id.trim() || crate::tz::same_instant(rid, &target);
        if let Some(block) = ours.iter().find(|b| recurrence_of(b).is_none()) {
            events.push_str(&verbatim(block));
        }
        let mut base = None;
        for (rid, block) in &overrides {
            if is_target(rid) {
                base = Some(block.join("\r\n"));
            } else {
                events.push_str(&verbatim(block));
            }
        }
        events.push_str(&event_block(e, base.as_deref(), stamp));
    }

    let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Vortex Mail//FR\r\n");
    // Fuseaux d'origine (VTIMEZONE) conservés : l'heure locale reste interprétable.
    let mut known = Vec::new();
    if let Some(raw) = raw {
        let mut inside = false;
        for line in unfold(raw) {
            let upper = line.to_ascii_uppercase();
            if upper == "BEGIN:VTIMEZONE" {
                inside = true;
            }
            if inside {
                if let Some(l) = parse_line(&line).filter(|l| l.name == "TZID") {
                    known.push(l.value.trim().to_string());
                }
                out.push_str(&fold(&line));
            }
            if upper == "END:VTIMEZONE" {
                inside = false;
            }
        }
    }
    // Fuseaux utilisés sans bloc : bloc calculé à partir de la base des fuseaux.
    let year = e.start.value.get(..4).and_then(|y| y.parse::<i16>().ok()).unwrap_or(2026);
    for line in unfold(&events) {
        let Some(l) = parse_line(&line) else { continue };
        if !matches!(l.name.as_str(), "DTSTART" | "DTEND" | "EXDATE" | "RDATE" | "RECURRENCE-ID") {
            continue;
        }
        let Some(tzid) = l.param("TZID").map(str::to_string) else { continue };
        if known.contains(&tzid) {
            continue;
        }
        if let Some(block) = crate::tz::vtimezone(&tzid, year) {
            out.push_str(&block);
        }
        known.push(tzid);
    }
    out.push_str(&events);
    out.push_str("END:VCALENDAR\r\n");
    out
}

/// Bloc VEVENT réécrit à partir de `e` ; `base` : bloc d'origine (propriétés
/// inconnues, participants, alarmes conservés).
fn event_block(e: &Event, base: Option<&str>, stamp: &str) -> String {
    let mut out = String::from("BEGIN:VEVENT\r\n");
    out.push_str(&fold(&format!("UID:{}", e.uid)));
    out.push_str(&fold(&format!("DTSTAMP:{stamp}")));
    if !e.recurrence_id.is_empty() {
        // RECURRENCE-ID d'origine (avec son fuseau) s'il existe.
        let original = base.and_then(|b| unfold(b).into_iter().find(|l| parse_line(l).is_some_and(|p| p.name == "RECURRENCE-ID")));
        let line = original.unwrap_or_else(|| {
            write_when("RECURRENCE-ID", &When { value: e.recurrence_id.clone(), tzid: e.start.tzid.clone() }).trim_end().to_string()
        });
        out.push_str(&fold(&line));
    }
    out.push_str(&write_when("DTSTART", &e.start));
    if !e.end.value.is_empty() {
        out.push_str(&write_when("DTEND", &e.end));
    } else if !e.duration.is_empty() {
        out.push_str(&fold(&format!("DURATION:{}", e.duration)));
    }
    out.push_str(&fold(&format!("SUMMARY:{}", escape(&e.summary))));
    if !e.location.is_empty() {
        out.push_str(&fold(&format!("LOCATION:{}", escape(&e.location))));
    }
    if !e.description.is_empty() {
        out.push_str(&fold(&format!("DESCRIPTION:{}", escape(&e.description))));
    }
    if !e.rrule.is_empty() {
        out.push_str(&fold(&format!("RRULE:{}", e.rrule)));
        for x in &e.exdates {
            let line = if x.len() == 8 {
                format!("EXDATE;VALUE=DATE:{x}")
            } else if !e.start.tzid.is_empty() && !x.ends_with('Z') {
                format!("EXDATE;TZID={}:{x}", e.start.tzid)
            } else {
                format!("EXDATE:{x}")
            };
            out.push_str(&fold(&line));
        }
    }
    let rewritten = [
        "UID",
        "DTSTAMP",
        "DTSTART",
        "DTEND",
        "DURATION",
        "SUMMARY",
        "LOCATION",
        "DESCRIPTION",
        "RRULE",
        "EXDATE",
        "RECURRENCE-ID",
        "SEQUENCE",
        "LAST-MODIFIED",
    ];
    // Version : gardée, et augmentée quand la date, le lieu ou la répétition
    // changent, pour que les logiciels des invités prennent la modification
    // (RFC 5546 §2.1.4 ; relecture du 12 novembre, n° 7).
    let previous = base.and_then(|b| parse_ical(b).into_iter().next());
    let mut sequence = e.sequence.max(previous.as_ref().map_or(0, |p| p.sequence));
    if previous.as_ref().is_some_and(|p| significant_change(p, e)) {
        sequence = sequence.saturating_add(1);
    }
    if sequence > 0 {
        out.push_str(&fold(&format!("SEQUENCE:{sequence}")));
    }
    // Alarmes d'origine remplacées seulement si le rappel a changé.
    let before = previous.as_ref().and_then(|m| m.reminder);
    let mut kept = kept_lines(base, "VEVENT", &rewritten);
    let rewrite_alarm = base.is_none() || before != e.reminder;
    if rewrite_alarm {
        kept = without_alarms(kept);
    }
    for line in kept {
        out.push_str(&fold(&line));
    }
    if rewrite_alarm {
        if let Some(minutes) = e.reminder {
            out.push_str(&alarm_block(minutes.max(0)));
        }
    }
    out.push_str(&fold(&format!("LAST-MODIFIED:{stamp}")));
    out.push_str("END:VEVENT\r\n");
    out
}

/// Changement qui demande une nouvelle version (`SEQUENCE`) : horaire,
/// répétition, dates exclues, lieu, statut.
fn significant_change(before: &Event, after: &Event) -> bool {
    let same = |a: &When, b: &When| (a.value.is_empty() && b.value.is_empty()) || crate::tz::same_instant(a, b);
    !same(&before.start, &after.start)
        || !same(&before.end, &after.end)
        || before.duration != after.duration
        || before.all_day != after.all_day
        || before.rrule != after.rrule
        || before.exdates != after.exdates
        || before.location != after.location
        || (!after.status.is_empty() && before.status != after.status)
}

/// Valeur d'un `EXDATE` avec le fuseau de la série.
fn exdate_when(value: &str, start: &When) -> When {
    let floating = !value.ends_with('Z') && value.len() > 8;
    When { value: value.to_string(), tzid: if floating { start.tzid.clone() } else { String::new() } }
}

/// Bloc recopié tel quel.
fn verbatim(block: &[String]) -> String {
    block.iter().map(String::as_str).map(fold).collect()
}

fn block_lines(block: &[String]) -> impl Iterator<Item = Line> + '_ {
    block.iter().filter_map(|l| parse_line(l))
}

/// `RECURRENCE-ID` d'un bloc VEVENT (hors sous-blocs), s'il en a un.
fn recurrence_of(block: &[String]) -> Option<When> {
    block_lines(block).find(|l| l.name == "RECURRENCE-ID").map(|l| when(&l))
}

/// Blocs VEVENT d'un calendrier, lignes dépliées.
pub(crate) fn vevent_blocks(raw: &str) -> Vec<Vec<String>> {
    component_blocks(raw, "VEVENT")
}

/// Blocs `BEGIN:<name>` … `END:<name>` de premier niveau pour ce nom (`VEVENT`,
/// `VTIMEZONE`, `VCARD`), sous-blocs compris, lignes dépliées.
pub(crate) fn component_blocks(raw: &str, name: &str) -> Vec<Vec<String>> {
    let (begin, end) = (format!("BEGIN:{name}"), format!("END:{name}"));
    let mut blocks: Vec<Vec<String>> = Vec::new();
    let mut current: Option<Vec<String>> = None;
    let mut depth: usize = 0;
    for line in unfold(raw) {
        let upper = line.to_ascii_uppercase();
        if upper == begin && current.is_none() {
            current = Some(vec![line]);
            depth = 0;
            continue;
        }
        if let Some(block) = current.as_mut() {
            if upper.starts_with("BEGIN:") {
                depth += 1;
            }
            if upper == end && depth == 0 {
                block.push(line);
                blocks.push(current.take().expect("bloc"));
                continue;
            }
            if upper.starts_with("END:") {
                depth = depth.saturating_sub(1);
            }
            block.push(line);
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_is_capped() {
        let one = "BEGIN:VEVENT\r\nUID:u\r\nDTSTART:20261102T090000Z\r\nEND:VEVENT\r\n";
        let many = format!("BEGIN:VCALENDAR\r\n{}END:VCALENDAR\r\n", one.repeat(MAX_COMPONENTS + 5));
        assert_eq!(parse_ical(&many).len(), MAX_COMPONENTS);

        let dates = vec!["20261102T090000Z"; MAX_EXDATES + 10].join(",");
        let raw = format!("BEGIN:VEVENT\r\nUID:u\r\nEXDATE:{dates}\r\nEXDATE:20261103T090000Z\r\nEND:VEVENT\r\n");
        assert_eq!(parse_ical(&raw)[0].exdates.len(), MAX_EXDATES);

        // Ligne de plus d'1 Mo : ignorée, le reste du rendez-vous est lu.
        let long = format!("BEGIN:VEVENT\r\nSUMMARY:{}\r\nUID:u\r\nEND:VEVENT\r\n", "x".repeat(MAX_LINE));
        let e = &parse_ical(&long)[0];
        assert_eq!((e.summary.as_str(), e.uid.as_str()), ("", "u"));

        let emails = "EMAIL:a@example.com\r\n".repeat(MAX_VALUES + 3);
        let c = parse_vcard(&format!("BEGIN:VCARD\r\n{emails}END:VCARD\r\n")).unwrap();
        assert_eq!(c.emails.len(), MAX_VALUES);
    }

    #[test]
    fn stray_end_does_not_turn_the_alarm_into_the_event() {
        let raw = "BEGIN:VEVENT\r\nUID:u\r\nDESCRIPTION:vraie\r\nEND:VTODO\r\nBEGIN:VALARM\r\nTRIGGER:-PT10M\r\n\
                   DESCRIPTION:alarme\r\nEND:VALARM\r\nSUMMARY:titre\r\nEND:VEVENT\r\n";
        let e = &parse_ical(raw)[0];
        assert_eq!(e.description, "vraie");
        assert_eq!(e.summary, "titre");
        assert_eq!(e.reminder, Some(10));
        assert_eq!(vevent_blocks(raw).len(), 1);
    }

    #[test]
    fn vcard_roundtrip_keeps_unknown_properties() {
        let raw = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:abc\r\nFN:Marie Dupont\r\nN:Dupont;Marie;;;\r\n\
                   EMAIL;TYPE=WORK:marie@example.com\r\nTEL;TYPE=CELL:+33 6 00 00 00 00\r\nORG:ACME\\, SA;Ventes\r\n\
                   NOTE:Ligne 1\\nLigne 2\r\nBDAY:1980-01-02\r\nPHOTO;ENCODING=b;TYPE=JPEG:QUJD\r\n DEF\r\nEND:VCARD\r\n";
        let c = parse_vcard(raw).unwrap();
        assert_eq!(c.name, "Marie Dupont");
        assert_eq!(c.emails, vec!["marie@example.com"]);
        assert_eq!(c.phones, vec!["+33 6 00 00 00 00"]);
        assert_eq!(c.organization, "ACME, SA");
        assert_eq!(c.note, "Ligne 1\nLigne 2");
        let mut edited = c;
        edited.emails.push("m.dupont@example.org".into());
        let out = write_vcard(&edited, Some(raw));
        assert!(out.contains("BDAY:1980-01-02"));
        assert!(out.contains("PHOTO;ENCODING=b;TYPE=JPEG:QUJDDEF"));
        let back = parse_vcard(&out).unwrap();
        assert_eq!(back.emails.len(), 2);
        assert_eq!(back.organization, "ACME, SA");
        assert!(out.lines().all(|l| l.len() <= 76));
    }

    #[test]
    fn vcard_name_from_structured_name() {
        let c = parse_vcard("BEGIN:VCARD\nN:Martin;Paul;;;\nEMAIL:p@x.fr\nEND:VCARD").unwrap();
        assert_eq!(c.name, "Paul Martin");
        assert_eq!((c.given_name.as_str(), c.family_name.as_str()), ("Paul", "Martin"));
    }

    #[test]
    fn vcard_writes_given_and_family_names() {
        let c = Contact {
            uid: "u".into(),
            name: "Jean-Pierre de la Tour".into(),
            given_name: "Jean-Pierre".into(),
            family_name: "de la Tour".into(),
            ..Default::default()
        };
        assert!(write_vcard(&c, None).contains("N:de la Tour;Jean-Pierre;;;"));
        // Sans prénom ni nom : rien n'est déduit du nom affiché (n° 43), N vide en 3.0.
        let plain = Contact { uid: "u".into(), name: "Mairie de Lyon".into(), ..Default::default() };
        let out = write_vcard(&plain, None);
        assert!(out.contains("FN:Mairie de Lyon\r\nN:;;;;\r\n"), "{out}");
    }

    #[test]
    fn vcard_garde_types_composants_et_version() {
        // Relecture du 12 novembre, n° 5 : fiche venue d'un téléphone, en 4.0.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:k\r\nFN:Dr Jean Dupont\r\nN:Dupont;Jean;Pierre;Dr;Jr\r\n\
                   TEL;TYPE=cell;PREF=1:+33 6 11\r\nTEL;VALUE=uri;TYPE=work:tel:+33-1-22\r\n\
                   item1.EMAIL;TYPE=home:j@x.fr\r\nitem1.X-ABLabel:Perso\r\nORG:ACME;Ventes\r\n\
                   PHOTO:data:image/png;base64,AAAA\r\nEND:VCARD\r\n";
        let mut c = parse_vcard(raw).unwrap();
        assert_eq!(c.phones, vec!["+33 6 11", "+33-1-22"]);
        c.note = "corrigée".into();
        c.phones.push("04 05".into());
        let out = write_vcard(&c, Some(raw));
        for kept in [
            "VERSION:4.0",
            "N:Dupont;Jean;Pierre;Dr;Jr",
            "TEL;TYPE=cell;PREF=1:+33 6 11",
            "TEL;VALUE=uri;TYPE=work:tel:+33-1-22",
            "item1.EMAIL;TYPE=home:j@x.fr",
            "item1.X-ABLabel:Perso",
            "ORG:ACME;Ventes",
            "TEL:04 05",
            "NOTE:corrigée",
        ] {
            assert!(out.contains(&format!("{kept}\r\n")), "{kept} manque :\n{out}");
        }
        assert!(!out.contains("VERSION:3.0"));
        assert_eq!(out.matches("TEL").count(), 3);
        // Prénom et service changés : les autres composants restent.
        c.given_name = "Paul".into();
        c.organization = "ACME SA".into();
        let out = write_vcard(&c, Some(raw));
        assert!(out.contains("N:Dupont;Paul;Pierre;Dr;Jr\r\n"), "{out}");
        assert!(out.contains("ORG:ACME SA;Ventes\r\n"), "{out}");
        // Numéro retiré : sa ligne disparaît.
        c.phones.remove(0);
        assert!(!write_vcard(&c, Some(raw)).contains("+33 6 11"));
    }

    #[test]
    fn ical_series_and_override() {
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\nEND:VTIMEZONE\r\n\
                   BEGIN:VEVENT\r\nUID:r1\r\nDTSTART;TZID=Europe/Paris:20261103T140000\r\n\
                   DTEND;TZID=Europe/Paris:20261103T150000\r\nRRULE:FREQ=WEEKLY;BYDAY=TU\r\n\
                   EXDATE;TZID=Europe/Paris:20261110T140000\r\nSUMMARY:Point d'équipe\r\nATTENDEE:mailto:a@x.fr\r\n\
                   BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:r1\r\nRECURRENCE-ID;TZID=Europe/Paris:20261117T140000\r\n\
                   DTSTART;TZID=Europe/Paris:20261117T160000\r\nDTEND;TZID=Europe/Paris:20261117T170000\r\n\
                   SUMMARY:Point d'équipe (décalé)\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:d1\r\nDTSTART;VALUE=DATE:20261111\r\nDTEND;VALUE=DATE:20261112\r\n\
                   SUMMARY:Férié\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let events = parse_ical(raw);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].start, When { value: "20261103T140000".into(), tzid: "Europe/Paris".into() });
        assert_eq!(events[0].rrule, "FREQ=WEEKLY;BYDAY=TU");
        assert_eq!(events[0].exdates, vec!["20261110T140000"]);
        assert_eq!(events[1].recurrence_id, "20261117T140000");
        assert!(events[2].all_day);

        let mut e = events[0].clone();
        e.summary = "Point hebdo".into();
        let out = write_ical(&e, Some(raw), "20261103T120000Z");
        assert!(out.contains("BEGIN:VTIMEZONE"));
        assert!(out.contains("ATTENDEE:mailto:a@x.fr"));
        assert!(out.contains("TRIGGER:-PT15M"));
        assert!(out.contains("EXDATE;TZID=Europe/Paris:20261110T140000"));
        // Occurrence modifiée recopiée (relecture F2), autre événement du fichier non.
        assert!(out.contains("décalé"), "occurrence modifiée perdue :\n{out}");
        assert!(!out.contains("Férié"));
        let back = parse_ical(&out);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].summary, "Point hebdo");
        assert_eq!(back[0].start.tzid, "Europe/Paris");
        assert_eq!(back[0].reminder, Some(15));
        assert_eq!(back[1].recurrence_id, "20261117T140000");
        // Un seul bloc VTIMEZONE pour Europe/Paris : celui d'origine.
        assert_eq!(out.matches("BEGIN:VTIMEZONE").count(), 1);
    }

    #[test]
    fn occurrences_modifiees_et_series() {
        let raw = "BEGIN:VCALENDAR\r\n\
                   BEGIN:VEVENT\r\nUID:r1\r\nDTSTART;TZID=Europe/Paris:20261103T140000\r\n\
                   DTEND;TZID=Europe/Paris:20261103T150000\r\nRRULE:FREQ=WEEKLY;BYDAY=TU\r\nSUMMARY:Point\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:r1\r\nRECURRENCE-ID;TZID=Europe/Paris:20261117T140000\r\n\
                   DTSTART;TZID=Europe/Paris:20261118T100000\r\nDTEND;TZID=Europe/Paris:20261118T110000\r\n\
                   SUMMARY:Point (mercredi)\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let events = parse_ical(raw);
        let master = events[0].clone();

        // Supprimer l'occurrence déplacée (F8) : EXDATE à sa date d'origine, l'occurrence modifiée disparaît.
        let mut e = master.clone();
        e.exdates.push("20261117T140000".into());
        let out = write_ical(&e, Some(raw), "20261103T120000Z");
        assert!(out.contains("EXDATE;TZID=Europe/Paris:20261117T140000"));
        assert!(!out.contains("mercredi"), "{out}");

        // Début de la série déplacé : les occurrences modifiées ne correspondent plus, abandonnées.
        let mut e = master.clone();
        e.start.value = "20261103T150000".into();
        e.end.value = "20261103T160000".into();
        assert!(!write_ical(&e, Some(raw), "20261103T120000Z").contains("mercredi"));

        // Même instant écrit autrement (UTC → heure de Paris) : conservées.
        let utc_raw = raw.replace("DTSTART;TZID=Europe/Paris:20261103T140000", "DTSTART:20261103T130000Z");
        let out = write_ical(&master, Some(&utc_raw), "20261103T120000Z");
        assert!(out.contains("mercredi"), "{out}");

        // Modifier l'occurrence seule : la série reste intacte, l'occurrence est réécrite.
        let mut o = events[1].clone();
        o.summary = "Point (jeudi)".into();
        o.start.value = "20261119T100000".into();
        o.end.value = "20261119T110000".into();
        let out = write_ical(&o, Some(raw), "20261103T120000Z");
        let back = parse_ical(&out);
        assert_eq!(back.len(), 2, "{out}");
        assert_eq!((back[0].summary.as_str(), back[0].rrule.as_str()), ("Point", "FREQ=WEEKLY;BYDAY=TU"));
        assert_eq!((back[1].summary.as_str(), back[1].recurrence_id.as_str()), ("Point (jeudi)", "20261117T140000"));
        assert!(out.contains("RECURRENCE-ID;TZID=Europe/Paris:20261117T140000"));
    }

    #[test]
    fn fuseau_ajoute_aux_nouveaux_rendez_vous() {
        let e = Event {
            uid: "n1".into(),
            summary: "Réunion".into(),
            start: When { value: "20261109T090000".into(), tzid: "Europe/Paris".into() },
            end: When { value: "20261109T100000".into(), tzid: "Europe/Paris".into() },
            rrule: "FREQ=WEEKLY".into(),
            ..Event::default()
        };
        let out = write_ical(&e, None, "20261109T080000Z");
        assert!(out.contains("BEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\n"), "{out}");
        assert!(out.contains("DTSTART;TZID=Europe/Paris:20261109T090000"));
        assert_eq!(out.matches("BEGIN:VTIMEZONE").count(), 1);
        // Le bloc VTIMEZONE précède les événements.
        assert!(out.find("BEGIN:VTIMEZONE") < out.find("BEGIN:VEVENT"));
    }

    #[test]
    fn h1_invites_lus() {
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:i\r\nDTSTART:20261103T140000Z\r\n\
                   ATTENDEE;CN=Paul;PARTSTAT=accepted:MAILTO:paul@exemple.fr\r\n\
                   ATTENDEE:mailto:marie@exemple.fr\r\n\
                   BEGIN:VALARM\r\nACTION:EMAIL\r\nATTENDEE:mailto:alarme@exemple.fr\r\nEND:VALARM\r\n\
                   END:VEVENT\r\nEND:VCALENDAR\r\n";
        let e = parse_ical(raw).remove(0);
        assert_eq!(
            e.attendees,
            vec![
                Attendee { email: "paul@exemple.fr".into(), name: "Paul".into(), partstat: "ACCEPTED".into() },
                Attendee { email: "marie@exemple.fr".into(), name: String::new(), partstat: "NEEDS-ACTION".into() },
            ],
            "invités du rendez-vous, sans celui de l'alarme"
        );
    }

    #[test]
    fn methode_version_et_statut() {
        // Relecture du 12 novembre, n° 7.
        let raw = "BEGIN:VCALENDAR\r\nMETHOD:cancel\r\nBEGIN:VEVENT\r\nUID:m\r\nSEQUENCE:3\r\nSTATUS:cancelled\r\n\
                   DTSTART:20261103T140000Z\r\nLOCATION:Salle A\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(calendar_method(raw), "CANCEL");
        assert_eq!(calendar_method("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nMETHOD:REPLY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"), "");
        let e = parse_ical(raw).remove(0);
        assert_eq!((e.sequence, e.status.as_str()), (3, "CANCELLED"));
        // Titre seul changé : même version ; lieu changé : version suivante.
        let mut edited = e;
        edited.summary = "Point".into();
        assert_eq!(parse_ical(&write_ical(&edited, Some(raw), "20261103T120000Z"))[0].sequence, 3);
        edited.location = "Salle B".into();
        let out = write_ical(&edited, Some(raw), "20261103T120000Z");
        assert_eq!(parse_ical(&out)[0].sequence, 4);
        assert_eq!(out.matches("SEQUENCE").count(), 1);
        // Nouveau rendez-vous : pas de SEQUENCE.
        let fresh = Event { uid: "n".into(), ..Default::default() };
        assert!(!write_ical(&fresh, None, "20261103T120000Z").contains("SEQUENCE"));
    }

    #[test]
    fn ical_reminders() {
        let ev = |trigger: &str| {
            let raw = format!(
                "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261103T140000Z\r\n\
                 BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER{trigger}\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
            );
            parse_ical(&raw)[0].reminder
        };
        assert_eq!(ev(":-PT15M"), Some(15));
        assert_eq!(ev(":-PT1H30M"), Some(90));
        assert_eq!(ev(";RELATED=START:-P1D"), Some(1440));
        assert_eq!(ev(":-P1W"), Some(10080));
        // Relecture S8 : plus de débordement, un rappel aberrant est ignoré.
        assert_eq!(ev(":-P9999999999999999W"), None);
        assert_eq!(ev(":-P99999999999999999999D"), None);
        assert_eq!(ev(":-PT9223372036854775807H"), None);
        assert_eq!(ev(":-P400D"), None);
        assert_eq!(ev(":PT0S"), Some(0));
        assert_eq!(ev(";VALUE=DATE-TIME:20261103T130000Z"), None);

        // Rappel changé : l'alarme d'origine est remplacée ; inchangé : conservée telle quelle.
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261103T140000Z\r\n\
                   BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let mut e = parse_ical(raw).remove(0);
        assert!(write_ical(&e, Some(raw), "20261103T120000Z").contains("ACTION:AUDIO"));
        e.reminder = Some(60);
        let out = write_ical(&e, Some(raw), "20261103T120000Z");
        assert!(!out.contains("ACTION:AUDIO") && out.contains("TRIGGER:-PT1H"));
        e.reminder = None;
        assert!(!write_ical(&e, Some(raw), "20261103T120000Z").contains("VALARM"));
        let fresh = Event { uid: "n".into(), reminder: Some(1440), ..Default::default() };
        assert!(write_ical(&fresh, None, "20261103T120000Z").contains("TRIGGER:-P1D"));
    }
}
