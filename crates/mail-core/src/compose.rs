//! Construction des messages à envoyer (RFC 5322 et MIME), sans dépendance.
//!
//! - en-têtes non ASCII encodés selon la RFC 2047 (`=?UTF-8?B?…?=`) ;
//! - corps `multipart/alternative` : texte brut + HTML, en quoted-printable ;
//! - `Message-ID`, `In-Reply-To`, `References` pour les fils de discussion ;
//! - préparation pour la commande SMTP `DATA` (point en début de ligne doublé).

use std::fmt::Write as _;

/// Fichier joint à un message envoyé.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutgoingAttachment {
    /// Nom du fichier.
    pub name: String,
    /// Type MIME (`application/pdf`…).
    pub content_type: String,
    /// Contenu brut, encodé en base64 à l'envoi.
    pub data: Vec<u8>,
}

/// Calendrier iTIP joint au corps (lot H1, iMIP : RFC 6047), en troisième
/// version du `multipart/alternative` : `text/calendar; method=…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingCalendar {
    /// Méthode iTIP (`REPLY`…), en lettres majuscules.
    pub method: String,
    /// Calendrier complet (`BEGIN:VCALENDAR` … `END:VCALENDAR`).
    pub ics: String,
}

/// Message rédigé, tel que l'interface l'envoie.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "camelCase", default))]
pub struct Draft {
    /// Nom affiché de l'expéditeur.
    pub from_name: String,
    /// Adresse de l'expéditeur.
    pub from_email: String,
    /// Destinataires (`To`).
    pub to: Vec<String>,
    /// Copies (`Cc`).
    pub cc: Vec<String>,
    /// Copies cachées (`Bcc`) : jamais écrites dans le message envoyé.
    pub bcc: Vec<String>,
    /// Objet, encodé à l'envoi si besoin (RFC 2047).
    pub subject: String,
    /// Corps HTML (fragment) ; la version texte en est dérivée.
    pub html: String,
    /// `Message-ID` du message auquel on répond.
    pub in_reply_to: Option<String>,
    /// `References` du message auquel on répond (sans son propre `Message-ID`).
    pub references: Option<String>,
    /// Message en texte brut : une seule partie `text/plain`, sans HTML.
    pub plain_text: bool,
    /// Clé d'envoi : 32 chiffres hexadécimaux tirés une fois par fenêtre de
    /// rédaction. Un nouvel essai après une coupure garde ainsi le même
    /// `Message-ID`, à quoi serveurs et destinataires reconnaissent un doublon
    /// (relecture complète n° 9). Vide ou mal formée : `Message-ID` neuf.
    pub send_key: String,
    /// Décalage de l'heure locale de l'expéditeur, en secondes, pour l'en-tête
    /// `Date` (RFC 5322 §3.3 : heure locale ; relecture complète n° 31). Fixé
    /// par le backend ; 0 : UTC.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub utc_offset: i32,
    /// Fichiers joints : fournis à part par le backend (jamais en JSON).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub attachments: Vec<OutgoingAttachment>,
    /// Signer le message en S/MIME à l'envoi (lot D2) ; jamais pour un brouillon.
    pub sign: bool,
    /// Chiffrer le message en S/MIME à l'envoi (lot D3b) ; jamais pour un brouillon.
    pub encrypt: bool,
    /// Réponse à une invitation (lot H1) : fournie par le backend (jamais en JSON).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub calendar: Option<OutgoingCalendar>,
}

impl Draft {
    /// Tous les destinataires SMTP (À, Cc, Cci), sans doublon.
    #[must_use]
    pub fn recipients(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for a in self.to.iter().chain(&self.cc).chain(&self.bcc) {
            let a = a.trim();
            if !a.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(a)) {
                out.push(a.to_string());
            }
        }
        out
    }

    /// Vérifie le message avant l'envoi : expéditeur, destinataires, adresses valides.
    ///
    /// # Errors
    ///
    /// Message traduit qui dit ce qui ne va pas (adresse invalide, aucun destinataire…).
    pub fn validate(&self) -> Result<(), String> {
        if !is_valid_email(&self.from_email) {
            let what = crate::lang::pick("adresse d'expéditeur invalide :", "invalid sender address:", "dirección de remitente no válida:");
            return Err(format!("{what} {}", self.from_email));
        }
        let recipients = self.recipients();
        if recipients.is_empty() {
            return Err(crate::lang::pick(
                "Ajoutez au moins un destinataire.",
                "Add at least one recipient.",
                "Añada al menos un destinatario.",
            )
            .into());
        }
        if let Some(bad) = recipients.iter().find(|a| !is_valid_email(a)) {
            let what = crate::lang::pick("Adresse invalide :", "Invalid address:", "Dirección no válida:");
            return Err(format!("{what} « {bad} »"));
        }
        // Domaine dont l'étiquette ne se convertit pas en Punycode (démesurée).
        // Une partie locale accentuée, elle, part telle quelle si le serveur
        // annonce SMTPUTF8 (lot H2, RFC 6531) ; sinon l'envoi le dit clairement.
        if let Some(bad) = std::iter::once(&self.from_email).chain(&recipients).find(|a| wire_address(a).is_none()) {
            let what = crate::lang::pick("Adresse invalide :", "Invalid address:", "Dirección no válida:");
            return Err(format!("{what} « {bad} »"));
        }
        Ok(())
    }

    /// Adresses écrites pour l'envoi : domaines internationaux en Punycode
    /// (`exemple-é.fr` → `xn--exemple--9ya.fr`), partie locale inchangée. Une
    /// adresse qui ne se convertit pas est laissée telle quelle (`validate` la refuse).
    pub fn wire_addresses(&mut self) {
        for list in [&mut self.to, &mut self.cc, &mut self.bcc] {
            for a in list.iter_mut() {
                if let Some(wire) = wire_address(a) {
                    *a = wire;
                }
            }
        }
        if let Some(wire) = wire_address(&self.from_email) {
            self.from_email = wire;
        }
    }

    /// Une adresse a une partie locale accentuée : l'envoi demande SMTPUTF8
    /// (RFC 6531), et les en-têtes portent l'adresse en UTF-8 (RFC 6532).
    #[must_use]
    pub fn needs_smtputf8(&self) -> bool {
        std::iter::once(&self.from_email).chain(&self.recipients()).any(|a| !local_part_is_ascii(a))
    }
}

/// La partie locale (avant le dernier `@`) est en ASCII.
fn local_part_is_ascii(address: &str) -> bool {
    address.trim().rsplit_once('@').is_none_or(|(local, _)| local.is_ascii())
}

/// Adresse en ASCII : domaine converti en Punycode (IDNA, RFC 5891), en
/// minuscules ; `None` si la partie locale n'est pas en ASCII.
#[must_use]
pub fn ascii_address(address: &str) -> Option<String> {
    wire_address(address).filter(|a| a.is_ascii())
}

/// Adresse pour l'envoi : domaine converti en Punycode (IDNA, RFC 5891), en
/// minuscules, partie locale inchangée (accentuée, elle demande SMTPUTF8) ;
/// `None` sans `@` ou si une étiquette ne se convertit pas.
#[must_use]
pub fn wire_address(address: &str) -> Option<String> {
    let address = address.trim();
    let (local, domain) = address.rsplit_once('@')?;
    if domain.is_ascii() {
        return Some(address.to_string());
    }
    let labels: Option<Vec<String>> = domain
        .split('.')
        .map(|label| {
            let label = label.to_lowercase();
            if label.is_ascii() {
                Some(label)
            } else {
                punycode(&label).map(|p| format!("xn--{p}"))
            }
        })
        .collect();
    Some(format!("{local}@{}", labels?.join(".")))
}

/// Encodage Punycode d'une étiquette (RFC 3492 §6.3) ; `None` si un calcul
/// déborde (étiquette démesurée).
#[must_use]
pub fn punycode(input: &str) -> Option<String> {
    const BASE: u32 = 36;
    const T_MIN: u32 = 1;
    const T_MAX: u32 = 26;
    const SKEW: u32 = 38;
    const DAMP: u32 = 700;
    fn adapt(mut delta: u32, points: u32, first: bool) -> u32 {
        delta /= if first { DAMP } else { 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > ((BASE - T_MIN) * T_MAX) / 2 {
            delta /= BASE - T_MIN;
            k += BASE;
        }
        k + (BASE - T_MIN + 1) * delta / (delta + SKEW)
    }
    /// Chiffre de base 36 : 0 à 25 → `a` à `z`, 26 à 35 → `0` à `9`.
    fn digit(d: u32) -> char {
        let d = u8::try_from(d).unwrap_or(0);
        char::from(if d < 26 { b'a' + d } else { b'0' + (d - 26) })
    }
    let points: Vec<u32> = input.chars().map(u32::from).collect();
    let mut out: String = input.chars().filter(char::is_ascii).collect();
    let basic = u32::try_from(out.len()).ok()?;
    if basic > 0 {
        out.push('-');
    }
    let (mut code, mut delta, mut bias, mut handled) = (128u32, 0u32, 72u32, basic);
    let total = u32::try_from(points.len()).ok()?;
    while handled < total {
        let next = points.iter().copied().filter(|&c| c >= code).min()?;
        delta = delta.checked_add((next - code).checked_mul(handled + 1)?)?;
        code = next;
        for &c in &points {
            if c < code {
                delta = delta.checked_add(1)?;
            }
            if c == code {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        T_MIN
                    } else if k >= bias + T_MAX {
                        T_MAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        code = code.checked_add(1)?;
    }
    Some(out)
}

/// Vérification simple d'une adresse : `local@domaine.tld`, sans espace.
#[must_use]
pub fn is_valid_email(s: &str) -> bool {
    let s = s.trim();
    let Some((local, domain)) = s.split_once('@') else { return false };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s.chars().any(|c| c.is_whitespace() || "<>,;\"()[]\\".contains(c))
        && !domain.contains('@')
}

// ------------------------------------------------------------------ base64

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode en base64 standard (RFC 4648), avec remplissage, sans retour à la ligne.
#[must_use]
pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (usize::from(b[0]) << 16) | (usize::from(b[1]) << 8) | usize::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(B64[(n >> (18 - 6 * i)) & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Décodage base64 (espaces et retours à la ligne ignorés).
///
/// # Errors
///
/// Caractère hors de l'alphabet base64.
pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => return Err("contenu base64 invalide".into()),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            let [.., byte] = (acc >> bits).to_be_bytes();
            out.push(byte);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// Base64 découpé en lignes de 76 caractères (RFC 2045).
fn base64_lines(data: &[u8]) -> String {
    let encoded = base64(data);
    let mut out = String::with_capacity(encoded.len() + encoded.len() / 76 * 2 + 2);
    for (i, chunk) in encoded.as_bytes().chunks(76).enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
    }
    out
}

/// Paramètre de nom de fichier (`name=` / `filename=`), RFC 2231 si non ASCII.
fn file_name_params(name: &str) -> (String, String) {
    let clean: String = name.chars().filter(|c| !c.is_control()).collect();
    let clean = if clean.trim().is_empty() { "fichier".to_string() } else { clean.trim().to_string() };
    let plain = clean.bytes().all(|b| (0x20..0x7f).contains(&b));
    if plain {
        let quoted = clean.replace('\\', "\\\\").replace('"', "\\\"");
        return (format!("name=\"{quoted}\""), format!("filename=\"{quoted}\""));
    }
    let mut pct = String::new();
    for b in clean.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            pct.push(char::from(b));
        } else {
            let _ = write!(pct, "%{b:02X}");
        }
    }
    // `name=` encodé RFC 2047 en complément, pour les lecteurs plus anciens.
    let words = encode_header_text(&clean).replace("\r\n ", " ");
    (format!("name=\"{words}\""), format!("filename*=UTF-8''{pct}"))
}

fn safe_content_type(ct: &str) -> &str {
    let ct = ct.trim();
    let valid = ct.contains('/') && ct.bytes().all(|b| b.is_ascii_alphanumeric() || b"/.+-_".contains(&b));
    if valid {
        ct
    } else {
        "application/octet-stream"
    }
}

// ------------------------------------------------------------------ en-têtes

/// Mot encodé RFC 2047 si nécessaire, découpé pour rester sous 76 caractères.
/// Les mots sont séparés par un pliage d'en-tête (`CRLF` + espace).
#[must_use]
pub fn encode_header_text(text: &str) -> String {
    encode_header_after(text, 1)
}

/// Comme [`encode_header_text`], la première ligne commençant après `used`
/// caractères (nom de l'en-tête, `: ` compris) : chaque ligne qui contient un
/// mot encodé reste sous 76 caractères (RFC 2047 §2). Un texte ASCII est
/// replié aux espaces vers 78 caractères (RFC 5322 §2.1.1 ; relecture
/// complète n° 16).
fn encode_header_after(text: &str, used: usize) -> String {
    if text.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return fold_ascii(text, used);
    }
    // Octets par mot : 4 caractères base64 pour 3 octets, plus 12 de syntaxe.
    let budget = |room: usize| (room.saturating_sub(12) / 4 * 3).max(3);
    let mut limit = budget(76usize.saturating_sub(used));
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in text.chars() {
        if !chunk.is_empty() && chunk.len() + c.len_utf8() > limit {
            words.push(format!("=?UTF-8?B?{}?=", base64(chunk.as_bytes())));
            chunk.clear();
            // Lignes suivantes : une espace de pliage avant le mot.
            limit = budget(75);
        }
        chunk.push(c);
    }
    if !chunk.is_empty() {
        words.push(format!("=?UTF-8?B?{}?=", base64(chunk.as_bytes())));
    }
    words.join("\r\n ")
}

/// Texte ASCII replié aux espaces pour que les lignes restent vers 78
/// caractères ; un mot plus long reste entier (la limite absolue est 998).
fn fold_ascii(text: &str, used: usize) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut width = used;
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            if width + 1 + word.len() > 78 && !word.is_empty() {
                out.push_str("\r\n ");
                width = 1;
            } else {
                out.push(' ');
                width += 1;
            }
        }
        out.push_str(word);
        width += word.len();
    }
    out
}

/// `Nom <adresse>` avec le nom encodé ou entre guillemets selon le besoin.
#[must_use]
pub fn format_mailbox(name: &str, email: &str) -> String {
    mailbox_after(name, email, 1)
}

/// Comme [`format_mailbox`], après `used` caractères sur la ligne (nom de l'en-tête).
fn mailbox_after(name: &str, email: &str, used: usize) -> String {
    let name = name.trim();
    if name.is_empty() || name == email {
        return email.to_string();
    }
    if name.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        let needs_quotes = name.chars().any(|c| "()<>[]:;@\\,.\"".contains(c));
        if needs_quotes {
            format!("\"{}\" <{email}>", name.replace('\\', "\\\\").replace('"', "\\\""))
        } else {
            format!("{name} <{email}>")
        }
    } else {
        format!("{} <{email}>", encode_header_after(name, used))
    }
}

const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Année, mois (1 à 12) et jour d'un nombre de jours depuis 1970.
pub(crate) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Date RFC 5322 en UTC : `Mon, 02 Nov 2026 09:00:00 +0000`.
#[must_use]
pub fn format_date(ts: i64) -> String {
    format_date_at(ts, 0)
}

/// Date RFC 5322 à l'heure locale de l'expéditeur, `offset` secondes après
/// l'UTC : `Mon, 02 Nov 2026 10:00:00 +0100` (relecture complète n° 31). Un
/// décalage hors de ±24 h est ramené à l'UTC.
#[must_use]
pub fn format_date_at(ts: i64, offset: i32) -> String {
    let offset = if offset.unsigned_abs() < 24 * 3600 { offset } else { 0 };
    let local = ts.saturating_add(i64::from(offset));
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let sign = if offset < 0 { '-' } else { '+' };
    let minutes = offset.unsigned_abs() / 60;
    format!(
        "{}, {d:02} {} {y:04} {:02}:{:02}:{:02} {sign}{:02}{:02}",
        DAYS[usize::try_from(days.rem_euclid(7)).unwrap_or_default()],
        MONTHS[usize::try_from(m - 1).unwrap_or_default()],
        secs / 3600,
        secs % 3600 / 60,
        secs % 60,
        minutes / 60,
        minutes % 60
    )
}

/// Octets d'une clé d'envoi (`Draft::send_key`) : exactement 32 chiffres
/// hexadécimaux, sinon `None`.
#[must_use]
pub fn send_key_bytes(key: &str) -> Option<[u8; 16]> {
    if key.len() != 32 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(key.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Identifiant unique : `<aléa@domaine-de-l-expéditeur>`. L'aléa (octets du
/// générateur du système, fournis par l'appelant) ne révèle ni l'heure ni le
/// processus ; un domaine qui ne ressemble pas à un nom d'hôte est remplacé.
#[must_use]
pub fn new_message_id(from_email: &str, random: &[u8]) -> String {
    let mut unique = String::with_capacity(random.len() * 2);
    for b in random {
        let _ = write!(unique, "{b:02x}");
    }
    let domain = from_email
        .rsplit_once('@')
        .map(|(_, d)| d)
        .filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
        .unwrap_or("vortexmail.local");
    format!("<{unique}@{domain}>")
}

/// Valeur reprise d'un message reçu : les caractères de contrôle (CR isolé…)
/// deviennent des espaces et ne peuvent pas ouvrir un nouvel en-tête.
fn no_controls(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

// ------------------------------------------------------------------ corps

/// Encodage quoted-printable (RFC 2045), lignes de 76 caractères au plus.
#[must_use]
pub fn quoted_printable(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    let mut out = String::with_capacity(normalized.len() + normalized.len() / 8);
    for (i, line) in normalized.split('\n').enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        let bytes = line.as_bytes();
        let mut width = 0;
        for (j, &b) in bytes.iter().enumerate() {
            let last = j + 1 == bytes.len();
            let literal = (b == b' ' || b == b'\t') && !last || (33..=126).contains(&b) && b != b'=';
            let mut piece = if literal { char::from(b).to_string() } else { format!("={b:02X}") };
            // Coupure « douce » : `=` en fin de ligne, invisible au décodage.
            if width + piece.len() > 75 {
                out.push_str("=\r\n");
                width = 0;
            }
            // « From » en début de ligne : certains serveurs le changent en « >From »,
            // ce qui casserait une signature (RFC 3156 §3). Encodé, il passe intact.
            if width == 0 && bytes.get(j..).is_some_and(|rest| rest.starts_with(b"From ")) {
                piece = "=46".to_string();
            }
            out.push_str(&piece);
            width += piece.len();
        }
    }
    out
}

/// Version texte d'un fragment HTML (pour la partie `text/plain`).
pub fn html_to_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    // Lien ouvert : position de son texte dans `out`, et son adresse.
    let mut link: Option<(usize, String)> = None;
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let Some(end) = html[i..].find('>').map(|e| i + e) else { break };
            let tag = &lower[i + 1..end];
            let name: String = tag.trim_start_matches('/').chars().take_while(char::is_ascii_alphanumeric).collect();
            // Contenu ignoré.
            if !tag.starts_with('/') && (name == "style" || name == "script" || name == "head") {
                let close = format!("</{name}");
                i = lower[end..].find(&close).map_or(bytes.len(), |p| end + p);
                if let Some(e) = html[i..].find('>') {
                    i += e + 1;
                }
                continue;
            }
            // Lien : son adresse suit le texte quand elle en diffère, pour un
            // destinataire qui lit en texte brut (relecture complète n° 27).
            if name == "a" {
                if tag.starts_with('/') {
                    if let Some((start, href)) = link.take() {
                        let shown = decode_entities(out[start..].trim());
                        let bare = href.trim_start_matches("mailto:");
                        let plain = |s: &str| s.trim_end_matches('/').to_ascii_lowercase();
                        if !href.is_empty() && plain(&shown) != plain(&href) && plain(&shown) != plain(bare) {
                            let _ = write!(out, " <{bare}>");
                        }
                    }
                } else {
                    link = href_of(&html[i + 1..end]).map(|h| (out.len(), h));
                }
            }
            match name.as_str() {
                "br" => out.push('\n'),
                "p" | "div" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "ul" | "ol" => {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                "li" if !tag.starts_with('/') => {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("• ");
                }
                _ => {}
            }
            i = end + 1;
        } else {
            let next = html[i..].find('<').map_or(bytes.len(), |p| i + p);
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    let decoded = decode_entities(&out);
    // Lignes nettoyées, au plus une ligne vide consécutive.
    let mut result = String::new();
    let mut blank = 0;
    for line in decoded.lines().map(str::trim_end) {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 || result.is_empty() {
                continue;
            }
        } else {
            blank = 0;
        }
        result.push_str(line);
        result.push('\n');
    }
    result.trim_end().to_string()
}

/// Adresse `http(s)`, `ftp` ou `mailto` d'une balise `<a …>`, entités décodées.
fn href_of(tag: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find("href") {
        let at = from + found;
        from = at + 4;
        // `href` doit être un nom d'attribut entier (pas `data-href`).
        if at > 0 && !lower.as_bytes()[at - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = tag[at + 4..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
        };
        let href = decode_entities(value.trim());
        let scheme = href.split(':').next().unwrap_or("").to_ascii_lowercase();
        return matches!(scheme.as_str(), "http" | "https" | "ftp" | "mailto").then_some(href);
    }
    None
}

/// Entités HTML courantes et numériques décodées ; une entité inconnue reste telle quelle.
fn decode_entities(text: &str) -> String {
    const NAMED: [(&str, char); 44] = [
        ("nbsp", ' '),
        ("lt", '<'),
        ("gt", '>'),
        ("quot", '"'),
        ("apos", '\''),
        ("amp", '&'),
        ("eacute", 'é'),
        ("egrave", 'è'),
        ("ecirc", 'ê'),
        ("euml", 'ë'),
        ("agrave", 'à'),
        ("aacute", 'á'),
        ("acirc", 'â'),
        ("auml", 'ä'),
        ("ccedil", 'ç'),
        ("icirc", 'î'),
        ("iuml", 'ï'),
        ("iacute", 'í'),
        ("ocirc", 'ô'),
        ("ouml", 'ö'),
        ("oacute", 'ó'),
        ("ugrave", 'ù'),
        ("ucirc", 'û'),
        ("uuml", 'ü'),
        ("uacute", 'ú'),
        ("ntilde", 'ñ'),
        ("Eacute", 'É'),
        ("Egrave", 'È'),
        ("Agrave", 'À'),
        ("Ccedil", 'Ç'),
        ("oelig", 'œ'),
        ("OElig", 'Œ'),
        ("euro", '€'),
        ("laquo", '«'),
        ("raquo", '»'),
        ("hellip", '…'),
        ("ndash", '–'),
        ("mdash", '—'),
        ("lsquo", '‘'),
        ("rsquo", '’'),
        ("ldquo", '“'),
        ("rdquo", '”'),
        ("copy", '©'),
        ("deg", '°'),
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.bytes().take(12).position(|b| b == b';');
        let decoded = end.and_then(|end| {
            let entity = &tail[1..end];
            let c = if let Some(hex) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else if let Some(dec) = entity.strip_prefix('#') {
                dec.parse().ok().and_then(char::from_u32)
            } else {
                NAMED.iter().find(|(n, _)| *n == entity).map(|(_, c)| *c)
            };
            c.filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).map(|c| (c, end))
        });
        if let Some((c, end)) = decoded {
            out.push(c);
            rest = &tail[end + 1..];
        } else {
            out.push('&');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Valeur de `References` : celles du parent puis le parent lui-même, chacun
/// une seule fois, que l'interface l'ait déjà ajouté ou non (relecture
/// complète n° 13), une par ligne.
fn references(known: Option<&str>, parent: &str) -> String {
    let known = known.map_or_default(no_controls);
    let mut refs: Vec<&str> = Vec::new();
    for id in known.split_whitespace().filter(|id| *id != parent).chain(std::iter::once(parent)) {
        if !refs.contains(&id) {
            refs.push(id);
        }
    }
    refs.join("\r\n ")
}

/// Message complet, prêt pour SMTP et pour l'`APPEND` dans « Éléments envoyés ».
#[must_use]
pub fn build(draft: &Draft, date: i64, message_id: &str) -> Vec<u8> {
    build_message(draft, date, message_id, false)
}

/// Brouillon à déposer dans « Brouillons » : comme [`build`], mais avec l'en-tête
/// `Cci` (pour retrouver les destinataires en reprenant le brouillon).
#[must_use]
pub fn build_draft(draft: &Draft, date: i64, message_id: &str) -> Vec<u8> {
    build_message(draft, date, message_id, true)
}

fn build_message(draft: &Draft, date: i64, message_id: &str, keep_bcc: bool) -> Vec<u8> {
    let mut out = headers(draft, date, message_id, keep_bcc).into_bytes();
    out.extend_from_slice(entity(draft, message_id).as_bytes());
    out
}

/// Message signé en S/MIME (RFC 8551 §3.5.3, `multipart/signed`, signature détachée : lisible
/// aussi sans S/MIME). La partie signée est le corps tel que [`build`] l'écrit, en ASCII
/// (quoted-printable et base64) : le transport ne la change pas. `sign` reçoit ses octets et
/// rend la signature CMS (DER) avec le nom de son hachage pour `micalg` (`sha-256`…).
///
/// Rend le message à envoyer et la copie pour « Éléments envoyés » (avec `Bcc`, hors de la
/// partie signée), signés une seule fois.
///
/// # Errors
///
/// L'erreur de `sign`.
pub fn build_signed<E>(
    draft: &Draft,
    date: i64,
    message_id: &str,
    sign: impl FnOnce(&[u8]) -> Result<(Vec<u8>, &'static str), E>,
) -> Result<(Vec<u8>, Vec<u8>), E> {
    let body = signed_entity(draft, message_id, sign)?;
    let message = |keep_bcc: bool| {
        let mut out = headers(draft, date, message_id, keep_bcc).into_bytes();
        out.extend_from_slice(body.as_bytes());
        out
    };
    Ok((message(false), message(true)))
}

/// Le contenu signé d'un message (`multipart/signed`), en-têtes `Content-Type` compris.
fn signed_entity<E>(draft: &Draft, message_id: &str, sign: impl FnOnce(&[u8]) -> Result<(Vec<u8>, &'static str), E>) -> Result<String, E> {
    let signed = entity(draft, message_id);
    let (signature, micalg) = sign(signed.as_bytes())?;
    let boundary = format!("=_vortexsig_{:x}", boundary_seed(message_id));
    let mut body = format!(
        "Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg={micalg};\r\n boundary=\"{boundary}\"\r\n\r\n"
    );
    body.push_str("This is an S/MIME signed message.\r\n");
    let _ = write!(body, "\r\n--{boundary}\r\n{signed}");
    let _ = write!(body, "\r\n--{boundary}\r\n");
    body.push_str("Content-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\n");
    body.push_str("Content-Transfer-Encoding: base64\r\n");
    body.push_str("Content-Disposition: attachment; filename=\"smime.p7s\"\r\n\r\n");
    body.push_str(&base64_lines(&signature));
    let _ = write!(body, "\r\n--{boundary}--\r\n");
    Ok(body)
}

/// Signature à faire avant le chiffrement : rend la signature CMS et le nom de son hachage.
pub type Signer<'a, E> = &'a mut dyn FnMut(&[u8]) -> Result<(Vec<u8>, &'static str), E>;

/// Message chiffré en S/MIME (RFC 8551 §3.3, `application/pkcs7-mime`,
/// `smime-type=enveloped-data`), signé d'abord si `sign` est donné (RFC 8551 §3.6 : signé
/// puis chiffré, la signature est donc invisible de l'extérieur). `encrypt` reçoit l'entité
/// à chiffrer et rend le CMS `EnvelopedData` (DER).
///
/// Rend le message à envoyer et la copie pour « Éléments envoyés » (avec `Bcc`, en clair dans
/// les en-têtes de la copie seulement), chiffrés une seule fois. L'objet, l'expéditeur et les
/// destinataires restent lisibles : seuls le corps et les pièces jointes sont chiffrés.
///
/// # Errors
///
/// L'erreur de `sign` ou de `encrypt`.
pub fn build_encrypted<E>(
    draft: &Draft,
    date: i64,
    message_id: &str,
    sign: Option<Signer<'_, E>>,
    encrypt: impl FnOnce(&[u8]) -> Result<Vec<u8>, E>,
) -> Result<(Vec<u8>, Vec<u8>), E> {
    let inner = match sign {
        Some(sign) => signed_entity(draft, message_id, |content| sign(content))?,
        None => entity(draft, message_id),
    };
    let cms = encrypt(inner.as_bytes())?;
    let mut body = String::from("Content-Type: application/pkcs7-mime; smime-type=enveloped-data; name=\"smime.p7m\"\r\n");
    body.push_str("Content-Transfer-Encoding: base64\r\n");
    body.push_str("Content-Disposition: attachment; filename=\"smime.p7m\"\r\n\r\n");
    body.push_str(&base64_lines(&cms));
    body.push_str("\r\n");
    let message = |keep_bcc: bool| {
        let mut out = headers(draft, date, message_id, keep_bcc).into_bytes();
        out.extend_from_slice(body.as_bytes());
        out
    };
    Ok((message(false), message(true)))
}

/// Graine des frontières MIME, dérivée du `Message-ID` : unique et absente du contenu encodé.
fn boundary_seed(message_id: &str) -> u64 {
    message_id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

/// En-têtes du message, sans ceux du contenu (`Content-Type`…), qu'écrit [`entity`].
fn headers(draft: &Draft, date: i64, message_id: &str, keep_bcc: bool) -> String {
    let mut h = String::new();
    let _ = write!(h, "Date: {}\r\n", format_date_at(date, draft.utc_offset));
    let _ = write!(h, "From: {}\r\n", mailbox_after(&draft.from_name, &draft.from_email, "From: ".len()));
    if !draft.to.is_empty() {
        let _ = write!(h, "To: {}\r\n", draft.to.join(",\r\n "));
    }
    if !draft.cc.is_empty() {
        let _ = write!(h, "Cc: {}\r\n", draft.cc.join(",\r\n "));
    }
    // Cci : jamais dans les en-têtes d'un message envoyé, seulement dans l'enveloppe SMTP.
    if keep_bcc && !draft.bcc.is_empty() {
        let _ = write!(h, "Bcc: {}\r\n", draft.bcc.join(",\r\n "));
    }
    let _ = write!(h, "Subject: {}\r\n", encode_header_after(&no_controls(&draft.subject), "Subject: ".len()));
    let _ = write!(h, "Message-ID: {message_id}\r\n");
    let parent = draft.in_reply_to.as_deref().map_or_default(no_controls);
    if !parent.trim().is_empty() {
        let parent = parent.trim();
        let _ = write!(h, "In-Reply-To: {parent}\r\n");
        let _ = write!(h, "References: {}\r\n", references(draft.references.as_deref(), parent));
    }
    h.push_str("MIME-Version: 1.0\r\n");
    h.push_str("X-Mailer: Vortex Mail\r\n");
    h
}

/// Le contenu du message : ses en-têtes `Content-*`, une ligne vide, puis le corps (texte,
/// HTML, images liées et fichiers joints), en ASCII seulement.
fn entity(draft: &Draft, message_id: &str) -> String {
    let boundary = format!("=_vortex_{:x}", boundary_seed(message_id));

    // Images collées dans le corps (`data:`) : envoyées en parties liées (`cid:`),
    // seule forme que les webmails affichent.
    let (body_html, inline) =
        if draft.plain_text { (draft.html.clone(), Vec::new()) } else { extract_inline_images(&draft.html, message_id) };
    let text = html_to_text(&body_html);
    let html = format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"></head>\n\
         <body style=\"font-family: Aptos, 'Segoe UI', sans-serif; font-size: 11pt;\">\n{body_html}\n</body></html>"
    );

    // multipart/mixed { multipart/related { multipart/alternative, images }, fichiers }
    // — chaque niveau n'est présent que s'il sert.
    let mut part_type = format!("multipart/alternative; boundary=\"{boundary}\"");
    let mut part = String::new();
    let _ = write!(part, "--{boundary}\r\n");
    part.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    part.push_str("Content-Transfer-Encoding: quoted-printable\r\n\r\n");
    part.push_str(&quoted_printable(&text));
    let _ = write!(part, "\r\n--{boundary}\r\n");
    part.push_str("Content-Type: text/html; charset=utf-8\r\n");
    part.push_str("Content-Transfer-Encoding: quoted-printable\r\n\r\n");
    part.push_str(&quoted_printable(&html));
    if let Some(calendar) = &draft.calendar {
        // Méthode réduite aux lettres : elle est écrite dans l'en-tête de la partie.
        let method: String = calendar.method.chars().filter(char::is_ascii_alphabetic).collect::<String>().to_ascii_uppercase();
        let _ = write!(part, "\r\n--{boundary}\r\n");
        let _ = write!(part, "Content-Type: text/calendar; charset=utf-8; method={method}\r\n");
        part.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
        part.push_str(&base64_lines(calendar.ics.as_bytes()));
    }
    let _ = write!(part, "\r\n--{boundary}--\r\n");

    if draft.plain_text && draft.calendar.is_none() {
        // Partie unique : son encodage accompagne son type.
        part_type = "text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable".to_string();
        part = quoted_printable(&text);
        part.push_str("\r\n");
    }

    if !inline.is_empty() {
        let related = format!("=_vortexrel_{}", &boundary["=_vortex_".len()..]);
        let mut r = format!("--{related}\r\nContent-Type: {part_type}\r\n\r\n{part}");
        for (i, img) in inline.iter().enumerate() {
            let _ = write!(r, "\r\n--{related}\r\n");
            let _ = write!(r, "Content-Type: {}; name=\"image{}\"\r\n", safe_content_type(&img.content_type), i + 1);
            let _ = write!(r, "Content-ID: <{}>\r\n", img.cid);
            let _ = write!(r, "Content-Disposition: inline; filename=\"image{}\"\r\n", i + 1);
            r.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
            r.push_str(&base64_lines(&img.data));
            r.push_str("\r\n");
        }
        let _ = write!(r, "\r\n--{related}--\r\n");
        part_type = format!("multipart/related; type=\"multipart/alternative\"; boundary=\"{related}\"");
        part = r;
    }

    let mut h = String::new();
    let mut b = String::new();
    if draft.attachments.is_empty() {
        let _ = write!(h, "Content-Type: {part_type}\r\n");
        b.push_str(&part);
    } else {
        let mixed = format!("=_vortexmix_{}", &boundary["=_vortex_".len()..]);
        let _ = write!(h, "Content-Type: multipart/mixed; boundary=\"{mixed}\"\r\n");
        b.push_str("This is a multi-part message in MIME format.\r\n");
        let _ = write!(b, "\r\n--{mixed}\r\n");
        let _ = write!(b, "Content-Type: {part_type}\r\n\r\n");
        b.push_str(&part);
        for a in &draft.attachments {
            let (name, filename) = file_name_params(&a.name);
            let _ = write!(b, "\r\n--{mixed}\r\n");
            let _ = write!(b, "Content-Type: {}; {name}\r\n", safe_content_type(&a.content_type));
            let _ = write!(b, "Content-Disposition: attachment; {filename}\r\n");
            b.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
            b.push_str(&base64_lines(&a.data));
            b.push_str("\r\n");
        }
        let _ = write!(b, "\r\n--{mixed}--\r\n");
    }
    h.push_str("\r\n");
    h.push_str(&b);
    h
}

/// Image intégrée au corps, envoyée comme partie liée.
struct InlineImage {
    cid: String,
    content_type: String,
    data: Vec<u8>,
}

/// Remplace les images `data:image/…;base64,…` du HTML par des références `cid:`.
fn extract_inline_images(html: &str, message_id: &str) -> (String, Vec<InlineImage>) {
    let domain = message_id.trim_matches(['<', '>']).rsplit('@').next().unwrap_or("vortex.local").to_string();
    let mut out = String::with_capacity(html.len());
    let mut images: Vec<InlineImage> = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("data:image/") {
        let tail = &rest[i..];
        let quote_before = rest[..i].chars().last();
        let end = tail.find(|c: char| c == '"' || c == '\'' || c == ')' || c.is_whitespace() || c == '>').unwrap_or(tail.len());
        let uri = &tail[..end];
        let parsed = uri
            .strip_prefix("data:")
            .and_then(|u| u.split_once(";base64,"))
            .and_then(|(ct, data)| base64_decode(data).ok().map(|d| (ct.to_string(), d)));
        match parsed {
            Some((content_type, data)) if matches!(quote_before, Some('"' | '\'' | '(' | '=')) => {
                let cid = format!("img{}.{}@{}", images.len() + 1, message_id.len(), domain);
                out.push_str(&rest[..i]);
                let _ = write!(out, "cid:{cid}");
                images.push(InlineImage { cid, content_type, data });
            }
            _ => out.push_str(&rest[..i + end]),
        }
        rest = &tail[end..];
        if end == 0 {
            out.push_str(&rest[..1]);
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    (out, images)
}

/// Contenu de la commande SMTP `DATA` : lignes en CRLF, points de tête
/// doublés, terminé par `CRLF.CRLF`.
#[must_use]
pub fn smtp_data(message: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(message).replace("\r\n", "\n");
    let mut out = Vec::with_capacity(message.len() + 16);
    for line in text.split('\n') {
        if line.starts_with('.') {
            out.push(b'.');
        }
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    // `split` produit une ligne vide finale si le message se termine par CRLF.
    if out.ends_with(b"\r\n\r\n") {
        out.truncate(out.len() - 2);
    }
    out.extend_from_slice(b".\r\n");
    out
}

// ------------------------------------------------------------------ lecture d'en-têtes

/// Valeur d'un en-tête du message brut, lignes de continuation dépliées.
#[must_use]
pub fn header_value(raw: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let head = text.split("\r\n\r\n").next().unwrap_or("");
    let head = head.split("\n\n").next().unwrap_or("");
    let mut current: Option<String> = None;
    for line in head.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(v) = current.as_mut() {
                v.push(' ');
                v.push_str(line.trim());
            }
            continue;
        }
        if current.is_some() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case(name) {
                current = Some(value.trim().to_string());
            }
        }
    }
    current
}

/// Adresses d'une liste `To`/`Cc` brute : `"Dupont, Marie" <m@x.fr>, b@y.com`.
/// Commentaires (`jean@x.fr (Jean, Dupont)`) et groupes (`Équipe: a@x.fr,
/// b@y.fr;`) compris (RFC 5322 §3.4 ; relecture complète n° 10) ; `;` sépare
/// aussi deux adresses, comme dans le champ de saisie.
#[must_use]
pub fn parse_address_list(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let (mut quoted, mut angle, mut bracket, mut comment, mut escaped) = (false, false, false, 0u32, false);
    for c in s.chars() {
        if escaped {
            escaped = false;
            if comment == 0 {
                current.push(c);
            }
            continue;
        }
        if c == '\\' && (quoted || comment > 0) {
            escaped = true;
            if comment == 0 {
                current.push(c);
            }
            continue;
        }
        if comment > 0 {
            match c {
                '(' => comment += 1,
                ')' => comment -= 1,
                _ => {}
            }
            if comment == 0 {
                current.push(' ');
            }
            continue;
        }
        match c {
            '"' => quoted = !quoted,
            '(' if !quoted => {
                comment = 1;
                continue;
            }
            '<' if !quoted => angle = true,
            '>' if !quoted => angle = false,
            '[' if !quoted => bracket = true,
            ']' if !quoted => bracket = false,
            // Début d'un groupe : son nom n'est pas une adresse.
            ':' if !quoted && !angle && !bracket => {
                current.clear();
                continue;
            }
            ',' | ';' if !quoted && !angle && !bracket => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    parts.push(current);
    parts
        .into_iter()
        .filter_map(|p| {
            let addr = match (p.rfind('<'), p.rfind('>')) {
                (Some(l), Some(r)) if l < r => p[l + 1..r].trim().to_string(),
                _ => p.trim().to_string(),
            };
            is_valid_email(&addr).then_some(addr)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64_decode(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let (mut acc, mut bits) = (0u32, 0);
        for c in s.bytes().filter(|&c| c != b'=') {
            acc = (acc << 6) | u32::try_from(B64.iter().position(|&x| x == c).unwrap()).unwrap();
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                let [.., byte] = (acc >> bits).to_be_bytes();
                out.push(byte);
                acc &= (1 << bits) - 1;
            }
        }
        out
    }

    fn decode_words(s: &str) -> String {
        s.split_whitespace()
            .map(|w| {
                let inner = w.strip_prefix("=?UTF-8?B?").unwrap().strip_suffix("?=").unwrap();
                String::from_utf8(b64_decode(inner)).unwrap()
            })
            .collect()
    }

    fn qp_decode(s: &str) -> String {
        let s = s.replace("=\r\n", "");
        let mut out = Vec::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'=' {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    fn draft() -> Draft {
        Draft {
            from_name: "Fabrice Joffre".into(),
            from_email: "moi@exemple.fr".into(),
            to: vec!["marie@example.com".into()],
            cc: vec!["paul@example.org".into()],
            bcc: vec!["secret@example.net".into(), "MARIE@example.com".into()],
            subject: "Réunion de rentrée — ordre du jour".into(),
            html: "<p>Bonjour Marie,</p><p>Voici l'ordre du jour&nbsp;:</p><ul><li>budget</li><li>planning</li></ul>".into(),
            in_reply_to: Some("<abc@mail.example.com>".into()),
            references: Some("<root@example.com>".into()),
            plain_text: false,
            send_key: String::new(),
            utc_offset: 0,
            attachments: Vec::new(),
            sign: false,
            encrypt: false,
            calendar: None,
        }
    }

    #[test]
    fn h1_reponse_a_une_invitation() {
        let mut d = draft();
        d.plain_text = true;
        d.calendar =
            Some(OutgoingCalendar { method: "reply\r\nX: y".into(), ics: "BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nEND:VCALENDAR\r\n".into() });
        let raw = String::from_utf8(build(&d, 1_700_000_000, "<id@x>")).unwrap();
        assert!(raw.contains("Content-Type: multipart/alternative;"), "texte, HTML et calendrier : {raw}");
        assert!(raw.contains("Content-Type: text/calendar; charset=utf-8; method=REPLYXY\r\n"), "méthode nettoyée : {raw}");
        assert!(raw.contains(&base64("BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nEND:VCALENDAR\r\n".as_bytes())), "calendrier en base64");
    }

    #[test]
    fn message_chiffre_en_smime() {
        let d = draft();
        let plain = String::from_utf8(build(&d, 1_700_000_000, "<id@x>")).unwrap();
        let (sent, kept) = build_encrypted(&d, 1_700_000_000, "<id@x>", None, |inner| -> Result<_, ()> {
            let text = std::str::from_utf8(inner).unwrap();
            assert!(plain.ends_with(text), "le contenu du message non chiffré");
            Ok(vec![1, 2, 3])
        })
        .unwrap();
        let sent = String::from_utf8(sent).unwrap();
        assert!(sent.contains(
            "Content-Type: application/pkcs7-mime; smime-type=enveloped-data; name=\"smime.p7m\"\r\nContent-Transfer-Encoding: base64\r\n"
        ));
        assert!(sent.ends_with("\r\n\r\nAQID\r\n"));
        assert!(!sent.contains("multipart/alternative"), "rien du contenu en clair");
        assert!(!sent.contains("Bcc:"));
        assert!(String::from_utf8(kept).unwrap().contains("Bcc: secret@example.net"));
        // Signé puis chiffré : l'entité chiffrée est le `multipart/signed`.
        let mut sign = |_: &[u8]| -> Result<(Vec<u8>, &'static str), ()> { Ok((vec![9], "sha-256")) };
        build_encrypted(&d, 1_700_000_000, "<id@x>", Some(&mut sign), |inner| {
            assert!(std::str::from_utf8(inner).unwrap().starts_with("Content-Type: multipart/signed;"));
            Ok(Vec::new())
        })
        .unwrap();
    }

    #[test]
    fn message_signe_en_smime() {
        let d = draft();
        let plain = String::from_utf8(build(&d, 1_700_000_000, "<id@x>")).unwrap();
        let (sent, kept) = build_signed(&d, 1_700_000_000, "<id@x>", |signed| -> Result<_, ()> {
            // La partie signée est le contenu de `build`, en-têtes `Content-*` compris.
            let text = std::str::from_utf8(signed).unwrap();
            assert!(text.starts_with("Content-Type: multipart/alternative;"), "{text}");
            assert!(plain.ends_with(text), "même contenu que le message non signé");
            Ok((vec![1, 2, 3], "sha-256"))
        })
        .unwrap();
        let sent = String::from_utf8(sent).unwrap();
        let kept = String::from_utf8(kept).unwrap();
        assert!(sent.contains(
            "Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256;\r\n boundary=\"=_vortexsig_"
        ));
        assert!(sent.contains("Content-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\n"));
        assert!(sent.contains("\r\n\r\nAQID\r\n--=_vortexsig_"), "signature en base64");
        assert!(!sent.contains("Bcc:"), "pas de copie cachée dans le message envoyé");
        assert!(kept.contains("Bcc: secret@example.net"), "copie cachée dans « Éléments envoyés »");
        assert_eq!(sent.matches("Content-Type: multipart/alternative;").count(), 1, "contenu signé une fois");
        // Le contenu signé est entre la première frontière et la suivante, intact.
        let boundary = sent.split("boundary=\"").nth(1).unwrap().split('"').next().unwrap();
        let start = sent.find(&format!("--{boundary}\r\n")).unwrap() + boundary.len() + 4;
        let end = sent[start..].find(&format!("\r\n--{boundary}")).unwrap() + start;
        assert!(plain.ends_with(&sent[start..end]) && sent[start..end].ends_with("--\r\n"));
        // Une erreur de signature arrête tout.
        assert_eq!(build_signed(&d, 0, "<id@x>", |_| Err::<(Vec<u8>, &str), _>("refus")), Err("refus"));
    }

    #[test]
    fn from_en_debut_de_ligne_encode() {
        assert_eq!(quoted_printable("From me\nFromage\nX From y"), "=46rom me\r\nFromage\r\nX From y");
    }

    #[test]
    fn attachments_make_a_mixed_message() {
        let mut d = draft();
        d.attachments = vec![
            OutgoingAttachment { name: "devis.pdf".into(), content_type: "application/pdf".into(), data: vec![0, 1, 2, 255] },
            OutgoingAttachment { name: "Récapitulatif été.txt".into(), content_type: "bad\r\ntype".into(), data: b"x".to_vec() },
        ];
        let raw = String::from_utf8(build(&d, 0, "<id@x>")).unwrap();
        assert!(raw.contains("Content-Type: multipart/mixed; boundary="));
        assert!(raw.contains("Content-Type: multipart/alternative; boundary="));
        assert!(raw.contains("Content-Type: application/pdf; name=\"devis.pdf\""));
        assert!(raw.contains("Content-Disposition: attachment; filename=\"devis.pdf\""));
        assert!(raw.contains("AAEC/w=="));
        assert!(raw.contains("filename*=UTF-8''R%C3%A9capitulatif%20%C3%A9t%C3%A9.txt"));
        assert!(raw.contains("Content-Type: application/octet-stream; name=\"=?UTF-8?B?"));
        assert!(raw.trim_end().ends_with("--"));
        assert!(raw.lines().all(|l| l.len() <= 998));
    }

    #[test]
    fn drafts_keep_bcc() {
        let d = draft();
        let sent = String::from_utf8(build(&d, 0, "<id@x>")).unwrap();
        let saved = String::from_utf8(build_draft(&d, 0, "<id@x>")).unwrap();
        assert!(!sent.contains("Bcc:"));
        assert!(saved.contains("Bcc: secret@example.net,\r\n MARIE@example.com\r\n"));
    }

    #[test]
    fn plain_text_has_no_html_part() {
        let mut d = draft();
        d.plain_text = true;
        d.html = "Bonjour<br>À bientôt".into();
        let raw = String::from_utf8(build(&d, 0, "<id@x>")).unwrap();
        assert!(raw.contains("Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n"));
        assert!(!raw.contains("text/html"));
        assert!(!raw.contains("multipart"));
        d.attachments = vec![OutgoingAttachment { name: "a.txt".into(), content_type: "text/plain".into(), data: b"a".to_vec() }];
        let raw = String::from_utf8(build(&d, 0, "<id@x>")).unwrap();
        assert!(raw.contains("multipart/mixed"));
        assert!(!raw.contains("text/html"));
    }

    #[test]
    fn pasted_images_become_related_parts() {
        let mut d = draft();
        d.html = r#"<p>Voici :</p><img src="data:image/png;base64,iVBORw0KGgo=" alt="x"><p>fin</p>"#.into();
        let raw = String::from_utf8(build(&d, 0, "<id@exemple.fr>")).unwrap();
        assert!(raw.contains("Content-Type: multipart/related; type=\"multipart/alternative\"; boundary="));
        assert!(raw.contains("Content-ID: <img1."));
        assert!(raw.contains("cid:img1."));
        assert!(!raw.contains("data:image"));
        assert!(raw.contains("iVBORw0KGgo="));
        d.attachments = vec![OutgoingAttachment { name: "a.txt".into(), content_type: "text/plain".into(), data: b"a".to_vec() }];
        let raw = String::from_utf8(build(&d, 0, "<id@exemple.fr>")).unwrap();
        assert!(raw.contains("multipart/mixed"));
        assert!(raw.contains("multipart/related"));
    }

    #[test]
    fn base64_roundtrip() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        assert_eq!(base64_decode(&base64_lines(&data)).unwrap(), data);
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert!(base64_decode("a*b").is_err());
    }

    #[test]
    fn recipients_and_validation() {
        let d = draft();
        assert_eq!(d.recipients(), vec!["marie@example.com", "paul@example.org", "secret@example.net"]);
        assert!(d.validate().is_ok());
        let mut bad = d.clone();
        bad.to = vec!["pas une adresse".into()];
        assert!(bad.validate().unwrap_err().contains("pas une adresse"));
        let mut none = d;
        none.to.clear();
        none.cc.clear();
        none.bcc.clear();
        assert!(none.validate().is_err());
        assert!(!is_valid_email("a@b"));
        assert!(is_valid_email("prenom.nom+tag@sous.domaine.fr"));
    }

    #[test]
    fn date_and_header_encoding() {
        assert_eq!(format_date(1_793_610_000), "Mon, 02 Nov 2026 09:00:00 +0000");
        assert_eq!(format_date(0), "Thu, 01 Jan 1970 00:00:00 +0000");
        assert_eq!(encode_header_text("Hello"), "Hello");
        let long = "Éléments très accentués pour vérifier le découpage des mots encodés à la française";
        let encoded = encode_header_text(long);
        assert!(encoded.lines().all(|l| l.trim_end().len() <= 76), "{encoded}");
        assert_eq!(decode_words(&encoded), long);
        assert_eq!(format_mailbox("Fabrice", "f@x.fr"), "Fabrice <f@x.fr>");
        assert_eq!(format_mailbox("Dupont, Marie", "m@x.fr"), "\"Dupont, Marie\" <m@x.fr>");
        assert!(format_mailbox("Élodie", "e@x.fr").starts_with("=?UTF-8?B?"));
    }

    #[test]
    fn quoted_printable_rules() {
        assert_eq!(quoted_printable("café"), "caf=C3=A9");
        assert_eq!(quoted_printable("a = b"), "a =3D b");
        assert_eq!(quoted_printable("fin "), "fin=20");
        let long = "x".repeat(200);
        let qp = quoted_printable(&long);
        assert!(qp.split("\r\n").all(|l| l.len() <= 76));
        assert_eq!(qp_decode(&qp), long);
        let text = "Ligne 1\nLigne 2 accentuée é\n";
        assert_eq!(qp_decode(&quoted_printable(text)), text.replace('\n', "\r\n"));
    }

    #[test]
    fn html_to_plain_text() {
        let t = html_to_text(
            "<style>p{color:red}</style><p>Bonjour&nbsp;Marie,</p><p>Liste&nbsp;:</p><ul><li>un</li><li>deux</li></ul>A&amp;B<br>fin",
        );
        assert_eq!(t, "Bonjour Marie,\nListe :\n• un\n• deux\nA&B\nfin");
    }

    #[test]
    fn full_message() {
        let raw = build(&draft(), 1_793_610_000, "<id1@mailo.com>");
        let text = String::from_utf8(raw.clone()).unwrap();
        let head = text.split("\r\n\r\n").next().unwrap();
        assert!(head.contains("Date: Mon, 02 Nov 2026 09:00:00 +0000"));
        assert!(head.contains("From: Fabrice Joffre <moi@exemple.fr>"));
        assert!(head.contains("To: marie@example.com"));
        assert!(head.contains("Cc: paul@example.org"));
        assert!(!text.contains("secret@example.net"), "Cci jamais dans le message");
        assert_eq!(header_value(&raw, "Message-ID").as_deref(), Some("<id1@mailo.com>"));
        assert_eq!(header_value(&raw, "In-Reply-To").as_deref(), Some("<abc@mail.example.com>"));
        assert_eq!(header_value(&raw, "references").as_deref(), Some("<root@example.com> <abc@mail.example.com>"));
        assert_eq!(decode_words(&header_value(&raw, "Subject").unwrap()), "Réunion de rentrée — ordre du jour");
        // Toutes les lignes en CRLF et en ASCII, 998 caractères maximum.
        assert!(raw.iter().all(u8::is_ascii));
        assert!(text.split("\r\n").all(|l| l.len() <= 998 && !l.contains('\n')));
        assert!(text.contains("Content-Type: text/plain; charset=utf-8"));
        assert!(text.contains("=E2=80=A2 budget"), "puce encodée dans la partie texte");
        assert!(text.trim_end().ends_with("--"));
    }

    #[test]
    fn smtp_dot_stuffing() {
        assert_eq!(smtp_data(b"a\r\n.b\r\n..c\r\n"), b"a\r\n..b\r\n...c\r\n.\r\n");
        assert_eq!(smtp_data(b"sans fin"), b"sans fin\r\n.\r\n");
    }

    #[test]
    fn reading_raw_headers_and_addresses() {
        let raw = b"From: a@x.fr\r\nTo: \"Dupont, Marie\" <marie@x.fr>, bob@y.com;\r\n =?UTF-8?B?w4l0aWVubmU=?= <e@z.fr>\r\nReferences: <1@x>\r\n\t<2@x>\r\nSubject: s\r\n\r\nTo: pas@ici.fr\r\n";
        let to = header_value(raw, "To").unwrap();
        assert_eq!(parse_address_list(&to), vec!["marie@x.fr", "bob@y.com", "e@z.fr"]);
        assert_eq!(header_value(raw, "References").as_deref(), Some("<1@x> <2@x>"));
        assert_eq!(header_value(raw, "Cc"), None);
        assert_eq!(new_message_id("f@mailo.com", &[0xab, 0x01]), "<ab01@mailo.com>");
        assert_eq!(new_message_id("f@x.fr\r\nX: y", &[1]), "<01@vortexmail.local>");
        // Clé d'envoi : même clé, même identifiant ; clé mal formée refusée.
        let key = "00112233445566778899aabbccddeeff";
        let bytes = send_key_bytes(key).expect("clé valide");
        assert_eq!(new_message_id("f@mailo.com", &bytes), format!("<{key}@mailo.com>"), "Message-ID tiré de la clé");
        assert_eq!(send_key_bytes("xyz"), None, "trop courte");
        assert_eq!(send_key_bytes(&"g".repeat(32)), None, "pas hexadécimale");
    }

    #[test]
    fn reply_headers_cannot_open_a_new_header() {
        let mut d = draft();
        d.in_reply_to = Some("<abc@x>\rBcc: espion@example.net".into());
        d.references = Some("<r@x>\nX-Piege: 1".into());
        let raw = build(&d, 1_793_610_000, "<id1@mailo.com>");
        let text = String::from_utf8(raw.clone()).unwrap();
        assert!(text.split("\r\n").all(|l| !l.contains('\r') && !l.contains('\n')));
        assert_eq!(header_value(&raw, "Bcc"), None);
        assert_eq!(header_value(&raw, "X-Piege"), None);
        assert_eq!(header_value(&raw, "In-Reply-To").as_deref(), Some("<abc@x> Bcc: espion@example.net"));
    }

    #[test]
    fn listes_d_adresses_avec_commentaires_et_groupes() {
        // Relecture complète n° 10.
        assert_eq!(parse_address_list("jean@x.fr (Jean, Dupont), marie@y.fr"), vec!["jean@x.fr", "marie@y.fr"]);
        assert_eq!(parse_address_list("Groupe: a@x.fr, b@y.fr;"), vec!["a@x.fr", "b@y.fr"]);
        assert_eq!(parse_address_list("undisclosed-recipients:;"), Vec::<String>::new());
        assert_eq!(
            parse_address_list(r#""Dupont, Marie (RH)" <m@x.fr> (bureau \) 2), Équipe: "A; B" <a@x.fr>; c@z.fr"#),
            vec!["m@x.fr", "a@x.fr", "c@z.fr"]
        );
        assert_eq!(
            parse_address_list("x@[IPv6:2001:db8::1], y@z.fr"),
            vec!["y@z.fr"],
            "littéral refusé par is_valid_email, sans casser la suite"
        );
    }

    #[test]
    fn references_sans_doublon() {
        // Relecture complète n° 13 : l'interface ajoute déjà le parent.
        let mut d = draft();
        d.in_reply_to = Some("<p@x>".into());
        d.references = Some("<root@x> <p@x>".into());
        assert_eq!(header_value(&build(&d, 0, "<id@x>"), "References").as_deref(), Some("<root@x> <p@x>"));
        d.references = Some("<p@x> <root@x> <root@x>".into());
        assert_eq!(header_value(&build(&d, 0, "<id@x>"), "References").as_deref(), Some("<root@x> <p@x>"));
        d.references = None;
        assert_eq!(header_value(&build(&d, 0, "<id@x>"), "References").as_deref(), Some("<p@x>"));
    }

    #[test]
    fn adresses_internationales() {
        // Relecture complète n° 15 : vecteurs de la RFC 3492 et d'usage courant.
        assert_eq!(punycode("münchen").as_deref(), Some("mnchen-3ya"));
        assert_eq!(punycode("bücher").as_deref(), Some("bcher-kva"));
        assert_eq!(punycode("mañana").as_deref(), Some("maana-pta"));
        assert_eq!(punycode("ü").as_deref(), Some("tda"));
        assert_eq!(ascii_address("rene@Bücher.example").as_deref(), Some("rene@xn--bcher-kva.example"));
        assert_eq!(ascii_address("a@x.fr").as_deref(), Some("a@x.fr"));
        assert_eq!(ascii_address("rené@x.fr"), None);
        let mut d = draft();
        d.to = vec!["marie@münchen.example".into()];
        assert!(d.validate().is_ok());
        assert!(!d.needs_smtputf8(), "domaine seul : Punycode, pas de SMTPUTF8");
        d.wire_addresses();
        assert_eq!(d.to, vec!["marie@xn--mnchen-3ya.example"]);
        assert!(build(&d, 0, "<id@x>").is_ascii());
    }

    #[test]
    fn h2_partie_locale_accentuee() {
        // Lot H2 : adresse accentuée avant le @, envoyée avec SMTPUTF8 (RFC 6531).
        assert_eq!(wire_address("rené@Bücher.example").as_deref(), Some("rené@xn--bcher-kva.example"));
        assert_eq!(wire_address("sans-arobase"), None);
        let mut d = draft();
        d.to = vec!["rené@bücher.example".into()];
        d.cc = vec!["a@x.fr".into()];
        assert!(d.validate().is_ok(), "plus refusée d'office");
        assert!(d.needs_smtputf8());
        d.wire_addresses();
        assert_eq!(d.to, vec!["rené@xn--bcher-kva.example"]);
        let raw = String::from_utf8(build(&d, 0, "<id@x>")).expect("en-têtes en UTF-8 (RFC 6532)");
        let to = header_value(raw.as_bytes(), "To").unwrap_or_default();
        assert!(to.contains("rené@xn--bcher-kva.example"), "adresse écrite telle quelle, jamais en mot encodé : {to}");
        d.to = vec!["a@x.fr".into()];
        d.from_email = "élodie@x.fr".into();
        assert!(d.needs_smtputf8(), "expéditeur accentué");
    }

    #[test]
    fn lignes_d_en_tete_repliees() {
        // Relecture complète n° 16.
        let mut d = draft();
        d.subject = "mot ".repeat(300);
        let raw = String::from_utf8(build(&d, 0, "<id@x>")).unwrap();
        assert!(raw.split("\r\n").all(|l| l.len() <= 78), "objet ASCII replié");
        assert_eq!(header_value(raw.as_bytes(), "Subject").unwrap().trim(), d.subject.trim());
        d.subject = "Réunion très importante à propos des éléments accentués et des congés d'été".into();
        d.from_name = "Élodie Bérénice de la Fontaine-Saint-Éloi".into();
        let raw = String::from_utf8(build(&d, 0, "<id@x>")).unwrap();
        let head = raw.split("\r\n\r\n").next().unwrap();
        assert!(head.split("\r\n").all(|l| l.len() <= 76), "{head}");
        assert_eq!(decode_words(&header_value(raw.as_bytes(), "Subject").unwrap()), d.subject);
        // Retour à la ligne dans l'objet : jamais un nouvel en-tête.
        d.subject = "a\r\nBcc: x@y.fr".into();
        assert_eq!(header_value(&build(&d, 0, "<id@x>"), "Bcc"), None);
    }

    #[test]
    fn texte_brut_avec_liens_et_entites() {
        // Relecture complète n° 27.
        let t = html_to_text(
            r#"<p>Voir <a href="https://exemple.fr/doc?a=1&amp;b=2">ce document</a>, <a href='https://exemple.fr'>https://exemple.fr/</a> ou <a data-href="x" href="mailto:a@x.fr">a@x.fr</a>.</p><p>&eacute;t&#233; &#x20AC; &euro; &inconnu; &amp;</p>"#,
        );
        assert_eq!(t, "Voir ce document <https://exemple.fr/doc?a=1&b=2>, https://exemple.fr/ ou a@x.fr.\nété € € &inconnu; &");
        assert_eq!(html_to_text(r#"<a href="javascript:alert(1)">ici</a>"#), "ici");
    }

    #[test]
    fn date_a_l_heure_locale() {
        // Relecture complète n° 31.
        assert_eq!(format_date_at(1_793_610_000, 3600), "Mon, 02 Nov 2026 10:00:00 +0100");
        assert_eq!(format_date_at(1_793_610_000, -5 * 3600 - 1800), "Mon, 02 Nov 2026 03:30:00 -0530");
        assert_eq!(format_date_at(1_793_610_000, 90_000), format_date(1_793_610_000));
        let mut d = draft();
        d.utc_offset = 3600;
        assert_eq!(header_value(&build(&d, 1_793_610_000, "<id@x>"), "Date").as_deref(), Some("Mon, 02 Nov 2026 10:00:00 +0100"));
    }
}
