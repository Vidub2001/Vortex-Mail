//! Analyse MIME volontairement minimale, suffisante pour les données de démo.
//! Le vrai client utilisera la crate `mail-parser` (encodages, multipart,
//! en-têtes repliés, RFC 2047…). L'interface de ce module restera la même.

/// Ce que l'affichage retient d'un message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedMail {
    /// Nom affiché de l'expéditeur.
    pub from_name: String,
    /// Adresse de l'expéditeur.
    pub from_email: String,
    /// En-tête `To`, tel quel.
    pub to: String,
    /// Objet, tel que lu (sans décodage RFC 2047 dans cet analyseur minimal).
    pub subject: String,
    /// Corps en HTML.
    pub body_html: String,
    /// Vrai si le message a des pièces jointes.
    pub has_attachments: bool,
}

/// Analyse un message RFC 822 ; ne peut pas échouer (valeurs vides si illisible).
#[must_use]
pub fn parse(bytes: &[u8]) -> ParsedMail {
    let text = String::from_utf8_lossy(bytes);
    let (head, body) = split_header_body(&text);

    let mut out = ParsedMail { body_html: body.to_string(), ..Default::default() };
    let mut from = String::new();
    for line in head.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "from" => from = value.to_string(),
            "to" => out.to = value.to_string(),
            "subject" => out.subject = value.to_string(),
            "content-type" => {
                out.has_attachments = value.to_ascii_lowercase().starts_with("multipart/mixed");
            }
            _ => {}
        }
    }
    let (name, email) = parse_address(&from);
    out.from_name = name;
    out.from_email = email;
    out
}

fn split_header_body(text: &str) -> (&str, &str) {
    if let Some(i) = text.find("\r\n\r\n") {
        (&text[..i], &text[i + 4..])
    } else if let Some(i) = text.find("\n\n") {
        (&text[..i], &text[i + 2..])
    } else {
        (text, "")
    }
}

/// `"Marie Dupont" <marie@example.com>` → `("Marie Dupont", "marie@example.com")`
#[must_use]
pub fn parse_address(s: &str) -> (String, String) {
    if let (Some(l), Some(r)) = (s.find('<'), s.rfind('>')) {
        if l < r {
            let name = s[..l].trim().trim_matches('"').trim().to_string();
            let email = s[l + 1..r].trim().to_string();
            let name = if name.is_empty() { email.clone() } else { name };
            return (name, email);
        }
    }
    let email = s.trim().to_string();
    (email.clone(), email)
}

/// Texte d'aperçu pour la liste : balises retirées, espaces compactés, tronqué.
#[must_use]
pub fn preview(html: &str, max_chars: usize) -> String {
    let html = without_hidden_blocks(html);
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        compact
    } else {
        let mut cut: String = compact.chars().take(max_chars).collect();
        cut.push('…');
        cut
    }
}

/// Éléments dont le contenu n'est jamais affiché : leur texte n'a rien à faire
/// dans un aperçu (« `table * { line-height: normal; …` » sous l'objet d'un
/// message HTML, vu le 18 novembre 2026).
const HIDDEN_BLOCKS: [&str; 4] = ["head", "style", "script", "title"];

/// HTML sans le contenu des éléments de `HIDDEN_BLOCKS`, en un seul passage.
/// Un bloc sans fin (début de message coupé) est retiré jusqu'au bout.
fn without_hidden_blocks(html: &str) -> String {
    // Minuscules ASCII : mêmes positions d'octets que le texte d'origine.
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while let Some(found) = lower[i..].find('<') {
        let start = i + found;
        let after = &lower[start + 1..];
        // Nom complet seulement : `<header>` n'est pas `<head>`.
        let block = HIDDEN_BLOCKS.into_iter().find(|name| {
            after.starts_with(name) && after[name.len()..].chars().next().is_none_or(|c| c == '>' || c == '/' || c.is_ascii_whitespace())
        });
        if let Some(name) = block {
            out.push_str(&html[i..start]);
            out.push(' ');
            let close = format!("</{name}");
            i = lower[start + 1..].find(&close).map_or(html.len(), |end| {
                let from = start + 1 + end;
                lower[from..].find('>').map_or(html.len(), |gt| from + gt + 1)
            });
        } else {
            out.push_str(&html[i..=start]);
            i = start + 1;
        }
    }
    out.push_str(&html[i..]);
    out
}

/// Construit un message RFC 822 simple (corps HTML), utilisé par les données de démo.
#[must_use]
pub fn build(from_name: &str, from_email: &str, to: &str, subject: &str, html: &str, with_attachment: bool) -> Vec<u8> {
    let content_type = if with_attachment { "multipart/mixed; boundary=\"demo\"" } else { "text/html; charset=utf-8" };
    format!(
        "From: \"{from_name}\" <{from_email}>\r\n\
         To: {to}\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: {content_type}\r\n\
         \r\n\
         {html}"
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_build_parse() {
        let raw = build("Marie Dupont", "marie@example.com", "moi@example.org", "Bonjour", "<p>Salut</p>", true);
        let p = parse(&raw);
        assert_eq!(p.from_name, "Marie Dupont");
        assert_eq!(p.from_email, "marie@example.com");
        assert_eq!(p.to, "moi@example.org");
        assert_eq!(p.subject, "Bonjour");
        assert_eq!(p.body_html, "<p>Salut</p>");
        assert!(p.has_attachments);
    }

    #[test]
    fn address_without_name() {
        assert_eq!(parse_address("a@b.c"), ("a@b.c".into(), "a@b.c".into()));
        assert_eq!(parse_address("<a@b.c>"), ("a@b.c".into(), "a@b.c".into()));
    }

    #[test]
    fn preview_strips_and_truncates() {
        assert_eq!(preview("<p>Bonjour&nbsp;<b>toi</b></p>\n<p>ça va ?</p>", 100), "Bonjour toi ça va ?");
        assert_eq!(preview("abcdef", 3), "abc…");
    }

    #[test]
    fn preview_without_style_or_script() {
        let html = "<html><head><title>Annonce</title><style type=\"text/css\">table * { line-height: normal; }</style></head>\
                    <body><header>En-tête</header><script>alert(1)</script><p>Nouvelle annonce de Sam</p></body></html>";
        assert_eq!(preview(html, 100), "En-tête Nouvelle annonce de Sam");
        // Début de message coupé au milieu de la feuille de style : rien plutôt que du CSS.
        assert_eq!(preview("<STYLE>td { color: red; ", 100), "");
        // `<style/>` ouvre quand même un bloc de style (HTML) : la suite est retirée.
        assert_eq!(preview("<p>avant</p><style/>après", 100), "avant");
    }
}
