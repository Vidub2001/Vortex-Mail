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
}
