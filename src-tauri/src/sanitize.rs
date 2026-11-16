//! Nettoyage du HTML des messages (lot 4, relecture S2, S3, S4, S6, F14).
//!
//! Seul point de nettoyage du HTML venant d'un mail : le corps « simplifié »
//! sert à la citation (réponse, transfert) et à la reprise des brouillons,
//! c'est-à-dire partout où le HTML entre dans une fenêtre de l'application.
//! Liste blanche (crate `ammonia`) : balises de mise en forme, liens `http`,
//! `https` et `mailto`, images `data:` et `cid:` seulement, styles en ligne
//! réduits à des propriétés inoffensives. Le résultat est inerte : aucun
//! script, aucune requête vers Internet, aucun recouvrement de la fenêtre.

use std::borrow::Cow;
use std::sync::OnceLock;

use ammonia::{Builder, UrlRelative};

/// Propriétés CSS gardées dans les attributs `style` (mise en forme du texte,
/// des tableaux et des blocs). Ni `position`, ni `background` (qui peut porter
/// une image distante), ni rien qui contienne une adresse.
const CSS_PROPERTIES: &[&str] = &[
    "color",
    "background-color",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-variant",
    "font-weight",
    "text-align",
    "text-decoration",
    "text-indent",
    "text-transform",
    "line-height",
    "letter-spacing",
    "word-spacing",
    "white-space",
    "vertical-align",
    "direction",
    "margin",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "border",
    "border-top",
    "border-right",
    "border-bottom",
    "border-left",
    "border-color",
    "border-width",
    "border-style",
    "border-collapse",
    "border-spacing",
    "border-radius",
    "width",
    "height",
    "min-width",
    "max-width",
    "min-height",
    "max-height",
    "list-style-type",
    "float",
    "clear",
    "table-layout",
    "caption-side",
    "empty-cells",
];

/// Fragments interdits dans une valeur CSS, quelle que soit la propriété.
/// `image-set(…)`, `image(…)`, `cross-fade(…)` et `element(…)` désignent une
/// image sans passer par `url(` (relecture du 10 novembre 2026).
const CSS_FORBIDDEN: &[&str] = &[
    "url(",
    "image-set",
    "image(",
    "cross-fade",
    "element(",
    "expression",
    "javascript:",
    "@import",
    "behavior",
    "-moz-binding",
    "\\",
    "/*",
];

/// `style` réduit aux propriétés de la liste blanche ; `None` s'il ne reste rien.
fn filter_style(style: &str) -> Option<String> {
    let kept: Vec<String> = style
        .split(';')
        .filter_map(|declaration| {
            let (property, value) = declaration.split_once(':')?;
            let property = property.trim().to_ascii_lowercase();
            let value = value.trim();
            let lower = value.to_ascii_lowercase();
            let allowed =
                CSS_PROPERTIES.contains(&property.as_str()) && !value.is_empty() && !CSS_FORBIDDEN.iter().any(|f| lower.contains(f));
            allowed.then(|| format!("{property}: {value}"))
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join("; "))
}

fn builder() -> &'static Builder<'static> {
    static BUILDER: OnceLock<Builder<'static>> = OnceLock::new();
    BUILDER.get_or_init(|| {
        let mut b = Builder::default();
        b.add_tags(["font", "center"])
            .add_tag_attributes("font", ["color", "face", "size"])
            .add_tag_attributes("table", ["width", "border", "cellpadding", "cellspacing", "bgcolor"])
            .add_tag_attributes("td", ["width", "height", "valign", "bgcolor", "nowrap"])
            .add_tag_attributes("th", ["width", "height", "valign", "bgcolor", "nowrap"])
            .add_tag_attributes("tr", ["valign", "bgcolor"])
            .add_generic_attributes(["style", "dir", "align"])
            // Contenu jeté avec la balise (et pas seulement la balise).
            .add_clean_content_tags(["noscript", "title", "template", "xmp", "noembed", "noframes"])
            .url_schemes(["http", "https", "mailto", "data", "cid"].into_iter().collect())
            .url_relative(UrlRelative::Deny)
            .link_rel(Some("noopener noreferrer"))
            .attribute_filter(|element, attribute, value| match (element, attribute) {
                ("a", "href") => {
                    let lower = value.trim_start().to_ascii_lowercase();
                    let ok = ["http://", "https://", "mailto:"].iter().any(|s| lower.starts_with(s));
                    ok.then_some(Cow::Borrowed(value))
                }
                // Images intégrées seulement : une image distante dans une
                // citation trahirait la lecture (traceur, relecture S2).
                ("img", "src") => {
                    let lower = value.trim_start().to_ascii_lowercase();
                    (lower.starts_with("data:image/") || lower.starts_with("cid:")).then_some(Cow::Borrowed(value))
                }
                (_, "style") => filter_style(value).map(Cow::Owned),
                // Adresses annexes (citation d'origine…) : inutiles, retirées.
                (_, "cite" | "longdesc" | "usemap") => None,
                _ => Some(Cow::Borrowed(value)),
            });
        b
    })
}

/// Corps nettoyé, à insérer dans une fenêtre de l'application.
#[must_use]
pub fn simplify(html: &str) -> String {
    builder().clean(html).to_string()
}

/// Balises retirées du HTML d'origine (mode d'affichage « Original »), comme
/// le fait Thunderbird : `<meta>` (rafraîchissement qui ferait naviguer le
/// message sans clic, relecture S6), `<link>` (pré-résolution DNS, préchargement :
/// le mail se signalerait ouvert) et `<base>` (adresse de base des liens).
const ORIGINAL_STRIPPED: [&str; 3] = ["meta", "link", "base"];

/// HTML d'origine sans les balises de [`ORIGINAL_STRIPPED`]. La fin d'une
/// balise est cherchée hors guillemets (`content="0;url=a>b"`), et toute balise
/// de ces noms part, quels que soient ses attributs ou leur codage
/// (`http-equiv="&#114;efresh"`).
#[must_use]
pub fn strip_refresh(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if opens_tag(tail.as_bytes(), &ORIGINAL_STRIPPED) {
            rest = &tail[tag_end(tail.as_bytes())..];
        } else {
            out.push('<');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// `tag` commence-t-il par `<nom` suivi d'un blanc, de `/` ou de `>` ?
fn opens_tag(tag: &[u8], names: &[&str]) -> bool {
    names.iter().any(|name| {
        let end = 1 + name.len();
        tag.get(1..end).is_some_and(|n| n.eq_ignore_ascii_case(name.as_bytes()))
            && tag.get(end).is_none_or(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c | b'/' | b'>'))
    })
}

/// Position juste après le `>` qui ferme la balise (hors guillemets), ou la fin.
fn tag_end(tag: &[u8]) -> usize {
    let mut quote = 0u8;
    for (i, &b) in tag.iter().enumerate().skip(1) {
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else if b == b'"' || b == b'\'' {
            quote = b;
        } else if b == b'>' {
            return i + 1;
        }
    }
    tag.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_et_evenements_supprimes() {
        let clean = simplify(r#"<p onclick="x()">Bonjour</p><script>alert(1)</script><img src=x onerror="y()">"#);
        assert!(!clean.contains("onclick") && !clean.contains("onerror") && !clean.contains("<script"));
        assert!(clean.contains("Bonjour"));
    }

    #[test]
    fn ressources_distantes_supprimees() {
        let t = "https://tracker.example.invalid/u";
        for html in [
            format!(r#"<img src="data:," srcset="{t} 1x">"#),
            format!(r#"<img src="{t}">"#),
            format!(r#"<svg><image href="{t}"/></svg>"#),
            format!(r#"<div style="background:url({t})">x</div>"#),
            format!(r#"<table><tr><td background="{t}">x</td></tr></table>"#),
            format!(r#"<video poster="{t}"></video>"#),
            format!(r#"<input type="image" src="{t}">"#),
            format!(r#"<picture><source srcset="{t}"></picture>"#),
        ] {
            let clean = simplify(&html);
            assert!(!clean.contains("tracker.example.invalid"), "{html} → {clean}");
        }
    }

    #[test]
    fn styles_filtres() {
        let clean = simplify(r#"<div style="position:fixed;inset:0;z-index:99999;color:red;font-weight:bold">x</div>"#);
        assert!(!clean.contains("position") && !clean.contains("z-index") && !clean.contains("inset"));
        assert!(clean.contains("color: red") && clean.contains("font-weight: bold"));
        assert_eq!(filter_style("color: expression(alert(1))"), None);
        assert_eq!(filter_style("color: re\\64"), None);
    }

    #[test]
    fn liens_et_images_integrees() {
        let clean = simplify(
            r#"<a href="https://example.fr/">ok</a><a href="javascript:alert(1)">ko</a><a href="data:text/html,x">ko</a><img src="data:image/png;base64,AA">"#,
        );
        assert!(clean.contains(r#"href="https://example.fr/""#));
        assert!(!clean.contains("javascript:") && !clean.contains("data:text/html"));
        assert!(clean.contains("data:image/png;base64,AA"));
        assert!(clean.contains("noopener"));
    }

    #[test]
    fn mxss_noscript_neutralise() {
        let clean = simplify(r#"<noscript><p title="</noscript><img src=x onerror=alert(1)>"></p></noscript>"#);
        // Selon l'analyseur, le <img> peut ressortir comme vraie balise : il
        // doit alors avoir perdu son gestionnaire et son adresse non intégrée.
        assert!(!clean.contains("onerror") && !clean.contains("src="), "{clean}");
        assert_eq!(simplify(&clean), clean, "second nettoyage différent : risque de mXSS");
    }

    #[test]
    fn rafraichissement_retire() {
        let html =
            r#"<html><head><META HTTP-EQUIV="Refresh" content="0;url=https://t/u"><meta charset="utf-8"></head><body>x</body></html>"#;
        let out = strip_refresh(html);
        assert!(!out.to_ascii_lowercase().contains("<meta"));
        assert!(out.contains("<head></head><body>x</body>"), "{out}");
        // Variantes de la relecture du 10 novembre 2026 : `>` entre guillemets,
        // nom codé ; et les balises qui font sortir une requête.
        for hostile in [
            r#"<meta content="0;url=https://t.invalid/a>b" http-equiv="refresh">"#,
            r#"<meta http-equiv="&#114;efresh" content="0;url=https://t.invalid/">"#,
            r#"<link rel="dns-prefetch" href="//id123.traceur.invalid">"#,
            "<LINK REL=preconnect HREF=https://t.invalid/>",
            r#"<base href="https://t.invalid/">"#,
        ] {
            let out = strip_refresh(&format!("<p>avant</p>{hostile}<p>après</p>"));
            assert_eq!(out, "<p>avant</p><p>après</p>", "{hostile}");
        }
        // Ce qui ressemble seulement à ces balises reste.
        assert_eq!(strip_refresh("<metadata>a</metadata><linked>"), "<metadata>a</metadata><linked>");
    }

    #[test]
    fn images_par_image_set_refusees() {
        assert_eq!(filter_style(r#"list-style: image-set("https://t.invalid/x" 1x)"#), None);
        assert_eq!(
            filter_style(r#"color: red; background-color: -webkit-image-set("https://t.invalid/x" 1x)"#).as_deref(),
            Some("color: red")
        );
        assert_eq!(filter_style("list-style-type: disc").as_deref(), Some("list-style-type: disc"));
    }
}
