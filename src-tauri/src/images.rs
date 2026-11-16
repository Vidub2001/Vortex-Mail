//! Images distantes des messages : bloquées par défaut (pistage), téléchargées
//! à la demande puis intégrées au message en `data:` — elles s'affichent
//! ensuite à chaque ouverture, même hors connexion.

use std::collections::HashMap;

const MAX_IMAGES: usize = 80;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 25 * 1024 * 1024;
const PARALLEL: usize = 8;

/// Une adresse d'image trouvée dans le HTML : texte exact et URL décodée.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub raw: String,
    pub url: String,
}

fn is_remote(value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    v.starts_with("http://") || v.starts_with("https://") || v.starts_with("//")
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&").replace("&#38;", "&").replace("&quot;", "\"").replace("&#39;", "'")
}

/// Valeur qui suit `pos` (après `=` ou `(`) : entre guillemets, ou jusqu'au séparateur.
fn value_at(html: &str, pos: usize, closing: Option<char>) -> Option<&str> {
    let begin = pos + (html[pos..].len() - html[pos..].trim_start().len());
    let first = html[begin..].chars().next()?;
    let (start, end_char) = if first == '"' || first == '\'' { (begin + 1, Some(first)) } else { (begin, closing) };
    let tail = &html[start..];
    let end = match end_char {
        Some(c) => tail.find(c)?,
        None => tail.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(tail.len()),
    };
    Some(html[start..start + end].trim())
}

/// Adresses d'images distantes : `src=`, `background=` et `url(...)` dans les styles.
pub(crate) fn find_remote(html: &str) -> Vec<Found> {
    let lower = html.to_ascii_lowercase();
    let mut out: Vec<Found> = Vec::new();
    let mut push = |raw: &str| {
        if is_remote(raw) && !out.iter().any(|f| f.raw == raw) {
            out.push(Found { raw: raw.to_string(), url: decode_entities(raw) });
        }
    };
    for (pattern, closing) in [("src=", None), ("background=", None), ("url(", Some(')'))] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(pattern) {
            let at = from + i + pattern.len();
            // `src=` doit être un attribut, pas la fin de `data-src=` ou `srcset=`.
            let before = lower[..from + i].chars().last();
            let attribute_start = pattern == "url(" || before.is_none_or(|c| c.is_whitespace() || c == '"' || c == '\'');
            if attribute_start {
                if let Some(value) = value_at(html, at, closing) {
                    push(value);
                }
            }
            from = at;
        }
    }
    out
}

/// Message avec images distantes ? (utilisé par les tests)
#[cfg(test)]
pub(crate) fn has_remote(html: &str) -> bool {
    !find_remote(html).is_empty()
}

fn download(url: &str) -> Option<String> {
    let response = crate::http::get(url, MAX_IMAGE_BYTES).ok()?;
    if response.status != 200 || response.body.is_empty() {
        return None;
    }
    let content_type = response.header("content-type").unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let content_type = if content_type.starts_with("image/") {
        content_type
    } else {
        // Certains serveurs n'indiquent pas le type : on le devine sur les premiers octets.
        let b = &response.body;
        let sniffed = if b.starts_with(b"\x89PNG") {
            "image/png"
        } else if b.starts_with(b"\xFF\xD8") {
            "image/jpeg"
        } else if b.starts_with(b"GIF8") {
            "image/gif"
        } else if b.len() > 12 && &b[8..12] == b"WEBP" {
            "image/webp"
        } else {
            return None;
        };
        sniffed.to_string()
    };
    Some(format!("data:{content_type};base64,{}", mail_core::compose::base64(&response.body)))
}

/// Télécharge les images distantes et les intègre au HTML. Les images
/// introuvables restent des liens (affichées en ligne si autorisées).
pub(crate) fn inline_remote(html: &str) -> (String, usize) {
    let found: Vec<Found> = find_remote(html).into_iter().take(MAX_IMAGES).collect();
    let mut data: HashMap<String, String> = HashMap::new();
    for chunk in found.chunks(PARALLEL) {
        let results: Vec<(String, Option<String>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk.iter().map(|f| scope.spawn(move || (f.raw.clone(), download(&f.url)))).collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        for (raw, uri) in results {
            if let Some(uri) = uri {
                data.insert(raw, uri);
            }
        }
        if data.values().map(String::len).sum::<usize>() > MAX_TOTAL_BYTES * 4 / 3 {
            break;
        }
    }
    // Chaque occurrence de l'adresse reçoit l'image entière : le plafond
    // compte toutes les copies (relecture du 10 novembre 2026).
    let mut out = html.to_string();
    let (mut total, mut inlined) = (0usize, 0usize);
    for (raw, uri) in &data {
        let cost = uri.len().saturating_mul(out.matches(raw.as_str()).count());
        if total.saturating_add(cost) > MAX_TOTAL_BYTES * 4 / 3 {
            continue;
        }
        total += cost;
        out = out.replace(raw.as_str(), uri);
        inlined += 1;
    }
    (out, inlined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_remote_images_only() {
        let html = r#"<img src="https://a.fr/x.png?a=1&amp;b=2"><img data-src="https://no.fr/1.png">
            <img src='http://b.fr/y.gif'><img src="cid:logo@x"><img src="data:image/png;base64,AA">
            <td background="https://c.fr/bg.jpg" style="background:url('https://d.fr/z.png')">
            <img src=https://e.fr/u.png alt=x>"#;
        let found = find_remote(html);
        let urls: Vec<&str> = found.iter().map(|f| f.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://a.fr/x.png?a=1&b=2", "http://b.fr/y.gif", "https://e.fr/u.png", "https://c.fr/bg.jpg", "https://d.fr/z.png"]
        );
        assert_eq!(found[0].raw, "https://a.fr/x.png?a=1&amp;b=2");
        assert!(!has_remote(r#"<p>Bonjour</p><img src="cid:x">"#));
    }
}
