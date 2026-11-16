//! Décodage MIME des vrais messages, via la crate `mail-parser`.
//! Tout l'usage de `mail-parser` est regroupé ici.

use std::collections::{HashMap, HashSet};

use mail_core::AttachmentInfo;
use mail_parser::{MessageParser, MessagePart};

/// Contenu d'une pièce jointe, prêt à être enregistré.
pub struct AttachmentData {
    pub name: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

/// N'affiche que la taille du contenu, pas les octets.
impl std::fmt::Debug for AttachmentData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachmentData")
            .field("name", &self.name)
            .field("content_type", &self.content_type)
            .field("len", &self.data.len())
            .finish()
    }
}

/// Taille maximale des images intégrées converties en `data:` pour l'affichage.
const INLINE_BUDGET: usize = 8 * 1024 * 1024;

/// Plafonds de lecture de `mail-parser` (1.0) : imbrication des parties,
/// nombre de parties, messages encodés dans des messages. Au-delà, le reste
/// du mail reste dans une dernière partie, marquée par la crate ; rien n'est
/// perdu ni rejeté. Valeurs plus basses que celles de la crate (64, 10 000,
/// 3) : aucun vrai mail n'en approche.
const MAX_DEPTH: usize = 32;
const MAX_PARTS: usize = 1_000;
const MAX_ENCODED_NESTING: usize = 3;

fn parser() -> MessageParser {
    MessageParser::new().max_depth(MAX_DEPTH).max_parts(MAX_PARTS).max_encoded_nesting(MAX_ENCODED_NESTING)
}

fn mime_type(part: MessagePart<'_>) -> String {
    part.content_type()
        .map_or_else(
            || "application/octet-stream".to_string(),
            |c| match c.subtype() {
                Some(s) => format!("{}/{}", c.ctype(), s),
                None => c.ctype().to_string(),
            },
        )
        .to_ascii_lowercase()
}

fn extension(content_type: &str) -> &'static str {
    match content_type {
        "application/pdf" => ".pdf",
        "image/jpeg" | "image/jpg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "text/plain" => ".txt",
        "text/html" => ".html",
        "text/calendar" => ".ics",
        "message/rfc822" => ".eml",
        _ => "",
    }
}

/// Nom affiché : celui du message, sinon l'objet d'un message joint, sinon un nom générique.
/// Nettoyé dès l'affichage : « photo » + U+202E + « gpj.exe » se lirait
/// « photoexe.jpg » (relecture du 10 novembre 2026).
fn part_name(part: MessagePart<'_>, position: usize, content_type: &str) -> String {
    if let Some(name) = part.attachment_name().map(str::trim).filter(|n| !n.is_empty()) {
        return crate::attachments::safe_file_name(name);
    }
    if let Some(subject) = part.nested().and_then(|m| m.subject()).map(str::trim).filter(|s| !s.is_empty()) {
        return crate::attachments::safe_file_name(&format!("{subject}.eml"));
    }
    format!("{}{}", tr!("pièce jointe {n}", n = position + 1), extension(content_type))
}

/// `mail-parser` a paniqué sur un mail abîmé (fuzzing du 10 novembre 2026 :
/// `message/rfc822` imbriqué presque vide). Une panique de la crate est
/// rattrapée : le message est traité comme illisible, au lieu de faire tomber
/// le fil qui le lisait (ouverture, préchargement hors connexion).
///
/// Écart voulu à la règle « pas de `catch_unwind` » (lot 10) : seulement autour
/// de cette crate externe, et le résultat partiel est jeté. Suppose une
/// compilation qui déroule la pile (réglage par défaut, à ne pas changer).
fn guarded<T>(read: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)).ok()
}

/// Contenu d'une pièce jointe désignée par sa position (voir `AttachmentInfo::index`).
pub(crate) fn attachment(raw: &[u8], index: u32) -> Option<AttachmentData> {
    guarded(|| {
        let m = parser().parse(raw)?;
        let position = usize::try_from(index).ok()?;
        let part = m.attachments().nth(position)?;
        let content_type = mime_type(part);
        Some(AttachmentData { name: part_name(part, position, &content_type), data: part.decoded().into_owned(), content_type })
    })
    .flatten()
}

#[derive(Debug, Default, Clone)]
pub(crate) struct Parsed {
    pub from_name: String,
    pub from_email: String,
    pub to: String,
    pub subject: String,
    pub date: Option<i64>,
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
}

/// Message décodé ; illisible si `mail-parser` le refuse ou panique.
pub(crate) fn parse(raw: &[u8]) -> Parsed {
    guarded(|| parse_unguarded(raw))
        .flatten()
        .unwrap_or_else(|| Parsed { subject: tr!("(message illisible)").into(), ..Default::default() })
}

fn parse_unguarded(raw: &[u8]) -> Option<Parsed> {
    let m = parser().parse(raw)?;

    let (from_name, from_email) = m.from().and_then(|a| a.first()).map_or_default(|a| {
        let email = a.address().unwrap_or_default().to_string();
        let name = a.name().map_or_else(|| email.clone(), std::string::ToString::to_string);
        (name, email)
    });

    let to = m.to().and_then(|a| a.first()).map_or_default(|a| match (a.name(), a.address()) {
        (Some(n), Some(e)) => format!("{n} <{e}>"),
        (None, Some(e)) => e.to_string(),
        (Some(n), None) => n.to_string(),
        (None, None) => String::new(),
    });

    let mut html = m.body_html(0).map(std::borrow::Cow::into_owned);
    // Images intégrées au corps (`<img src="cid:…">`) : affichées, pas
    // listées. Depuis la 1.0, une image montrée dans le corps n'est plus
    // aussi une pièce jointe : on les cherche dans toutes les parties du
    // message (pas dans celles d'un message joint).
    let mut shown = Vec::new();
    let mut budget = INLINE_BUDGET;
    if let Some(body) = html.as_mut() {
        // Références `cid:` entières, décodées (RFC 2392) : `cid:img1` ne
        // touche plus le début de `cid:img10` (relecture complète n° 14).
        let refs = cid_refs(body);
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for (_, _, id) in &refs {
            *counts.entry(id.as_str()).or_default() += 1;
        }
        let mut uris: HashMap<String, String> = HashMap::new();
        let mut seen: HashSet<String> = HashSet::new();
        for part in m.root().parts() {
            let Some(cid) = part.content_id() else { continue };
            let cid = cid.trim().trim_start_matches('<').trim_end_matches('>');
            let uses = counts.get(cid).copied().unwrap_or(0);
            if uses == 0 || !seen.insert(cid.to_string()) {
                continue;
            }
            shown.push(part.id());
            let content_type = mime_type(part);
            // Chaque occurrence reçoit l'image entière : le budget compte
            // toutes les copies (20 000 fois la même référence demandaient
            // des dizaines de Go, relecture du 10 novembre 2026), et il est
            // vérifié avant tout décodage.
            let cost = part.decoded_len().saturating_mul(uses);
            if content_type.starts_with("image/") && cost <= budget {
                budget -= cost;
                let uri = format!("data:{content_type};base64,{}", mail_core::compose::base64(&part.decoded()));
                uris.insert(cid.to_string(), uri);
            }
        }
        if !uris.is_empty() {
            *body = replace_cids(body, &refs, &uris);
        }
    }
    let mut attachments = Vec::new();
    for (position, part) in m.attachments().enumerate() {
        if shown.contains(&part.id()) {
            continue;
        }
        let content_type = mime_type(part);
        attachments.push(AttachmentInfo {
            index: u32::try_from(position).unwrap_or(u32::MAX),
            name: part_name(part, position, &content_type),
            size: u64::try_from(part.decoded_len()).unwrap_or(u64::MAX),
            content_type,
        });
    }

    Some(Parsed {
        from_name,
        from_email,
        to,
        // Objet vide : l'interface affiche « (sans objet) » dans la langue choisie.
        subject: m.subject().unwrap_or("").to_string(),
        date: m.date().map(|d| d.to_timestamp()),
        html,
        text: m.body_text(0).map(std::borrow::Cow::into_owned),
        attachments,
    })
}

/// Références `cid:` d'un corps HTML : (début, fin, identifiant décodé des
/// `%xx`). La référence s'arrête au premier guillemet, parenthèse, chevron
/// ou espace : c'est une valeur d'attribut ou d'`url()` entière.
fn cid_refs(body: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let lower = body.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find("cid:") {
        let start = from + found;
        let after = start + 4;
        from = after;
        // `acid:` dans le texte n'est pas une référence.
        if lower[..start].chars().next_back().is_some_and(char::is_alphanumeric) {
            continue;
        }
        let rest = &body[after..];
        let len = rest.find(|c: char| matches!(c, '"' | '\'' | ')' | '>' | '<') || c.is_whitespace()).unwrap_or(rest.len());
        if len > 0 {
            out.push((start, after + len, percent_decode(&rest[..len])));
            from = after + len;
        }
    }
    out
}

/// `%xx` décodés (octets, puis UTF-8 avec remplacement).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Corps où chaque référence connue est remplacée par son adresse `data:`.
fn replace_cids(body: &str, refs: &[(usize, usize, String)], uris: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut copied = 0;
    for (start, end, id) in refs {
        if let Some(uri) = uris.get(id) {
            out.push_str(&body[copied..*start]);
            out.push_str(uri);
            copied = *end;
        }
    }
    out.push_str(&body[copied..]);
    out
}

/// Texte resté en base64, décodé ; `None` pour un texte ordinaire. Reconnu
/// strictement : chaque mot n'a que des caractères base64, et l'un d'eux au
/// moins fait 40 caractères (un aperçu en cache est fait de mots séparés par
/// des espaces, et peut finir par « … »).
pub(crate) fn decode_if_base64(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches('…');
    let base64 = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=');
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || !words.iter().all(|w| w.chars().all(base64)) || !words.iter().any(|w| w.len() >= 40) {
        return None;
    }
    let bytes = mail_core::compose::base64_decode(&words.concat()).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Repère `multipart/mixed` (pièces jointes probables) dans un bloc d'en-têtes.
pub(crate) fn looks_like_attachment(headers: &[u8]) -> bool {
    let text = String::from_utf8_lossy(headers).to_ascii_lowercase();
    text.lines().any(|l| l.starts_with("content-type:") && l.contains("multipart/mixed"))
}

/// HTML affichable : le HTML du message, ou son texte brut échappé.
pub(crate) fn display_html(p: &Parsed) -> String {
    if let Some(html) = &p.html {
        return html.clone();
    }
    let text = p.text.as_deref().unwrap_or("");
    let escaped = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!("<pre style=\"white-space:pre-wrap;font-family:inherit;margin:0\">{escaped}</pre>")
}

/// Texte d'aperçu pour la liste.
pub(crate) fn preview(p: &Parsed) -> String {
    match (&p.text, &p.html) {
        (Some(t), _) if !t.trim().is_empty() => mail_core::mime::preview(&t.replace('<', " "), 140),
        (_, Some(h)) => mail_core::mime::preview(h, 140),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    #[test]
    fn h2_adresses_en_utf8() {
        // Lot H2 : en-têtes en UTF-8 (RFC 6532), comme les écrit un envoi SMTPUTF8.
        let raw = "From: Ren\u{e9} <ren\u{e9}@b\u{fc}cher.example>\r\nTo: \u{e9}lodie@x.fr\r\nSubject: Caf\u{e9}\r\n\r\nBonjour\r\n";
        let p = parse(raw.as_bytes());
        assert_eq!(p.from_email, "rené@bücher.example");
        assert_eq!(p.from_name, "René");
        assert_eq!(p.to, "élodie@x.fr");
        assert_eq!(p.subject, "Café");
    }

    #[test]
    fn jeux_de_caracteres_hors_unicode_decodes() {
        // « 日本 » en Shift_JIS, et « Привет » en KOI8-R.
        let mut raw = b"From: a@x.jp\r\nSubject: s\r\nContent-Type: text/plain; charset=shift_jis\r\n\r\n".to_vec();
        raw.extend_from_slice(&[0x93, 0xfa, 0x96, 0x7b]);
        assert_eq!(parse(&raw).text.as_deref().map(str::trim), Some("日本"));
        let mut raw = b"From: a@x.ru\r\nSubject: s\r\nContent-Type: text/plain; charset=koi8-r\r\n\r\n".to_vec();
        raw.extend_from_slice(&[0xf0, 0xd2, 0xc9, 0xd7, 0xc5, 0xd4]);
        assert_eq!(parse(&raw).text.as_deref().map(str::trim), Some("Привет"));
    }

    #[test]
    fn parties_et_imbrication_plafonnees() {
        let mut raw =
            String::from("From: a@x.fr\r\nSubject: s\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n");
        for i in 0..3_000 {
            let _ = write!(
                raw,
                "--B\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"f{i}.bin\"\r\n\r\nx\r\n"
            );
        }
        raw.push_str("--B--\r\n");
        let parsed = parse(raw.as_bytes());
        assert!(parsed.attachments.len() >= 100, "{} pièces jointes", parsed.attachments.len());
        assert!(parsed.attachments.len() <= MAX_PARTS, "{} pièces jointes", parsed.attachments.len());
        // 200 multiparts imbriqués : lu sans déborder la pile.
        let mut deep = String::from("From: a@x.fr\r\nSubject: s\r\nMIME-Version: 1.0\r\n");
        for i in 0..200 {
            let _ = write!(deep, "Content-Type: multipart/mixed; boundary=\"N{i}\"\r\n\r\n--N{i}\r\n");
        }
        deep.push_str("Content-Type: text/plain\r\n\r\nfond\r\n");
        assert_eq!(parse(deep.as_bytes()).subject, "s");
    }

    fn related(body: &str, image: &[u8]) -> String {
        let image = mail_core::compose::base64(image);
        format!(
            "From: a@exemple.fr\r\nSubject: x\r\nMIME-Version: 1.0\r\n\
             Content-Type: multipart/related; boundary=\"B\"\r\n\r\n\
             --B\r\nContent-Type: text/html\r\n\r\n{body}\r\n\
             --B\r\nContent-Type: image/png\r\nContent-ID: <a>\r\nContent-Transfer-Encoding: base64\r\n\r\n{image}\r\n--B--\r\n"
        )
    }

    #[test]
    fn inline_image_is_shown() {
        let parsed = parse(related("<p><img src=\"cid:a\"></p>", &[0u8; 64]).as_bytes());
        let html = parsed.html.unwrap();
        assert!(html.contains("data:image/png;base64,"), "{html}");
        assert!(!html.contains("cid:a"));
    }

    #[test]
    fn references_cid_entieres_et_decodees() {
        // Relecture complète n° 14 : `cid:a` ne touche pas `cid:ab`, `%40` est décodé.
        let body = "<img src=\"cid:ab\"><img src='cid:a'><div style=\"background:url(cid:a)\"></div>";
        let refs = cid_refs(body);
        assert_eq!(refs.iter().map(|r| r.2.as_str()).collect::<Vec<_>>(), vec!["ab", "a", "a"]);
        let out = replace_cids(body, &refs, &HashMap::from([("a".to_string(), "data:x".to_string())]));
        assert_eq!(out, "<img src=\"cid:ab\"><img src='data:x'><div style=\"background:url(data:x)\"></div>");
        assert_eq!(cid_refs("<img src=\"cid:img%401.x\">")[0].2, "img@1.x");
        assert_eq!(cid_refs("un acid:a dans le texte").len(), 0, "pas une référence");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        let parsed = parse(related("<img src=\"cid:ab\"><img src=\"cid:a\">", &[0u8; 8]).as_bytes());
        let html = parsed.html.unwrap();
        assert!(html.contains("cid:ab") && html.contains("data:image/png"), "{html}");
    }

    #[test]
    fn repeated_inline_image_is_counted_for_each_use() {
        // 5 000 copies de 4 Kio dépasseraient le budget : l'image n'est pas intégrée.
        let body = "<img src=\"cid:a\">".repeat(5_000);
        let parsed = parse(related(&body, &[0u8; 4096]).as_bytes());
        let html = parsed.html.unwrap();
        assert!(html.len() < INLINE_BUDGET, "{}", html.len());
        assert!(html.contains("cid:a"));
    }
}
