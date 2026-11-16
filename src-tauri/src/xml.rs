//! Lecteur XML minimal pour les réponses `WebDAV` (`CardDAV`, `CalDAV`) : arbre
//! d'éléments, noms sans préfixe avec l'adresse de leur espace de noms,
//! entités et CDATA décodés.
//! Volontairement tolérant : un document mal formé donne un arbre partiel.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Element {
    /// Nom local, sans préfixe (`d:href` → `href`), en minuscules.
    pub name: String,
    /// Adresse de l'espace de noms (`DAV:`), vide si aucun n'est déclaré.
    pub ns: String,
    pub children: Vec<Element>,
    pub text: String,
    /// Attributs hors déclarations d'espace de noms : (nom local en
    /// minuscules, valeur décodée).
    pub attrs: Vec<(String, String)>,
}

impl Element {
    /// Valeur d'un attribut (nom local, sans préfixe).
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Premier descendant direct de ce nom.
    pub(crate) fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Élément de l'espace de noms `DAV:` (ou sans espace de noms déclaré,
    /// par tolérance) : un élément d'une extension qui porte le même nom
    /// local n'est pas pris pour lui (RFC 4918 §17, relecture complète n° 46).
    pub(crate) fn is_dav(&self, name: &str) -> bool {
        self.name == name && (self.ns == DAV || self.ns.is_empty())
    }

    /// Premier enfant direct `DAV:` de ce nom.
    pub(crate) fn dav_child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.is_dav(name))
    }

    /// Tous les descendants `DAV:` (à toute profondeur) de ce nom.
    pub(crate) fn dav_descendants(&self, name: &str) -> Vec<&Element> {
        self.descendants(name).into_iter().filter(|e| e.is_dav(name)).collect()
    }

    /// Tous les descendants (à toute profondeur) de ce nom.
    pub(crate) fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Element>) {
        for c in &self.children {
            if c.name == name {
                out.push(c);
            }
            c.find_all(name, out);
        }
    }

    pub(crate) fn descendants(&self, name: &str) -> Vec<&Element> {
        let mut out = Vec::new();
        self.find_all(name, &mut out);
        out
    }

    /// Premier descendant (à toute profondeur) de ce nom.
    pub(crate) fn find(&self, name: &str) -> Option<&Element> {
        for c in &self.children {
            if c.name == name {
                return Some(c);
            }
            if let Some(found) = c.find(name) {
                return Some(found);
            }
        }
        None
    }

    /// Texte de l'élément et de ses descendants, sans espaces autour.
    pub(crate) fn text_content(&self) -> String {
        let mut s = self.text.clone();
        for c in &self.children {
            s.push_str(&c.text_content());
        }
        s.trim().to_string()
    }
}

/// Profondeur d'imbrication la plus grande gardée. L'arbre est ensuite
/// parcouru (et libéré) récursivement : sans plafond, un serveur d'agenda ou
/// de contacts qui renvoie 200 000 balises ouvrantes ferait déborder la pile
/// et fermer l'application (fuzzing, lot 8). Au-delà, les balises sont
/// ignorées et leur texte va à l'élément le plus profond.
const MAX_DEPTH: usize = 256;

/// Espace de noms de `WebDAV`.
pub(crate) const DAV: &str = "DAV:";

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_ascii_lowercase()
}

/// Préfixe d'un nom (`d:href` → `d`), vide sans préfixe.
fn prefix(name: &str) -> &str {
    name.rsplit_once(':').map_or("", |(p, _)| p)
}

/// Attributs d'une balise (après son nom) : (nom tel qu'écrit, valeur
/// décodée). Lecture attribut par attribut, guillemets compris : `xmlns` dans
/// la valeur d'un autre attribut n'est pas pris pour une déclaration
/// (relecture du lot F4). S'arrête au premier attribut mal formé.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(start) = tag.find(char::is_whitespace) else { return out };
    let mut rest = &tag[start..];
    loop {
        rest = rest.trim_start();
        let Some(eq) = rest.find('=') else { break };
        let name = rest[..eq].trim();
        if name.is_empty() || name.contains(char::is_whitespace) {
            break;
        }
        let value_part = rest[eq + 1..].trim_start();
        let Some(q) = value_part.chars().next().filter(|q| *q == '"' || *q == '\'') else { break };
        let body = &value_part[1..];
        let Some(end) = body.find(q) else { break };
        out.push((name.to_string(), decode_entities(&body[..end])));
        rest = &body[end + 1..];
    }
    out
}

/// Déclarations `xmlns` et `xmlns:p` parmi des attributs : (préfixe, adresse).
fn declarations(attrs: &[(String, String)]) -> Vec<(String, String)> {
    attrs
        .iter()
        .filter_map(|(name, uri)| {
            let declared = if name == "xmlns" { Some("") } else { name.strip_prefix("xmlns:") };
            declared.map(|p| (p.to_string(), uri.clone()))
        })
        .collect()
}

/// Caractère admis dans un document XML (XML 1.0 §2.2) : une référence
/// `&#0;` ou à un autre caractère de contrôle n'est pas décodée.
fn xml_char(c: char) -> bool {
    (matches!(c, '\t' | '\n' | '\r') || !c.is_control()) && !matches!(c, '\u{FFFE}' | '\u{FFFF}')
}

pub(crate) fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        // `;` cherché dans les 11 premiers octets seulement : chercher plus
        // loin rendait la lecture quadratique (400 000 « & » : plusieurs
        // secondes, relecture du 10 novembre 2026).
        let Some(end) = tail.bytes().take(11).position(|b| b == b';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        }
        .filter(|c| xml_char(*c));
        if let Some(c) = decoded {
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

pub(crate) fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Analyse un document ; renvoie un élément racine artificiel contenant les éléments de premier niveau.
pub(crate) fn parse(input: &str) -> Element {
    let mut stack: Vec<Element> = vec![Element { name: "#document".into(), ..Default::default() }];
    // Espaces de noms déclarés par chaque élément ouvert (parallèle à `stack`).
    let mut scopes: Vec<Vec<(String, String)>> = vec![Vec::new()];
    let mut rest = input;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&decode_entities(rest));
            }
            break;
        };
        if lt > 0 {
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&decode_entities(&rest[..lt]));
            }
        }
        rest = &rest[lt..];
        if let Some(after) = rest.strip_prefix("<![CDATA[") {
            let end = after.find("]]>").unwrap_or(after.len());
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&after[..end]);
            }
            rest = after.get(end + 3..).unwrap_or("");
            continue;
        }
        if let Some(after) = rest.strip_prefix("<!--") {
            let end = after.find("-->").map_or(after.len(), |e| e + 3);
            rest = &after[end..];
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            let end = rest.find('>').map_or(rest.len(), |e| e + 1);
            rest = &rest[end..];
            continue;
        }
        // Balise : fin au premier `>` hors guillemets.
        let bytes = rest.as_bytes();
        let mut quote = 0u8;
        let mut end = None;
        for (i, &b) in bytes.iter().enumerate().skip(1) {
            if quote != 0 {
                if b == quote {
                    quote = 0;
                }
            } else if b == b'"' || b == b'\'' {
                quote = b;
            } else if b == b'>' {
                end = Some(i);
                break;
            }
        }
        let Some(end) = end else { break };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            let name = local(name.trim());
            // Ferme jusqu'à l'élément correspondant (tolérant).
            if let Some(pos) = stack.iter().rposition(|e| e.name == name) {
                if pos > 0 {
                    while stack.len() > pos {
                        let done = stack.pop().expect("pile non vide");
                        scopes.pop();
                        stack.last_mut().expect("parent").children.push(done);
                    }
                }
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let name = tag.split_whitespace().next().unwrap_or("");
        let all = attributes(tag);
        let declared = declarations(&all);
        let wanted = prefix(name);
        let ns = declared.iter().chain(scopes.iter().rev().flatten()).find(|(p, _)| p == wanted).map_or_default(|(_, uri)| uri.clone());
        let attrs = all.into_iter().filter(|(k, _)| k != "xmlns" && !k.starts_with("xmlns:")).map(|(k, v)| (local(&k), v)).collect();
        let element = Element { name: local(name), ns, attrs, ..Default::default() };
        if self_closing {
            stack.last_mut().expect("racine").children.push(element);
        } else if stack.len() <= MAX_DEPTH {
            stack.push(element);
            scopes.push(declared);
        }
    }
    while stack.len() > 1 {
        let done = stack.pop().expect("pile non vide");
        stack.last_mut().expect("racine").children.push(done);
    }
    stack.pop().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn depth(e: &Element) -> usize {
        1 + e.children.iter().map(depth).max().unwrap_or(0)
    }

    #[test]
    fn entities_are_read_in_linear_time() {
        let text = "&".repeat(400_000);
        assert_eq!(decode_entities(&text), text);
        assert_eq!(decode_entities("a &amp; b &#x41;&#66; &inconnu; &"), "a & b AB &inconnu; &");
    }

    #[test]
    fn endless_nesting_is_capped() {
        let doc = format!("{}texte{}", "<d:prop>".repeat(200_000), "</d:prop>".repeat(200_000));
        let root = parse(&doc);
        assert!(depth(&root) <= MAX_DEPTH + 1);
        assert_eq!(root.text_content(), "texte");
        assert_eq!(root.descendants("prop").len(), MAX_DEPTH);
        // Libéré sans débordement de pile, même dans un thread à petite pile.
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || drop(parse(&doc)))
            .expect("thread")
            .join()
            .expect("pas de débordement");
    }

    #[test]
    fn multistatus() {
        let doc = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <d:response>
    <d:href>/dav/contacts/a.vcf</d:href>
    <d:propstat><d:prop><d:getetag>"123"</d:getetag>
      <card:address-data><![CDATA[BEGIN:VCARD
FN:Marie & Co
END:VCARD]]></card:address-data></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response><d:href>/dav/contacts/b.vcf</d:href><d:propstat><d:prop>
    <d:resourcetype/><d:displayname>L&apos;agenda &amp; co</d:displayname></d:prop></d:propstat></d:response>
</d:multistatus>"#;
        let root = parse(doc);
        let responses = root.descendants("response");
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0].child("href").unwrap().text_content(), "/dav/contacts/a.vcf");
        assert_eq!(responses[0].find("getetag").unwrap().text_content(), "\"123\"");
        assert!(responses[0].find("address-data").unwrap().text_content().contains("Marie & Co"));
        assert_eq!(responses[1].find("displayname").unwrap().text_content(), "L'agenda & co");
        assert!(responses[1].find("resourcetype").is_some());
        assert_eq!(responses[0].ns, DAV, "espace de noms retrouvé par le préfixe");
    }

    #[test]
    fn extension_homonyme_ignoree() {
        // Relecture complète n° 46 : un `href` d'une extension, placé avant le
        // `DAV:href`, n'est pas pris pour l'adresse de la réponse.
        let doc = r#"<d:multistatus xmlns:d="DAV:"><d:response xmlns:x="urn:autre">
            <x:href>https://ailleurs.example/</x:href><d:href>/cal/a.ics</d:href></d:response></d:multistatus>"#;
        let root = parse(doc);
        let response = root.dav_descendants("response")[0];
        assert_eq!(response.dav_child("href").map(Element::text_content).as_deref(), Some("/cal/a.ics"), "seul DAV:href compte");
        // Espace de noms par défaut, et document sans aucune déclaration (tolérance).
        let plain = parse(r#"<multistatus xmlns="DAV:"><response><href>/a</href></response></multistatus>"#);
        assert_eq!(plain.dav_descendants("href").len(), 1, "espace de noms par défaut");
        assert_eq!(parse("<response><href>/a</href></response>").dav_descendants("href").len(), 1, "sans déclaration");
        // Caractère interdit en XML : la référence n'est pas décodée.
        assert_eq!(decode_entities("a&#0;b&#x1;c"), "a&#0;b&#x1;c", "NUL et contrôles refusés");
        assert_eq!(decode_entities("&#xFFFF;"), "&#xFFFF;");
    }

    #[test]
    fn attributs() {
        // `xmlns` dans la valeur d'un autre attribut : pas une déclaration (relecture du lot F4).
        let doc = r#"<r xmlns:c="urn:ietf:params:xml:ns:caldav"><x title='a xmlns:d="DAV:"'><d:href>/a</d:href></x>
            <c:comp name="VEVENT" C:Extra = "1&amp;2"/></r>"#;
        let root = parse(doc);
        let href = root.find("href").expect("href");
        assert_eq!(href.ns, "", "préfixe d non déclaré");
        let comp = root.find("comp").expect("comp");
        assert_eq!(
            (comp.ns.as_str(), comp.attr("name"), comp.attr("extra")),
            ("urn:ietf:params:xml:ns:caldav", Some("VEVENT"), Some("1&2"))
        );
        assert_eq!(root.find("x").and_then(|x| x.attr("title")), Some("a xmlns:d=\"DAV:\""));
        assert!(root.find("r").is_some_and(|r| r.attrs.is_empty()), "déclarations hors des attributs");
    }
}
