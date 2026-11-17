//! Réglages d'un compte trouvés d'après son adresse (lot I1), avant toute
//! connexion avec le mot de passe. Chaque étape ne sert que si les précédentes
//! n'ont pas tout trouvé :
//!
//! 1. fournisseurs connus de Vortex Mail, par le domaine de l'adresse ;
//! 2. copie intégrée de l'ISPDB de Thunderbird (`src-tauri/ispdb/`, MPL-2.0) :
//!    rien ne sort de l'ordinateur ;
//! 3. serveurs de réception du domaine (`MX`) : hébergeur connu (Google, O365,
//!    iCloud…), ou domaine de l'hébergeur dans la copie de l'ISPDB ;
//! 4. enregistrements `SRV` de la RFC 6186 (`_imaps._tcp`, `_submissions._tcp`,
//!    puis `_imap._tcp`, `_submission._tcp`) ;
//! 5. fichier publié par le domaine pour les logiciels de messagerie, en HTTPS
//!    seulement (`autoconfig.<domaine>`, puis `<domaine>/.well-known`), sans
//!    l'adresse ;
//! 6. noms courants (`imap.`, `mail.`, `smtp.`), essayés **sans mot de passe** :
//!    accueil du serveur et chiffrement seulement.
//!
//! Ce qui vient du réseau est hostile : documents plafonnés, champs bornés, un
//! serveur sans chiffrement n'est jamais retenu (SSL/TLS préféré à STARTTLS,
//! RFC 8314). Le DNS n'étant pas authentifié, un serveur trouvé par `MX` ou `SRV`
//! hors du domaine de l'adresse est signalé avant que le mot de passe y parte.
//! L'ISPDB n'est jamais interrogée en ligne (`autoconfig.thunderbird.net`
//! apprendrait le domaine de chaque compte ajouté).
//!
//! Ordre des étapes, table des fournisseurs et reconnaissance par `MX` inspirés
//! de `MailCraft` (`crates/model/src/provider.rs`, `crates/net/src/discover.rs`,
//! MIT OR Apache-2.0).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::Security;
use crate::xml::{self, Element};

/// Copie de l'ISPDB de Thunderbird, produite par `scripts/ispdb.py maj`.
const ISPDB: &str = include_str!("../ispdb/ispdb.xml");
/// Taille lue au plus d'un document publié par un domaine.
const MAX_DOCUMENT: usize = 256 * 1024;
/// Serveurs lus au plus par document, et longueur d'un champ.
const MAX_SERVERS: usize = 16;
const MAX_FIELD: usize = 512;
/// Durée totale de la recherche : un domaine dont les serveurs ne répondent pas
/// ne fait pas attendre une minute.
const BUDGET: Duration = Duration::from_secs(30);

/// D'où viennent les réglages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Source {
    /// Fournisseur connu de Vortex Mail.
    Builtin,
    /// Copie intégrée de l'ISPDB.
    Ispdb,
    /// Hébergeur reconnu par les serveurs de réception du domaine (`MX`).
    Mx,
    /// Enregistrements `SRV` du domaine.
    Srv,
    /// Document publié par le domaine.
    Domain,
    /// Noms courants, essayés sans mot de passe.
    Guess,
}

/// Comment l'utilisateur se connecte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Login {
    /// Mot de passe du compte.
    Password,
    /// Mot de passe d'application, créé dans les réglages de sécurité du compte.
    AppPassword,
    /// Connexion Google dans le navigateur (`OAuth2`).
    Google,
    /// Connexion Outlook.com dans le navigateur (`OAuth2`).
    Outlook,
    /// Proton Mail par Proton Mail Bridge, sur cet ordinateur (lot I3).
    ProtonBridge,
}

/// Un serveur trouvé.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Server {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

/// Réglages trouvés pour une adresse, à confirmer par l'utilisateur.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Detected {
    /// Source du serveur de réception.
    pub source: Source,
    /// Nom du fournisseur ; vide s'il n'est pas connu.
    pub provider: String,
    pub imap: Server,
    pub smtp: Server,
    /// Identifiant de connexion (l'adresse, le plus souvent).
    pub username: String,
    pub login: Login,
    /// Un serveur trouvé par le DNS est hors du domaine de l'adresse : à signaler.
    pub outside_domain: bool,
    /// Page d'aide du fournisseur (HTTPS), ou vide.
    pub help_url: String,
}

/// Ni l'identifiant ni les serveurs dans un journal.
impl std::fmt::Debug for Detected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Detected")
            .field("source", &self.source)
            .field("login", &self.login)
            .field("outside_domain", &self.outside_domain)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------- adresses et noms

/// Nom d'hôte utilisable : lettres, chiffres, tirets et points, avec un point.
fn is_domain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.contains('.')
        && name
            .split('.')
            .all(|label| !label.is_empty() && label.len() <= 63 && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// `host` est le domaine lui-même ou un nom sous lui (jamais un sosie : `evilgoogle.com`).
fn within(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|head| head.ends_with('.'))
}

/// Deuxième niveau d'un pays qui est lui-même un suffixe (`co.uk`, `com.au`) ; liste
/// courte reprise de `MailCraft`, pas la Public Suffix List.
const COUNTRY_SECOND_LEVELS: &[&str] = &["co", "com", "net", "org", "gov", "edu", "ac", "or", "ne", "go", "me"];

/// Domaine dont `host` est un nom : ses deux dernières étiquettes, trois sous un
/// suffixe de pays (`mx1.mail.ovh.net` → `ovh.net`, `mx.exemple.co.uk` → `exemple.co.uk`).
fn base_domain(host: &str) -> Option<String> {
    let labels: Vec<&str> = host.split('.').collect();
    let last = |n: usize| labels.get(labels.len().checked_sub(n)?..).map(|tail| tail.join("."));
    let two = last(2)?;
    let public = two.split_once('.').is_some_and(|(second, top)| top.len() == 2 && COUNTRY_SECOND_LEVELS.contains(&second));
    let base = if public { last(3)? } else { two };
    is_domain(&base).then_some(base)
}

/// Partie locale et domaine (en ASCII, Punycode compris, minuscules) d'une adresse.
fn split_address(email: &str) -> Option<(String, String)> {
    let wire = mail_core::compose::wire_address(email.trim())?;
    let (local, domain) = wire.rsplit_once('@')?;
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    (!local.is_empty() && !local.contains(['\r', '\n', ' ']) && is_domain(&domain)).then(|| (local.to_string(), domain))
}

/// Texte d'un document remplacé : `%EMAILADDRESS%`, `%EMAILLOCALPART%`, `%EMAILDOMAIN%`.
fn expand(template: &str, local: &str, domain: &str) -> String {
    template.replace("%EMAILADDRESS%", &format!("{local}@{domain}")).replace("%EMAILLOCALPART%", local).replace("%EMAILDOMAIN%", domain)
}

// ---------------------------------------------------------------- fournisseurs connus

fn server(host: &str, port: u16, security: Security) -> Server {
    Server { host: host.to_string(), port, security }
}

/// Réglages complets d'un fournisseur connu, pour l'adresse `local@domain`.
fn preset(source: Source, provider: &str, login: Login, imap: Server, smtp: Server, local: &str, domain: &str) -> Detected {
    Detected {
        source,
        provider: provider.to_string(),
        imap,
        smtp,
        username: format!("{local}@{domain}"),
        login,
        outside_domain: false,
        help_url: String::new(),
    }
}

/// Adresses Outlook.com et ses anciens noms, dans tous les pays (`hotmail.co.uk`,
/// `live.com.au` ; jamais `live.entreprise.example`).
fn is_outlook_consumer(domain: &str) -> bool {
    let mut labels = domain.split('.');
    let first = labels.next().unwrap_or_default();
    let rest: Vec<&str> = labels.collect();
    matches!(first, "outlook" | "hotmail" | "live" | "msn")
        && (1..=2).contains(&rest.len())
        && rest.iter().all(|l| (2..=3).contains(&l.len()))
}

fn google(source: Source, provider: &str, local: &str, domain: &str) -> Detected {
    let (imap, smtp) = (server("imap.gmail.com", 993, Security::SslTls), server("smtp.gmail.com", 465, Security::SslTls));
    preset(source, provider, Login::Google, imap, smtp, local, domain)
}

fn outlook(source: Source, local: &str, domain: &str) -> Detected {
    let (imap, smtp) = (server("outlook.office365.com", 993, Security::SslTls), server("smtp-mail.outlook.com", 587, Security::StartTls));
    preset(source, "Outlook.com", Login::Outlook, imap, smtp, local, domain)
}

fn proton(source: Source, local: &str, domain: &str) -> Detected {
    // Bridge sert IMAP et SMTP sur cet ordinateur, en STARTTLS (lot I3).
    let (imap, smtp) = (server("127.0.0.1", 1143, Security::StartTls), server("127.0.0.1", 1025, Security::StartTls));
    preset(source, "Proton Mail", Login::ProtonBridge, imap, smtp, local, domain)
}

/// Fournisseur à mot de passe : IMAP en SSL/TLS sur 993.
fn with_password(
    source: Source,
    provider: &str,
    login: Login,
    imap: &str,
    smtp: (&str, u16, Security),
    local: &str,
    domain: &str,
) -> Detected {
    preset(source, provider, login, server(imap, 993, Security::SslTls), server(smtp.0, smtp.1, smtp.2), local, domain)
}

/// Fournisseur connu par le domaine de l'adresse.
fn builtin(local: &str, domain: &str) -> Option<Detected> {
    let (tls, starttls, b) = (Security::SslTls, Security::StartTls, Source::Builtin);
    if is_outlook_consumer(domain) {
        return Some(outlook(b, local, domain));
    }
    Some(match domain {
        "gmail.com" | "googlemail.com" => google(b, "Gmail", local, domain),
        "proton.me" | "pm.me" | "protonmail.com" | "protonmail.ch" => proton(b, local, domain),
        "mailo.com" => with_password(b, "Mailo", Login::Password, "mail.mailo.com", ("mail.mailo.com", 465, tls), local, domain),
        "icloud.com" | "me.com" | "mac.com" => {
            with_password(b, "iCloud", Login::AppPassword, "imap.mail.me.com", ("smtp.mail.me.com", 587, starttls), local, domain)
        }
        "fastmail.com" | "fastmail.fm" => {
            with_password(b, "Fastmail", Login::AppPassword, "imap.fastmail.com", ("smtp.fastmail.com", 465, tls), local, domain)
        }
        "aol.com" => with_password(b, "AOL", Login::AppPassword, "imap.aol.com", ("smtp.aol.com", 465, tls), local, domain),
        "zoho.com" | "zohomail.com" => {
            with_password(b, "Zoho", Login::Password, "imap.zoho.com", ("smtp.zoho.com", 465, tls), local, domain)
        }
        "gmx.com" | "gmx.us" => with_password(b, "GMX", Login::Password, "imap.gmx.com", ("mail.gmx.com", 587, starttls), local, domain),
        "gmx.net" | "gmx.de" | "gmx.at" | "gmx.ch" => {
            with_password(b, "GMX", Login::Password, "imap.gmx.net", ("mail.gmx.net", 587, starttls), local, domain)
        }
        "mail.com" => with_password(b, "mail.com", Login::Password, "imap.mail.com", ("smtp.mail.com", 587, starttls), local, domain),
        d if d == "ymail.com" || d == "rocketmail.com" || d.starts_with("yahoo.") => {
            with_password(b, "Yahoo", Login::AppPassword, "imap.mail.yahoo.com", ("smtp.mail.yahoo.com", 465, tls), local, domain)
        }
        _ => return None,
    })
}

/// Domaine hébergé chez O365 (comptes professionnels et scolaires) : lot C.
fn o365_unsupported() -> String {
    tr!("Ce domaine est hébergé par O365 : les comptes professionnels et scolaires ne sont pas encore pris en charge.").into()
}

/// Hébergeur reconnu par un serveur de réception du domaine
/// (`exemple-org.mail.protection.outlook.com`). `Err` : O365, pas pris en charge.
fn by_mx(host: &str, local: &str, domain: &str) -> Result<Option<Detected>, String> {
    let under = |parent: &str| within(host, parent);
    let (tls, starttls, m) = (Security::SslTls, Security::StartTls, Source::Mx);
    // Anciens domaines personnels d'Outlook.com, qui gardent un MX propre à Hotmail.
    if under("olc.protection.outlook.com") || under("pamx1.hotmail.com") {
        return Ok(Some(outlook(m, local, domain)));
    }
    if under("mail.protection.outlook.com") || under("mx.microsoft") {
        return Err(o365_unsupported());
    }
    Ok(Some(if under("google.com") || under("googlemail.com") {
        google(m, "Google Workspace", local, domain)
    } else if under("protonmail.ch") {
        proton(m, local, domain)
    } else if under("yahoodns.net") {
        with_password(m, "Yahoo", Login::AppPassword, "imap.mail.yahoo.com", ("smtp.mail.yahoo.com", 465, tls), local, domain)
    } else if under("mail.icloud.com") {
        with_password(m, "iCloud", Login::AppPassword, "imap.mail.me.com", ("smtp.mail.me.com", 587, starttls), local, domain)
    } else if under("messagingengine.com") {
        with_password(m, "Fastmail", Login::AppPassword, "imap.fastmail.com", ("smtp.fastmail.com", 465, tls), local, domain)
    } else if under("zoho.com") || under("zoho.eu") {
        with_password(m, "Zoho", Login::Password, "imap.zoho.com", ("smtp.zoho.com", 465, tls), local, domain)
    } else {
        return Ok(None);
    }))
}

/// Serveur IMAP d'Outlook.com ou d'O365.
fn is_microsoft_imap(host: &str) -> bool {
    matches!(host, "outlook.office365.com" | "outlook.office.com" | "imap-mail.outlook.com")
}

/// Contrôle commun à toutes les sources : un serveur Google demande la connexion
/// Google ; un serveur de Microsoft pour un domaine qui n'est pas d'Outlook.com, O365.
fn classify(mut found: Detected) -> Result<Detected, String> {
    if found.imap.host == "imap.gmail.com" && found.login != Login::Google {
        found.login = Login::Google;
    }
    if is_microsoft_imap(&found.imap.host) && found.login != Login::Outlook {
        return Err(o365_unsupported());
    }
    Ok(found)
}

// ---------------------------------------------------------------- documents `clientConfig`

/// Un serveur tel qu'un document le décrit.
#[derive(Default)]
struct Listed {
    hostname: String,
    port: String,
    socket_type: String,
    username: String,
    auth: Vec<String>,
}

/// Texte d'un champ, refusé s'il est démesuré ou contient un caractère de contrôle.
fn field(e: &Element, name: &str) -> String {
    let text = e.child(name).map_or_default(|c| c.text.trim().to_string());
    if text.len() > MAX_FIELD || text.chars().any(char::is_control) {
        return String::new();
    }
    text
}

fn listed(e: &Element) -> Listed {
    Listed {
        hostname: field(e, "hostname"),
        port: field(e, "port"),
        socket_type: field(e, "sockettype"),
        username: field(e, "username"),
        auth: e
            .children
            .iter()
            .filter(|c| c.name == "authentication")
            .map(|c| c.text.trim().to_ascii_lowercase())
            .take(MAX_SERVERS)
            .collect(),
    }
}

impl Listed {
    /// Le serveur, s'il est chiffré, sur un hôte valable, et accepte un mot de passe
    /// (ou la connexion Google, la seule `OAuth2` que Vortex Mail sait faire ici).
    fn server(&self, local: &str, domain: &str, usual: (u16, u16)) -> Option<Server> {
        let host = expand(&self.hostname, local, domain).trim_end_matches('.').to_ascii_lowercase();
        if !is_domain(&host) {
            return None;
        }
        let security = match self.socket_type.to_ascii_uppercase().as_str() {
            "SSL" => Security::SslTls,
            "STARTTLS" => Security::StartTls,
            // `plain` : jamais retenu.
            _ => return None,
        };
        let password = self.auth.is_empty() || self.auth.iter().any(|a| matches!(a.as_str(), "password-cleartext" | "password-encrypted"));
        let google = host == "imap.gmail.com" || host == "smtp.gmail.com";
        if !(password || (google && self.auth.iter().any(|a| a == "oauth2"))) {
            return None;
        }
        let port = match self.port.as_str() {
            "" if security == Security::SslTls => usual.0,
            "" => usual.1,
            digits => digits.parse::<u16>().ok().filter(|p| *p != 0)?,
        };
        Some(Server { host, port, security })
    }
}

/// Le meilleur des serveurs d'un type : SSL/TLS avant STARTTLS, sinon le premier.
fn best(servers: &[Listed], local: &str, domain: &str, usual: (u16, u16)) -> Option<(Server, String)> {
    let usable: Vec<(Server, String)> =
        servers.iter().take(MAX_SERVERS).filter_map(|l| l.server(local, domain, usual).map(|s| (s, l.username.clone()))).collect();
    usable.iter().find(|(s, _)| s.security == Security::SslTls).or_else(|| usable.first()).cloned()
}

/// Ce qu'un élément `emailProvider` dit des serveurs : (IMAP et identifiant, SMTP, nom, aide).
struct Provided {
    imap: Option<(Server, String)>,
    smtp: Option<Server>,
    provider: String,
    help_url: String,
}

fn provided(provider: &Element, local: &str, domain: &str) -> Provided {
    let typed = |element: &str, kind: &str| -> Vec<Listed> {
        provider
            .children
            .iter()
            .filter(|c| c.name == element && c.attr("type").is_some_and(|t| t.trim().eq_ignore_ascii_case(kind)))
            .take(MAX_SERVERS)
            .map(listed)
            .collect()
    };
    let imap = best(&typed("incomingserver", "imap"), local, domain, (993, 143));
    let smtp = best(&typed("outgoingserver", "smtp"), local, domain, (465, 587)).map(|(s, _)| s);
    let name = field(provider, "displayname");
    let help_url = provider
        .children
        .iter()
        .filter(|c| c.name == "documentation")
        .filter_map(|c| c.attr("url"))
        .find(|u| u.starts_with("https://") && u.len() <= MAX_FIELD && !u.chars().any(|c| c.is_control() || c.is_whitespace()))
        .unwrap_or_default()
        .to_string();
    Provided { imap, smtp, provider: name.chars().take(100).collect(), help_url }
}

/// Réglages complets d'un `emailProvider`, s'il nomme un serveur IMAP et un serveur SMTP.
fn complete(provider: &Element, source: Source, local: &str, domain: &str) -> Option<Detected> {
    let p = provided(provider, local, domain);
    let ((imap, username), smtp) = (p.imap?, p.smtp?);
    let username = if username.is_empty() { format!("{local}@{domain}") } else { expand(&username, local, domain) };
    Some(Detected {
        source,
        provider: p.provider,
        imap,
        smtp,
        username,
        login: Login::Password,
        outside_domain: false,
        help_url: p.help_url,
    })
}

/// L'élément `emailProvider` d'un document `clientConfig` ; `None` s'il n'en a pas.
fn email_provider(document: &Element) -> Option<&Element> {
    document.child("clientconfig")?.child("emailprovider")
}

// ---------------------------------------------------------------- copie de l'ISPDB

/// La copie de l'ISPDB lue : ses fournisseurs, et l'index de chacun par domaine.
struct Ispdb {
    providers: Vec<Element>,
    by_domain: HashMap<String, usize>,
}

impl Ispdb {
    /// Lit une copie (`<ispdb>` contenant des `<clientConfig>`).
    fn read(text: &str) -> Self {
        let root = xml::parse(text);
        let mut providers = Vec::new();
        let mut by_domain = HashMap::new();
        let configs = root.child("ispdb").map_or_default(|db| db.children.iter().filter(|c| c.name == "clientconfig").collect::<Vec<_>>());
        for config in configs {
            let Some(provider) = config.child("emailprovider") else { continue };
            let index = providers.len();
            for d in provider.children.iter().filter(|c| c.name == "domain") {
                let name = d.text.trim().trim_end_matches('.').to_ascii_lowercase();
                if is_domain(&name) {
                    by_domain.entry(name).or_insert(index);
                }
            }
            providers.push(provider.clone());
        }
        Ispdb { providers, by_domain }
    }

    /// Le fournisseur d'un domaine.
    fn get(&self, domain: &str) -> Option<&Element> {
        self.providers.get(*self.by_domain.get(domain)?)
    }
}

/// La copie intégrée, lue au premier besoin.
fn ispdb() -> &'static Ispdb {
    static CELL: OnceLock<Ispdb> = OnceLock::new();
    CELL.get_or_init(|| Ispdb::read(ISPDB))
}

// ---------------------------------------------------------------- réseau

/// Réglages trouvés au fil des étapes réseau : chaque serveur garde sa source.
#[derive(Default)]
struct Partial {
    imap: Option<(Server, Source)>,
    smtp: Option<(Server, Source)>,
    username: String,
    provider: String,
    help_url: String,
}

impl Partial {
    fn done(&self) -> bool {
        self.imap.is_some() && self.smtp.is_some()
    }

    /// Complète ce qui manque avec un document ; l'identifiant suit le serveur IMAP.
    fn take(&mut self, p: Provided, source: Source) {
        if self.imap.is_none() {
            if let Some((server, username)) = p.imap {
                self.imap = Some((server, source));
                self.username = username;
                self.provider = p.provider;
                self.help_url = p.help_url;
            }
        }
        if self.smtp.is_none() {
            if let Some(server) = p.smtp {
                self.smtp = Some((server, source));
            }
        }
    }

    fn finish(self, local: &str, domain: &str) -> Option<Detected> {
        let ((imap, source), (smtp, smtp_source)) = (self.imap?, self.smtp?);
        // Le DNS n'est pas authentifié : un serveur qu'il désigne hors du domaine est signalé.
        let doubtful = |server: &Server, source: Source| matches!(source, Source::Srv | Source::Mx) && !within(&server.host, domain);
        let outside_domain = doubtful(&imap, source) || doubtful(&smtp, smtp_source);
        let username = if self.username.is_empty() { format!("{local}@{domain}") } else { expand(&self.username, local, domain) };
        Some(Detected {
            source,
            provider: self.provider,
            imap,
            smtp,
            username,
            login: Login::Password,
            outside_domain,
            help_url: self.help_url,
        })
    }
}

/// Services de la RFC 6186, SSL/TLS d'abord (RFC 8314).
const IMAP_SERVICES: [(&str, Security); 2] = [("_imaps._tcp", Security::SslTls), ("_imap._tcp", Security::StartTls)];
const SMTP_SERVICES: [(&str, Security); 2] = [("_submissions._tcp", Security::SslTls), ("_submission._tcp", Security::StartTls)];

/// Le meilleur serveur annoncé : le premier service qui en a un l'emporte.
fn from_srv(found: &[(Security, Vec<crate::dns::Srv>)]) -> Option<Server> {
    found.iter().find_map(|(security, records)| {
        records.iter().find(|r| r.port != 0 && is_domain(&r.target)).map(|r| Server {
            host: r.target.clone(),
            port: r.port,
            security: *security,
        })
    })
}

fn srv(services: &[(&str, Security)], domain: &str) -> Option<Server> {
    let found: Vec<(Security, Vec<crate::dns::Srv>)> =
        services.iter().map(|(service, security)| (*security, crate::dns::srv_records(service, domain))).collect();
    from_srv(&found)
}

/// Document `clientConfig` publié par le domaine, en HTTPS seulement, sans l'adresse.
fn domain_document(domain: &str, local: &str) -> Option<Provided> {
    let places = [
        format!("https://autoconfig.{domain}/mail/config-v1.1.xml"),
        format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
    ];
    places.iter().find_map(|url| {
        let response = crate::http::get(url, MAX_DOCUMENT).ok().filter(|r| r.status == 200)?;
        let text = String::from_utf8_lossy(&response.body);
        let document = xml::parse(&text);
        let p = provided(email_provider(&document)?, local, domain);
        (p.imap.is_some() || p.smtp.is_some()).then_some(p)
    })
}

/// Noms courants, essayés en parallèle sans mot de passe ; le premier qui répond, dans l'ordre.
fn guess(domain: &str, imap: bool) -> Option<Server> {
    let candidates: Vec<Server> = if imap {
        vec![
            server(&format!("imap.{domain}"), 993, Security::SslTls),
            server(&format!("mail.{domain}"), 993, Security::SslTls),
            server(&format!("imap.{domain}"), 143, Security::StartTls),
            server(&format!("mail.{domain}"), 143, Security::StartTls),
        ]
    } else {
        vec![
            server(&format!("smtp.{domain}"), 465, Security::SslTls),
            server(&format!("mail.{domain}"), 465, Security::SslTls),
            server(&format!("smtp.{domain}"), 587, Security::StartTls),
            server(&format!("mail.{domain}"), 587, Security::StartTls),
        ]
    };
    let answers: Vec<bool> = std::thread::scope(|scope| {
        let probes: Vec<_> = candidates
            .iter()
            .map(|s| {
                scope.spawn(move || {
                    if imap {
                        crate::transport::probe_imap(&s.host, s.port, s.security)
                    } else {
                        crate::transport::probe_smtp(&s.host, s.port, s.security)
                    }
                })
            })
            .collect();
        probes.into_iter().map(|p| p.join().unwrap_or(false)).collect()
    });
    candidates.into_iter().zip(answers).find_map(|(s, ok)| ok.then_some(s))
}

/// Les réglages de `email`, trouvés comme le dit l'en-tête du module.
///
/// # Errors
///
/// Adresse invalide, domaine hébergé chez O365, ou rien trouvé.
pub(crate) fn detect(email: &str) -> Result<Detected, String> {
    let started = Instant::now();
    let (local, domain) = split_address(email).ok_or_else(|| tr!("Adresse invalide.").to_string())?;
    if let Some(found) = builtin(&local, &domain) {
        return Ok(found);
    }
    if let Some(found) = ispdb().get(&domain).and_then(|p| complete(p, Source::Ispdb, &local, &domain)) {
        return classify(found);
    }
    let exchangers = crate::dns::mx(&domain);
    for host in exchangers.iter().take(4) {
        if let Some(found) = by_mx(host, &local, &domain)? {
            return Ok(found);
        }
    }
    // Domaine de l'hébergeur dans la copie de l'ISPDB (`ovh.net` pour `mx1.mail.ovh.net`).
    let hoster = exchangers.first().and_then(|host| base_domain(host)).filter(|h| *h != domain);
    if let Some(found) = hoster.as_deref().and_then(|h| ispdb().get(h)).and_then(|p| complete(p, Source::Mx, &local, &domain)) {
        let outside_domain = !within(&found.imap.host, &domain) || !within(&found.smtp.host, &domain);
        return classify(Detected { outside_domain, ..found });
    }
    let mut partial = Partial { imap: srv(&IMAP_SERVICES, &domain).map(|s| (s, Source::Srv)), ..Partial::default() };
    partial.smtp = srv(&SMTP_SERVICES, &domain).map(|s| (s, Source::Srv));
    if !partial.done() && started.elapsed() < BUDGET {
        if let Some(p) = domain_document(&domain, &local) {
            partial.take(p, Source::Domain);
        }
    }
    if partial.imap.is_none() && started.elapsed() < BUDGET {
        partial.imap = guess(&domain, true).map(|s| (s, Source::Guess));
    }
    if partial.smtp.is_none() && started.elapsed() < BUDGET {
        partial.smtp = guess(&domain, false).map(|s| (s, Source::Guess));
    }
    match partial.finish(&local, &domain) {
        Some(found) => classify(found),
        None => Err(tr!("Aucun réglage trouvé pour {domain} : saisissez les serveurs à la main.", domain = domain)),
    }
}

/// Lecture d'un document `clientConfig` quelconque, pour le fuzzing : comme celui
/// d'un domaine, puis comme une copie de l'ISPDB.
pub(crate) fn fuzz_document(text: &str) {
    let document = xml::parse(text);
    if let Some(provider) = email_provider(&document) {
        let _ = complete(provider, Source::Domain, "moi", "exemple.fr").map(classify);
        let mut partial = Partial::default();
        partial.take(provided(provider, "moi", "exemple.fr"), Source::Domain);
        let _ = partial.finish("moi", "exemple.fr");
    }
    let db = Ispdb::read(text);
    for name in db.by_domain.keys().take(4) {
        let _ = db.get(name).and_then(|p| complete(p, Source::Ispdb, "moi", name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="free.fr">
    <domain>free.fr</domain>
    <displayName>Freebox</displayName>
    <incomingServer type="pop3">
      <hostname>pop.free.fr</hostname><port>995</port><socketType>SSL</socketType>
      <username>%EMAILLOCALPART%</username><authentication>password-cleartext</authentication>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>imap.free.fr</hostname><port>143</port><socketType>STARTTLS</socketType>
      <username>%EMAILLOCALPART%</username><authentication>password-cleartext</authentication>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>imap.free.fr</hostname><port>993</port><socketType>SSL</socketType>
      <username>%EMAILLOCALPART%</username><authentication>password-cleartext</authentication>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.free.fr</hostname><port>25</port><socketType>plain</socketType>
      <authentication>password-cleartext</authentication>
    </outgoingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.%EMAILDOMAIN%</hostname><socketType>SSL</socketType>
      <authentication>password-cleartext</authentication>
    </outgoingServer>
    <documentation url="https://assistance.free.fr/aide"><descr lang="fr">Aide</descr></documentation>
  </emailProvider>
</clientConfig>"#;

    fn tls(host: &str, port: u16) -> Server {
        server(host, port, Security::SslTls)
    }

    #[test]
    fn i1_fournisseurs_connus() {
        let g = builtin("moi", "gmail.com").expect("Gmail");
        assert_eq!((g.login, g.imap.clone(), g.username.as_str()), (Login::Google, tls("imap.gmail.com", 993), "moi@gmail.com"));
        for domain in ["outlook.com", "hotmail.co.uk", "live.com.au", "msn.com"] {
            assert_eq!(builtin("moi", domain).map(|d| d.login), Some(Login::Outlook), "{domain}");
        }
        for domain in ["live.entreprise.example", "outlook.exemple.org", "exemple.fr"] {
            assert_eq!(builtin("moi", domain), None, "{domain}");
        }
        let p = builtin("moi", "proton.me").expect("Proton");
        assert_eq!((p.login, p.imap.port, p.smtp.port), (Login::ProtonBridge, 1143, 1025));
        assert_eq!(builtin("moi", "yahoo.fr").map(|d| d.login), Some(Login::AppPassword));
    }

    #[test]
    fn i1_hebergeur_par_mx() {
        let g = by_mx("aspmx.l.google.com", "moi", "entreprise.example").unwrap().expect("Google");
        assert_eq!((g.source, g.provider.as_str(), g.username.as_str()), (Source::Mx, "Google Workspace", "moi@entreprise.example"));
        assert!(by_mx("entreprise-example.mail.protection.outlook.com", "moi", "entreprise.example").is_err(), "O365 : lot C");
        assert_eq!(by_mx("outlook-com.olc.protection.outlook.com", "moi", "x.example").unwrap().map(|d| d.login), Some(Login::Outlook));
        assert_eq!(by_mx("mail.protonmail.ch", "moi", "x.example").unwrap().map(|d| d.login), Some(Login::ProtonBridge));
        for host in ["evilgoogle.com", "mail.protection.outlook.com.evil.example", "mx.exemple.fr"] {
            assert_eq!(by_mx(host, "moi", "x.example").unwrap(), None, "{host}");
        }
    }

    #[test]
    fn i1_document_client_config() {
        let document = xml::parse(FREE);
        let found = complete(email_provider(&document).expect("emailProvider"), Source::Domain, "moi", "free.fr").expect("réglages");
        assert_eq!(found.imap, tls("imap.free.fr", 993), "SSL/TLS préféré, POP3 ignoré");
        assert_eq!(found.smtp, tls("smtp.free.fr", 465), "sans chiffrement écarté, port usuel, %EMAILDOMAIN%");
        assert_eq!(found.username, "moi", "%EMAILLOCALPART%");
        assert_eq!((found.provider.as_str(), found.help_url.as_str()), ("Freebox", "https://assistance.free.fr/aide"));
        // Seulement OAuth2, hors Google : écarté ; sans chiffrement seulement : rien.
        let oauth = FREE.replace("password-cleartext", "OAuth2");
        assert!(complete(email_provider(&xml::parse(&oauth)).unwrap(), Source::Domain, "moi", "free.fr").is_none());
        let plain = FREE.replace("SSL", "plain").replace("STARTTLS", "plain");
        assert!(complete(email_provider(&xml::parse(&plain)).unwrap(), Source::Domain, "moi", "free.fr").is_none());
        // Hôte invalide, port nul, champ démesuré.
        let bad = FREE.replace("imap.free.fr", "imap free.fr").replace("<port>995</port>", "<port>0</port>");
        assert!(complete(email_provider(&xml::parse(&bad)).unwrap(), Source::Domain, "moi", "free.fr").is_none());
        assert_eq!(field(&xml::parse(&format!("<a><b>{}</b></a>", "x".repeat(600))).children[0], "b"), "");
    }

    #[test]
    fn i1_copie_ispdb() {
        let text = format!("<ispdb date=\"2026-11-16\">{}<clientConfig><emailProvider><domain>Autre.example</domain></emailProvider></clientConfig></ispdb>", FREE.replace(r#"<?xml version="1.0" encoding="UTF-8"?>"#, ""));
        let db = Ispdb::read(&text);
        assert_eq!(db.providers.len(), 2);
        assert!(db.get("free.fr").is_some() && db.get("autre.example").is_some() && db.get("exemple.fr").is_none());
        // La copie intégrée se lit, et chaque fournisseur a un domaine.
        assert!(ispdb().providers.len() > 100, "copie intégrée presque vide : lancer scripts/ispdb.py maj");
        let date =
            xml::parse(ISPDB.get(..4096).unwrap_or(ISPDB)).child("ispdb").and_then(|db| db.attr("date")).unwrap_or_default().to_string();
        assert_eq!(date.len(), 10, "date de la copie : {date}");
    }

    #[test]
    fn i1_dns_et_hors_domaine() {
        let r = |target: &str, port: u16| crate::dns::Srv { priority: 0, weight: 0, port, target: target.into() };
        let found = vec![(Security::SslTls, vec![]), (Security::StartTls, vec![r("bad name", 143), r("imap.hebergeur.example", 143)])];
        assert_eq!(from_srv(&found), Some(server("imap.hebergeur.example", 143, Security::StartTls)));
        let mut partial =
            Partial { imap: Some((server("imap.hebergeur.example", 993, Security::SslTls), Source::Srv)), ..Partial::default() };
        partial.smtp = Some((server("smtp.exemple.fr", 465, Security::SslTls), Source::Guess));
        let found = partial.finish("moi", "exemple.fr").expect("complet");
        assert!(found.outside_domain, "serveur annoncé par le DNS hors du domaine");
        assert_eq!(found.username, "moi@exemple.fr");
        assert_eq!(base_domain("mx1.mail.ovh.net").as_deref(), Some("ovh.net"));
        assert_eq!(base_domain("mx.exemple.co.uk").as_deref(), Some("exemple.co.uk"));
        assert!(within("imap.exemple.fr", "exemple.fr") && !within("imap.mauvais-exemple.fr", "exemple.fr"));
    }

    /// Essai réel, à la demande seulement (réseau) :
    /// `VORTEX_ESSAI_ADRESSES=a@x.fr,b@y.com cargo test --manifest-path src-tauri/Cargo.toml i1_essai_reel -- --nocapture`.
    #[test]
    fn i1_essai_reel() {
        let Ok(addresses) = std::env::var("VORTEX_ESSAI_ADRESSES") else { return };
        for address in addresses.split(',') {
            let found = detect(address.trim()).map(|d| (d.source, d.provider, d.imap, d.smtp, d.username, d.login, d.outside_domain));
            println!("{address} : {found:?}");
        }
    }

    #[test]
    fn i1_adresse_et_classement() {
        assert_eq!(split_address(" Moi@Bücher.Example "), Some(("Moi".into(), "xn--bcher-kva.example".into())));
        assert_eq!(split_address("pas-une-adresse"), None);
        assert_eq!(split_address("moi@localhost"), None);
        let mut d = google(Source::Ispdb, "", "moi", "x.example");
        d.login = Login::Password;
        assert_eq!(classify(d).map(|d| d.login), Ok(Login::Google));
        let o365 = preset(
            Source::Ispdb,
            "",
            Login::Password,
            tls("outlook.office365.com", 993),
            tls("smtp.office365.com", 587),
            "moi",
            "x.example",
        );
        assert!(classify(o365).is_err());
    }
}
