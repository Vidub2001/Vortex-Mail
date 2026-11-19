//! Client `CardDAV` (contacts) et `CalDAV` (agendas) : découverte des carnets et
//! agendas d'un compte, lecture de leur contenu, écriture et suppression.
//!
//! Découverte (RFC 6764) : `/.well-known/carddav` ou `/.well-known/caldav`,
//! puis `current-user-principal` → `addressbook-home-set` / `calendar-home-set`
//! → collections de ce dossier.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::Mutex;

use crate::http;
use crate::xml::{self, Element};

const MAX_RESPONSE: usize = 32 * 1024 * 1024;
/// Éléments demandés par requête `multiget`.
const MULTIGET_BATCH: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Contacts,
    Calendar,
}

impl Kind {
    fn well_known(self) -> &'static str {
        match self {
            Kind::Contacts => "/.well-known/carddav",
            Kind::Calendar => "/.well-known/caldav",
        }
    }
    fn home_set(self) -> &'static str {
        match self {
            Kind::Contacts => "addressbook-home-set",
            Kind::Calendar => "calendar-home-set",
        }
    }
    fn namespace(self) -> &'static str {
        match self {
            Kind::Contacts => "urn:ietf:params:xml:ns:carddav",
            Kind::Calendar => "urn:ietf:params:xml:ns:caldav",
        }
    }
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Kind::Contacts => "contacts",
            Kind::Calendar => "calendar",
        }
    }
}

/// Où le mot de passe ou le jeton d'un compte peut être envoyé : les sites
/// du compte (domaine de l'adresse, serveurs saisis dans les réglages), en
/// HTTPS ; en clair, seulement l'hôte d'une adresse `http://` saisie. Une
/// adresse renvoyée par le serveur (`href`, redirection) hors de ces sites ne
/// reçoit rien (relecture du 10 novembre 2026).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Scope {
    sites: Vec<String>,
    plain_hosts: Vec<String>,
}

impl Scope {
    /// `trusted` : adresses e-mail, domaines ou adresses de serveurs.
    #[must_use]
    pub(crate) fn new(trusted: &[&str]) -> Self {
        let mut scope = Scope::default();
        for value in trusted.iter().map(|v| v.trim()).filter(|v| !v.is_empty()) {
            if value.contains("://") {
                if let Ok((https, host, _, _)) = http::split_url(value) {
                    if !https {
                        scope.plain_hosts.push(host.to_ascii_lowercase());
                    }
                    scope.sites.push(site(&host));
                }
            } else {
                scope.sites.push(site(value.rsplit('@').next().unwrap_or(value)));
            }
        }
        scope.sites.sort();
        scope.sites.dedup();
        scope
    }

    pub(crate) fn allows(&self, url: &str) -> bool {
        let Ok((https, host, _, _)) = http::split_url(url) else { return false };
        let host = host.to_ascii_lowercase();
        if https {
            self.sites.contains(&site(&host))
        } else {
            self.plain_hosts.contains(&host)
        }
    }
}

/// Domaines de deuxième niveau courants sous un code pays (`exemple.co.uk`).
const SECOND_LEVEL: [&str; 8] = ["ac", "co", "com", "edu", "gouv", "gov", "net", "org"];

/// Site d'un hôte : son domaine principal (`dav.exemple.fr` → `exemple.fr`,
/// `mail.exemple.co.uk` → `exemple.co.uk`) ; une adresse IP reste entière.
fn site(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.starts_with('[') || host.parse::<std::net::IpAddr>().is_ok() {
        return host;
    }
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    let keep = if n >= 3 && labels[n - 1].len() == 2 && SECOND_LEVEL.contains(&labels[n - 2]) { 3 } else { 2 };
    labels[n.saturating_sub(keep)..].join(".")
}

/// Identification auprès du serveur.
#[derive(Clone)]
pub(crate) enum Auth {
    Basic { user: String, password: String, scope: Scope },
    Bearer(String, Scope),
}

impl Auth {
    fn scope(&self) -> &Scope {
        match self {
            Auth::Basic { scope, .. } | Auth::Bearer(_, scope) => scope,
        }
    }

    /// Réponse 401 : un jeton d'accès Google refusé est oublié et renouvelé
    /// (« Reconnecter » proposé si l'autorisation a été retirée).
    fn refused(&self, status: u16) {
        if let (401, Auth::Bearer(token, _)) = (status, self) {
            crate::google::token_refused(token);
        }
    }

    fn header(&self) -> (&'static str, String) {
        match self {
            Auth::Basic { user, password, .. } => {
                ("Authorization", format!("Basic {}", imap_lite::base64_encode(format!("{user}:{password}").as_bytes())))
            }
            Auth::Bearer(token, _) => ("Authorization", format!("Bearer {token}")),
        }
    }
}

/// Carnet d'adresses ou agenda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Collection {
    pub url: String,
    pub name: String,
    pub color: String,
    pub kind: Kind,
}

/// Contact ou rendez-vous tel que stocké sur le serveur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub url: String,
    pub etag: String,
    pub data: String,
}

fn status_error(status: u16, what: &str) -> String {
    match status {
        401 => tr!("{what} : identifiants refusés par le serveur de contacts et d'agenda", what = what),
        403 => tr!("{what} : accès refusé par le serveur de contacts et d'agenda", what = what),
        404 => tr!("{what} : introuvable sur le serveur", what = what),
        412 => tr!("{what} : modifié ailleurs entre-temps, resynchronisez puis recommencez", what = what),
        _ => tr!("{what} : réponse HTTP {status}", what = what, status = status),
    }
}

/// Raison donnée par le serveur dans le corps d'une réponse d'erreur
/// (JSON de Google `"message": "…"`, ou texte / XML), courte et sur une ligne.
/// Octets d'une réponse d'erreur lus pour en tirer la raison.
const MAX_REASON_SOURCE: usize = 64 * 1024;

pub(crate) fn server_reason(body: &[u8]) -> String {
    // Le début suffit pour la raison ; une page d'erreur de plusieurs Mo
    // rendait le retrait des blocs `<style>` quadratique (audit S7).
    let body = &body[..body.len().min(MAX_REASON_SOURCE)];
    let text = String::from_utf8_lossy(body);
    let reason = if let Some(i) = text.find("\"message\"") {
        let rest = &text[i + 9..];
        let start = rest.find('"').map_or(0, |q| q + 1);
        let rest = &rest[start..];
        let mut out = String::new();
        let mut escaped = false;
        for c in rest.chars() {
            match (escaped, c) {
                (true, _) => {
                    out.push(c);
                    escaped = false;
                }
                (false, '\\') => escaped = true,
                (false, '"') => break,
                _ => out.push(c),
            }
        }
        out
    } else {
        // Page d'erreur HTML : sans le contenu des blocs `<style>` et
        // `<script>` (la page 502 de Google affichait toute sa feuille de style).
        let text = without_blocks(&text);
        let mut out = String::new();
        let mut in_tag = false;
        for c in text.chars() {
            match c {
                '<' => in_tag = true,
                '>' => {
                    in_tag = false;
                    out.push(' ');
                }
                _ if !in_tag => out.push(c),
                _ => {}
            }
        }
        out
    };
    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut short: String = reason.chars().take(300).collect();
    if short.len() < reason.len() {
        short.push('…');
    }
    short
}

/// Texte sans les blocs `<style>` et `<script>`, en un seul passage : retirer
/// les blocs un par un en relisant tout le texte à chaque fois était encore
/// quadratique sur les 64 Kio gardés (test S7 à 2,5 s sur une machine chargée).
fn without_blocks(text: &str) -> String {
    // Minuscules ASCII : mêmes positions d'octets que le texte d'origine.
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(found) = lower[i..].find('<') {
        let start = i + found;
        if let Some(block) = ["style", "script"].into_iter().find(|block| lower[start + 1..].starts_with(*block)) {
            out.push_str(&text[i..start]);
            out.push(' ');
            let close = format!("</{block}>");
            // Bloc sans fin : retiré jusqu'au bout.
            i = lower[start..].find(&close).map_or(text.len(), |end| start + end + close.len());
        } else {
            out.push_str(&text[i..=start]);
            i = start + 1;
        }
    }
    out.push_str(&text[i..]);
    out
}

fn http_error(response: &http::Response, what: &str) -> String {
    let base = status_error(response.status, what);
    let reason = server_reason(&response.body);
    if reason.is_empty() {
        base
    } else {
        tr!("{base} (HTTP {status} : {reason})", base = base, status = response.status, reason = reason)
    }
}

/// Identifiants envoyés seulement vers les sites du compte : lectures
/// (`dav`) comme écritures (`put_response`, `delete` ; relecture complète
/// n° 3 : l'adresse d'un élément vient du serveur).
fn check_scope(url: &str, auth: &Auth) -> Result<(), String> {
    if auth.scope().allows(url) {
        return Ok(());
    }
    Err(tr!(
        "Le serveur d'agenda ou de contacts renvoie vers {url}, hors du serveur du compte : identifiants non envoyés. \
         S'il est légitime, indiquez son adresse dans les réglages du compte.",
        url = url
    ))
}

/// Sites (schéma, hôte, port) qui ont demandé le mot de passe par une réponse
/// `401` avec `WWW-Authenticate: Basic`, pendant cette session.
static CHALLENGED: Mutex<BTreeSet<(bool, String, u16)>> = Mutex::new(BTreeSet::new());

fn challenged(url: &str) -> bool {
    let Ok(site) = http::origin(url) else { return false };
    // Verrou repris après une panique : un ensemble qu'on ne fait qu'agrandir.
    CHALLENGED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&site)
}

fn note_challenge(url: &str) {
    if let Ok(site) = http::origin(url) {
        CHALLENGED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(site);
    }
}

/// Requête `WebDAV` authentifiée. Le mot de passe (`Basic`) ne part vers un
/// site qu'après que celui-ci l'a demandé (`401` et `WWW-Authenticate: Basic`),
/// comme le fait un navigateur : un site web du domaine qui n'est pas un
/// serveur d'agenda ne le reçoit plus (relecture complète n° 4). Un jeton
/// (`Bearer`) ne va que vers les API connues : il part d'emblée.
fn send_authorized(
    method: &str,
    url: &str,
    auth: &Auth,
    headers: &[(&'static str, String)],
    body: Option<&[u8]>,
    max_bytes: usize,
) -> Result<(http::Response, String), String> {
    check_scope(url, auth)?;
    let with_auth = |extra: &[(&'static str, String)]| {
        let mut all = vec![auth.header()];
        all.extend_from_slice(extra);
        all
    };
    if matches!(auth, Auth::Bearer(..)) || challenged(url) {
        let result = http::send(method, url, &with_auth(headers), body, max_bytes)?;
        auth.refused(result.0.status);
        return Ok(result);
    }
    let (response, final_url) = http::send(method, url, headers, body, max_bytes)?;
    let asks_basic =
        response.status == 401 && response.header("WWW-Authenticate").is_some_and(|v| v.to_ascii_lowercase().contains("basic"));
    if !asks_basic {
        return Ok((response, final_url));
    }
    // Demandé par le site où la requête a abouti (après d'éventuelles redirections).
    check_scope(&final_url, auth)?;
    note_challenge(&final_url);
    let result = http::send(method, &final_url, &with_auth(headers), body, max_bytes)?;
    auth.refused(result.0.status);
    Ok(result)
}

fn dav(method: &str, url: &str, auth: &Auth, depth: Option<&str>, body: Option<&str>) -> Result<(http::Response, String), String> {
    let mut headers = Vec::new();
    if let Some(d) = depth {
        headers.push(("Depth", d.to_string()));
    }
    if body.is_some() {
        headers.push(("Content-Type", "application/xml; charset=utf-8".to_string()));
    }
    send_authorized(method, url, auth, &headers, body.map(str::as_bytes), MAX_RESPONSE)
}

/// `PROPFIND` ou `REPORT` : réponse multistatus analysée, et adresse finale (après redirections).
fn multistatus(method: &str, url: &str, auth: &Auth, depth: &str, body: &str) -> Result<(Element, String), String> {
    let (response, final_url) = dav(method, url, auth, Some(depth), Some(body))?;
    if response.status != 207 {
        return Err(http_error(&response, url));
    }
    Ok((xml::parse(&String::from_utf8_lossy(&response.body)), final_url))
}

fn href_of(base: &str, element: &Element) -> Option<String> {
    let href = element.dav_child("href").or_else(|| element.dav_descendants("href").into_iter().next())?.text_content();
    (!href.is_empty()).then(|| http::resolve(base, &href))
}

/// Partie `prop` réussie (statut 200) d'une réponse.
fn ok_props(response: &Element) -> Vec<&Element> {
    response
        .dav_descendants("propstat")
        .into_iter()
        .filter(|p| p.dav_child("status").is_none_or(|s| s.text_content().contains(" 200")))
        .filter_map(|p| p.dav_child("prop"))
        .collect()
}

fn prop<'a>(response: &'a Element, name: &str) -> Option<&'a Element> {
    ok_props(response).into_iter().find_map(|p| p.child(name))
}

/// Adresse de base à partir d'une adresse saisie ou d'un domaine.
fn start_urls(hint: &str, kind: Kind) -> Vec<String> {
    let hint = hint.trim().trim_end_matches('/');
    if hint.starts_with("http://") || hint.starts_with("https://") {
        // `.well-known` redirige : pas de barre finale.
        return vec![if hint.contains("/.well-known/") { hint.to_string() } else { format!("{hint}/") }];
    }
    let domain = hint.rsplit('@').next().unwrap_or(hint);
    vec![format!("https://{domain}{}", kind.well_known()), format!("https://dav.{domain}{}", kind.well_known())]
}

/// Adresses de départ annoncées par le DNS (lot G3, RFC 6764 §3) pour une adresse ou un
/// domaine `hint` : serveurs `SRV` en HTTPS du même site que le domaine, chemin du `TXT`
/// (sinon `.well-known`).
fn srv_urls(domain: &str, kind: Kind, servers: &[(crate::dns::Srv, Option<String>)]) -> Vec<String> {
    let domain_site = site(domain);
    servers
        .iter()
        .filter(|(s, _)| {
            let target = s.target.trim_end_matches('.').to_ascii_lowercase();
            !target.is_empty()
                && target.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
                && site(&target) == domain_site
        })
        .map(|(s, path)| {
            let target = s.target.trim_end_matches('.').to_ascii_lowercase();
            let port = if s.port == 443 { String::new() } else { format!(":{}", s.port) };
            format!("https://{target}{port}{}", path.as_deref().unwrap_or(kind.well_known()))
        })
        .take(4)
        .collect()
}

/// Carnets d'adresses ou agendas du compte.
pub(crate) fn discover(hint: &str, auth: &Auth, kind: Kind) -> Result<Vec<Collection>, String> {
    let mut last_error = String::from(tr!("aucun serveur trouvé"));
    let trimmed = hint.trim();
    let mut starts = Vec::new();
    // Adresse ou domaine (pas une adresse web donnée) : le DNS d'abord (lot G3).
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        let domain = trimmed.rsplit('@').next().unwrap_or(trimmed);
        starts = srv_urls(domain, kind, &crate::dns::dav_servers(domain, kind == Kind::Calendar));
    }
    // Sans doublon : une adresse annoncée peut être aussi une adresse devinée.
    for url in start_urls(hint, kind) {
        if !starts.contains(&url) {
            starts.push(url);
        }
    }
    for start in starts {
        match discover_from(&start, auth, kind) {
            Ok(list) => return Ok(list),
            Err(e) => last_error = e,
        }
    }
    Err(last_error)
}

fn discover_from(start: &str, auth: &Auth, kind: Kind) -> Result<Vec<Collection>, String> {
    match discover_home(start, auth, kind) {
        Ok(home) => match list_collections(&home, auth, kind) {
            Ok(list) => Ok(list),
            // Adresse donnée = un agenda précis (Mailo…) : on le prend tel quel.
            Err(e) if home.trim_end_matches('/') != start.trim_end_matches('/') => list_collections(start, auth, kind).map_err(|_| e),
            Err(e) => Err(e),
        },
        Err(e) => list_collections(start, auth, kind).map_err(|_| e),
    }
}

/// Dossier des carnets ou agendas, par le principal de l'utilisateur.
fn discover_home(start: &str, auth: &Auth, kind: Kind) -> Result<String, String> {
    // 1. Principal de l'utilisateur.
    let body =
        r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#;
    let (root, base) = multistatus("PROPFIND", start, auth, "0", body)?;
    let principal = root.find("current-user-principal").and_then(|p| href_of(&base, p)).unwrap_or_else(|| base.clone());

    // 2. Dossier des carnets ou agendas.
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="{}"><d:prop><c:{}/></d:prop></d:propfind>"#,
        kind.namespace(),
        kind.home_set()
    );
    let (root, base) = multistatus("PROPFIND", &principal, auth, "0", &body)?;
    Ok(root.find(kind.home_set()).and_then(|h| href_of(&base, h)).unwrap_or(base))
}

/// 3. Carnets ou agendas trouvés dans un dossier (ou l'adresse elle-même).
fn list_collections(home: &str, auth: &Auth, kind: Kind) -> Result<Vec<Collection>, String> {
    let body = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/"><d:prop><d:resourcetype/><d:displayname/><a:calendar-color/><c:supported-calendar-component-set/></d:prop></d:propfind>"#;
    let (root, base) = multistatus("PROPFIND", home, auth, "1", body)?;
    let wanted = match kind {
        Kind::Contacts => "addressbook",
        Kind::Calendar => "calendar",
    };
    let mut out = Vec::new();
    for response in root.dav_descendants("response") {
        let Some(url) = href_of(&base, response) else { continue };
        let Some(types) = prop(response, "resourcetype") else { continue };
        if types.child(wanted).is_none() || (kind == Kind::Calendar && !holds_events(response)) {
            continue;
        }
        let name = prop(response, "displayname").map_or_default(Element::text_content);
        let color = prop(response, "calendar-color").map_or_default(Element::text_content);
        let fallback = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
        out.push(Collection { name: if name.is_empty() { fallback } else { name }, color, url, kind });
    }
    if out.is_empty() {
        return Err(if kind == Kind::Contacts {
            tr!("aucun carnet d'adresses trouvé").to_string()
        } else {
            tr!("aucun agenda trouvé").to_string()
        });
    }
    Ok(out)
}

/// Agenda qui accepte des rendez-vous : une liste de tâches (`VTODO`
/// seulement) n'est pas proposée comme agenda (RFC 4791 §5.2.3 ; relecture
/// du 12 novembre, n° 30). Sans la propriété, tous les types sont acceptés.
fn holds_events(response: &Element) -> bool {
    prop(response, "supported-calendar-component-set").is_none_or(|set| {
        let comps = set.descendants("comp");
        comps.is_empty() || comps.iter().any(|c| c.attr("name").is_some_and(|n| n.eq_ignore_ascii_case("VEVENT")))
    })
}

fn etag_of(response: &Element) -> String {
    prop(response, "getetag").map_or_default(Element::text_content)
}

/// Résultat d'une relecture : éléments nouveaux ou changés, avec leur
/// contenu, et adresses des éléments inchangés, à garder tels quels (audit P6).
#[derive(Debug, Default)]
pub(crate) struct Fetched {
    pub changed: Vec<Item>,
    pub unchanged: Vec<String>,
}

/// Adresse et version (`ETag`) de chaque élément d'une réponse, sans la
/// collection elle-même ni ses sous-dossiers.
fn listing(root: &Element, base: &str) -> Vec<(String, String)> {
    root.dav_descendants("response")
        .into_iter()
        .filter_map(|r| {
            let url = href_of(base, r)?;
            (url.trim_end_matches('/') != base.trim_end_matches('/') && !url.ends_with('/')).then(|| (url, etag_of(r)))
        })
        .collect()
}

/// Sépare les éléments inchangés (même `ETag` que celui du cache) de ceux à
/// relire : nouveaux, changés, ou sans `ETag` (rien ne prouve qu'ils n'ont pas
/// changé). `known` : adresse → `ETag` des éléments en cache.
fn split_by_etag(listing: Vec<(String, String)>, known: &HashMap<String, String>) -> (Vec<String>, Vec<String>) {
    let mut wanted = Vec::new();
    let mut unchanged = Vec::new();
    for (url, etag) in listing {
        if !etag.is_empty() && known.get(&url) == Some(&etag) {
            unchanged.push(url);
        } else {
            wanted.push(url);
        }
    }
    (wanted, unchanged)
}

/// Contenu des éléments `urls`, par lots (`addressbook-multiget` ou
/// `calendar-multiget`).
fn multiget(collection: &str, auth: &Auth, kind: Kind, urls: &[String]) -> Result<Vec<Item>, String> {
    let (report, data) = match kind {
        Kind::Contacts => ("addressbook-multiget", "address-data"),
        Kind::Calendar => ("calendar-multiget", "calendar-data"),
    };
    let mut items = Vec::new();
    for batch in urls.chunks(MULTIGET_BATCH) {
        let hrefs = batch.iter().fold(String::new(), |mut s, u| {
            let _ = write!(s, "<d:href>{}</d:href>", xml::escape(&path_of(u)));
            s
        });
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><c:{report} xmlns:d="DAV:" xmlns:c="{ns}"><d:prop><d:getetag/><c:{data}/></d:prop>{hrefs}</c:{report}>"#,
            ns = kind.namespace()
        );
        let (root, base) = multistatus("REPORT", collection, auth, "1", &body)?;
        items.extend(items_from(&root, &base, data));
    }
    Ok(items)
}

/// Fiches d'un carnet : liste des versions, puis contenu des seules fiches
/// nouvelles ou changées depuis `known` (adresse → `ETag`, audit P6).
pub(crate) fn fetch_contacts(collection: &str, auth: &Auth, known: &HashMap<String, String>) -> Result<Fetched, String> {
    let body =
        r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:getetag/><d:getcontenttype/></d:prop></d:propfind>"#;
    let (root, base) = multistatus("PROPFIND", collection, auth, "1", body)?;
    let (wanted, unchanged) = split_by_etag(listing(&root, &base), known);
    Ok(Fetched { changed: multiget(collection, auth, Kind::Contacts, &wanted)?, unchanged })
}

/// Rendez-vous d'un agenda entre deux instants (`AAAAMMJJTHHMMSSZ`),
/// répétitions comprises : versions seules d'abord (`calendar-query`), puis
/// contenu des seuls rendez-vous nouveaux ou changés (audit P6).
pub(crate) fn fetch_events(
    collection: &str,
    auth: &Auth,
    from: &str,
    to: &str,
    known: &HashMap<String, String>,
) -> Result<Fetched, String> {
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="{from}" end="{to}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#
    );
    let (root, base) = multistatus("REPORT", collection, auth, "1", &body)?;
    let (wanted, unchanged) = split_by_etag(listing(&root, &base), known);
    Ok(Fetched { changed: multiget(collection, auth, Kind::Calendar, &wanted)?, unchanged })
}

pub(crate) fn items_from(root: &Element, base: &str, data_name: &str) -> Vec<Item> {
    root.dav_descendants("response")
        .into_iter()
        .filter_map(|r| {
            let url = href_of(base, r)?;
            let data = prop(r, data_name)?.text_content();
            (!data.is_empty()).then(|| Item { url, etag: etag_of(r), data })
        })
        .collect()
}

fn path_of(url: &str) -> String {
    http::split_url(url).map_or_else(|_| url.to_string(), |(_, _, _, path)| path)
}

/// Crée (`create`) ou remplace un élément ; renvoie sa nouvelle version (peut être vide).
/// La version connue (`etag`) protège d'un écrasement d'une modification faite ailleurs.
pub(crate) fn put(url: &str, auth: &Auth, content_type: &str, data: &str, etag: &str, create: bool) -> Result<String, String> {
    let response = put_response(url, auth, content_type, data, etag, create)?;
    if !(200..300).contains(&response.status) {
        return Err(http_error(&response, tr!("enregistrement")));
    }
    Ok(response.header("etag").unwrap_or("").to_string())
}

/// Création d'une ressource dont Vortex Mail a choisi le nom (UID qu'il a
/// généré). Si elle existe déjà, c'est la sienne : un essai précédent dont la
/// réponse s'est perdue (délai dépassé). Elle est alors remplacée, au lieu
/// d'un refus puis d'un doublon au nouvel essai.
pub(crate) fn put_own(url: &str, auth: &Auth, content_type: &str, data: &str) -> Result<String, String> {
    let response = put_response(url, auth, content_type, data, "", true)?;
    if response.status == 412 {
        return put(url, auth, content_type, data, "", false);
    }
    if !(200..300).contains(&response.status) {
        return Err(http_error(&response, tr!("enregistrement")));
    }
    Ok(response.header("etag").unwrap_or("").to_string())
}

/// Relit un élément après un enregistrement sans version en réponse :
/// (version, contenu tel que le serveur l'a gardé).
pub(crate) fn reread(url: &str, auth: &Auth) -> Result<(String, String), String> {
    let (response, _) = send_authorized("GET", url, auth, &[], None, 1024 * 1024)?;
    if !(200..300).contains(&response.status) {
        return Err(http_error(&response, url));
    }
    let etag = response.header("etag").unwrap_or("").to_string();
    Ok((etag, String::from_utf8_lossy(&response.body).into_owned()))
}

/// Le serveur fait-il la planification lui-même (RFC 6638, `calendar-auto-schedule`
/// dans l'en-tête `DAV` de `OPTIONS`) ? Il dépose alors les invitations chez
/// les invités et transmet leurs réponses à l'organisateur (lot H1b).
pub(crate) fn auto_schedule(url: &str, auth: &Auth) -> bool {
    send_authorized("OPTIONS", url, auth, &[], None, 64 * 1024)
        .is_ok_and(|(r, _)| r.header("dav").is_some_and(|dav| dav.to_ascii_lowercase().contains("calendar-auto-schedule")))
}

/// Remplace la copie d'une invitation où la ligne du compte porte la réponse
/// (lot H1b). Sur un serveur qui planifie, `notify` faux ajoute
/// `Schedule-Reply: F` (RFC 6638 §8.1) : l'organisateur n'est pas prévenu.
pub(crate) fn put_answer(url: &str, auth: &Auth, data: &str, etag: &str, notify: bool) -> Result<String, String> {
    let mut headers = vec![("Content-Type", "text/calendar; charset=utf-8".to_string())];
    if !etag.is_empty() {
        headers.push(("If-Match", etag.to_string()));
    }
    if !notify {
        headers.push(("Schedule-Reply", "F".to_string()));
    }
    let (response, _) = send_authorized("PUT", url, auth, &headers, Some(data.as_bytes()), 1024 * 1024)?;
    if !(200..300).contains(&response.status) {
        return Err(http_error(&response, tr!("enregistrement")));
    }
    Ok(response.header("etag").unwrap_or("").to_string())
}

fn put_response(url: &str, auth: &Auth, content_type: &str, data: &str, etag: &str, create: bool) -> Result<http::Response, String> {
    let mut headers = vec![("Content-Type", format!("{content_type}; charset=utf-8"))];
    if create {
        headers.push(("If-None-Match", "*".to_string()));
    } else if !etag.is_empty() {
        headers.push(("If-Match", etag.to_string()));
    }
    let (response, _) = send_authorized("PUT", url, auth, &headers, Some(data.as_bytes()), 1024 * 1024)?;
    Ok(response)
}

pub(crate) fn delete(url: &str, auth: &Auth, etag: &str) -> Result<(), String> {
    let mut headers = Vec::new();
    if !etag.is_empty() {
        headers.push(("If-Match", etag.to_string()));
    }
    let (response, _) = send_authorized("DELETE", url, auth, &headers, None, 1024 * 1024)?;
    match response.status {
        200..=299 | 404 => Ok(()),
        s => Err(status_error(s, tr!("suppression"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_stay_on_the_account_sites() {
        let scope = Scope::new(&["moi@exemple.fr", "https://dav.fournisseur.net/dav/", "http://192.168.1.10:8080/dav/"]);
        assert!(scope.allows("https://exemple.fr/.well-known/caldav"));
        assert!(scope.allows("https://dav.exemple.fr/cal/"));
        assert!(scope.allows("https://p42.fournisseur.net/x"));
        assert!(scope.allows("http://192.168.1.10:8080/dav/cal/"));
        // Ailleurs, ou en clair vers un hôte qui n'a pas été saisi : rien.
        assert!(!scope.allows("https://autre.example/p/"));
        assert!(!scope.allows("http://dav.exemple.fr/cal/"));
        assert!(!scope.allows("https://exemple.fr.autre.example/"));
        assert!(!scope.allows("http://192.168.1.11/"));
        let uk = Scope::new(&["a@mail.exemple.co.uk"]);
        assert!(uk.allows("https://dav.exemple.co.uk/"));
        assert!(!uk.allows("https://autre.co.uk/"));

        // Un `href` vers un autre hôte n'est pas suivi avec les identifiants
        // (refusé avant toute connexion).
        let auth = Auth::Basic { user: "moi".into(), password: "secret".into(), scope };
        let refused = dav("PROPFIND", "http://autre.example/p/", &auth, Some("0"), None).err().unwrap();
        assert!(refused.contains("http://autre.example/p/"), "{refused}");
        // Écritures aussi (relecture complète n° 3) : l'adresse d'un élément
        // vient du serveur.
        let put = put_response("http://autre.example/x.ics", &auth, "text/calendar", "BEGIN:VCALENDAR", "", false).err().unwrap();
        assert!(put.contains("autre.example"), "{put}");
        let removed = delete("http://autre.example/x.ics", &auth, "").err().unwrap();
        assert!(removed.contains("autre.example"), "{removed}");
    }

    #[test]
    fn s7_page_d_erreur_enorme_lue_vite() {
        let page = "<style>x{}</style>".repeat(500_000);
        let started = std::time::Instant::now();
        let _ = server_reason(page.as_bytes());
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
        // Le même travail sans limite de taille reste linéaire : 9 Mo en un passage.
        let started = std::time::Instant::now();
        assert_eq!(without_blocks(&page).trim(), "");
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn blocs_style_et_script_retires() {
        assert_eq!(without_blocks("a<STYLE>x</style>b<script src=x>y</SCRIPT>c<b>d</b>"), "a b c<b>d</b>");
        assert_eq!(without_blocks("a<style>sans fin"), "a ");
        assert_eq!(without_blocks("é<p>ü</p>"), "é<p>ü</p>");
    }

    #[test]
    fn server_reasons() {
        let google = br#"{"error": {"code": 403, "message": "CalDAV API has not been used in project 12 before or it is disabled.", "status": "PERMISSION_DENIED"}}"#;
        assert_eq!(server_reason(google), "CalDAV API has not been used in project 12 before or it is disabled.");
        assert_eq!(server_reason(b"<html><body><h1>Forbidden</h1>\n<p>Nope</p></body></html>"), "Forbidden Nope");
        assert_eq!(server_reason(b""), "");
        let google_502 = b"<!DOCTYPE html><html lang=en><meta charset=utf-8><title>Error 502 (Server Error)!!1</title>\
            <style>*{margin:0;padding:0}html,code{font:15px/22px arial,sans-serif}</style>\
            <p><b>502.</b> <ins>That\xe2\x80\x99s an error.</ins><p>The server encountered a temporary error.</html>";
        let reason = server_reason(google_502);
        assert!(!reason.contains("margin"), "sans la feuille de style : {reason}");
        assert!(reason.starts_with("Error 502 (Server Error)!!1 502."), "{reason}");
    }

    #[test]
    fn adresses_annoncees_par_le_dns() {
        let srv = |target: &str, port: u16| crate::dns::Srv { priority: 0, weight: 0, port, target: target.into() };
        let servers = vec![
            (srv("dav.exemple.fr.", 443), Some("/caldav/".to_string())),
            (srv("cal.exemple.fr", 8443), None),
            // Hors du domaine : un DNS falsifié ne détourne pas vers un autre site.
            (srv("pirate.example", 443), None),
            (srv("a b.exemple.fr", 443), None),
        ];
        assert_eq!(
            srv_urls("exemple.fr", Kind::Calendar, &servers),
            vec!["https://dav.exemple.fr/caldav/", "https://cal.exemple.fr:8443/.well-known/caldav"]
        );
    }

    #[test]
    fn start_urls_from_hint() {
        assert_eq!(
            start_urls("moi@mailo.com", Kind::Calendar),
            vec!["https://mailo.com/.well-known/caldav", "https://dav.mailo.com/.well-known/caldav"]
        );
        assert_eq!(start_urls("https://dav.exemple.fr/dav/", Kind::Contacts), vec!["https://dav.exemple.fr/dav/"]);
    }

    #[test]
    fn agendas_de_rendez_vous_seulement() {
        let doc = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
          <d:response><d:href>/taches/</d:href><d:propstat><d:prop><c:supported-calendar-component-set>
            <c:comp name="VTODO"/></c:supported-calendar-component-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/agenda/</d:href><d:propstat><d:prop><c:supported-calendar-component-set>
            <c:comp name="VEVENT"/><c:comp name="VTODO"/></c:supported-calendar-component-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/ancien/</d:href><d:propstat><d:prop><d:displayname>Ancien</d:displayname></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          </d:multistatus>"#;
        let root = xml::parse(doc);
        let kept: Vec<bool> = root.dav_descendants("response").into_iter().map(holds_events).collect();
        assert_eq!(kept, vec![false, true, true]);
    }

    #[test]
    fn p6_seuls_les_elements_changes_sont_relus() {
        let doc = r#"<d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/carnet/</d:href><d:propstat><d:prop><d:getetag>"c"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/carnet/meme.vcf</d:href><d:propstat><d:prop><d:getetag>"1"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/carnet/change.vcf</d:href><d:propstat><d:prop><d:getetag>"3"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/carnet/nouveau.vcf</d:href><d:propstat><d:prop><d:getetag>"1"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/carnet/sans-version.vcf</d:href><d:propstat><d:prop/><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/carnet/sous-dossier/</d:href><d:propstat><d:prop/><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          </d:multistatus>"#;
        let base = "https://dav.exemple.fr/carnet/";
        let root = xml::parse(doc);
        let at = |name: &str| format!("{base}{name}");
        let known: HashMap<String, String> =
            [("meme.vcf", "\"1\""), ("change.vcf", "\"2\""), ("sans-version.vcf", ""), ("supprime.vcf", "\"1\"")]
                .into_iter()
                .map(|(name, etag)| (at(name), etag.to_owned()))
                .collect();
        let (wanted, unchanged) = split_by_etag(listing(&root, base), &known);
        assert_eq!(unchanged, vec![at("meme.vcf")]);
        assert_eq!(wanted, vec![at("change.vcf"), at("nouveau.vcf"), at("sans-version.vcf")]);
    }

    #[test]
    fn items_and_props() {
        let doc = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
          <d:response><d:href>/cal/a.ics</d:href><d:propstat><d:prop><d:getetag>"e1"</d:getetag>
            <c:calendar-data>BEGIN:VCALENDAR
END:VCALENDAR</c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
            <d:propstat><d:prop><x:missing xmlns:x="X"/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
          </d:response></d:multistatus>"#;
        let root = xml::parse(doc);
        let items = items_from(&root, "https://dav.exemple.fr/cal/", "calendar-data");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].url, "https://dav.exemple.fr/cal/a.ics");
        assert_eq!(items[0].etag, "\"e1\"");
    }
}
