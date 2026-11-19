//! Client HTTP(S) minimal pour télécharger les images distantes des messages
//! (sans dépendance supplémentaire : TCP + `native-tls`).
//!
//! Durcissements du lot 5 (relecture A1, A2, A7) :
//! - adresses refusées si elles contiennent un caractère de contrôle (pas
//!   d'injection de requête) ;
//! - images d'un mail : aucune adresse locale ou privée, vérifiée à chaque
//!   redirection, connexion à l'adresse vérifiée elle-même ;
//! - redirections : `Authorization` et cookies jamais renvoyés à un autre
//!   hôte, https → http refusé ;
//! - réponse tronquée = erreur, jamais une réponse complète.
//!
//! Audit P7 : vers les serveurs de l'utilisateur (`WebDAV`, Outlook.com), la
//! connexion reste ouverte pour les requêtes suivantes (réserve `pool`) ;
//! chaque réponse est lue jusqu'à sa fin annoncée, sans attendre la fermeture.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Images d'un mail : on n'attend pas longtemps.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Serveurs de l'utilisateur (DAV, Outlook.com) : une écriture peut être lente.
const SERVER_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;

/// Adresses joignables : `Any` pour les serveurs configurés par l'utilisateur
/// (un serveur DAV du réseau local reste joignable), `PublicOnly` pour les
/// adresses venues d'un mail (images distantes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reach {
    Any,
    PublicOnly,
}

/// En-têtes d'identification, jamais renvoyés à un autre hôte.
const CREDENTIAL_HEADERS: [&str; 3] = ["authorization", "cookie", "proxy-authorization"];

pub(crate) struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Découpe `http(s)://hôte[:port]/chemin` (les URL relatives au protocole sont en HTTPS).
pub(crate) fn split_url(url: &str) -> Result<(bool, String, u16, String), String> {
    let url = url.trim();
    // CR, LF et autres caractères de contrôle : ils finiraient dans la ligne
    // de requête (requêtes injectées, relecture A2).
    if url.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(tr!("adresse invalide (caractère de contrôle)").into());
    }
    let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else if let Some(r) = url.strip_prefix("//") {
        (true, r)
    } else {
        return Err(tr!("adresse non prise en charge : {url}", url = url));
    };
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = path.split('#').next().unwrap_or("/");
    let path = if path.starts_with('?') { format!("/{path}") } else { path.to_string() };
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
            (h.to_string(), p.parse().map_err(|_| tr!("port invalide"))?)
        }
        _ => (authority.to_string(), if https { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(tr!("adresse sans hôte").into());
    }
    if host.contains([' ', '\\']) {
        return Err(tr!("adresse invalide : {url}", url = url));
    }
    let path = path.replace(' ', "%20");
    Ok((https, host, port, if path.is_empty() { "/".into() } else { path }))
}

/// Réponse brute → code, en-têtes, corps (décodé si `chunked`).
pub(crate) fn parse_response(raw: &[u8]) -> Result<Response, String> {
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or(tr!("réponse HTTP illisible"))?;
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let mut lines = head.lines();
    let status: u16 =
        lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok()).ok_or(tr!("code HTTP illisible"))?;
    let headers: Vec<(String, String)> =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let body = &raw[head_end + 4..];
    let chunked = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked"));
    let body = if chunked {
        // Exige le bloc final de taille 0 : sinon, réponse tronquée.
        crate::oauth::decode_chunked(body)?
    } else if let Some(length) = content_length(&headers)? {
        if body.len() < length {
            return Err(tr!("réponse HTTP tronquée").into());
        }
        body[..length].to_vec()
    } else {
        body.to_vec()
    };
    Ok(Response { status, headers, body })
}

/// Valeur de `Content-Length`, si l'en-tête est présent.
fn content_length(headers: &[(String, String)]) -> Result<Option<usize>, String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.trim().parse::<usize>().map_err(|_| tr!("longueur de réponse HTTP invalide").to_string()))
        .transpose()
}

/// La réponse porte sa propre longueur (`Content-Length`, `chunked`) ou n'a
/// pas de corps : elle a été vérifiée complète par `parse_response`.
fn self_delimited(response: &Response) -> bool {
    matches!(response.status, 100..=199 | 204 | 304)
        || response.header("content-length").is_some()
        || response.header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
}

/// Adresse joignable depuis Internet (ni bouclage, ni réseau privé, ni
/// link-local, ni plage réservée) ; les images d'un mail n'en visent pas d'autre.
pub(crate) fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // CGNAT
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (b == 18 || b == 19)) // bancs de test
        || a >= 240)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let seg = ip.segments();
    // NAT64 (64:ff9b::/96) et 6to4 (2002::/16) : l'adresse IPv4 embarquée décide.
    if seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0] {
        let [a, b] = seg[6].to_be_bytes();
        let [c, d] = seg[7].to_be_bytes();
        return is_public_v4(Ipv4Addr::new(a, b, c, d));
    }
    if seg[0] == 0x2002 {
        let [a, b] = seg[1].to_be_bytes();
        let [c, d] = seg[2].to_be_bytes();
        return is_public_v4(Ipv4Addr::new(a, b, c, d));
    }
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (seg[0] & 0xfe00) == 0xfc00 // adresses locales uniques
        || (seg[0] & 0xffc0) == 0xfe80 // link-local
        || (seg[0] & 0xffc0) == 0xfec0 // site-local (obsolète, encore routé localement)
        || (seg[0] == 0x64 && seg[1] == 0xff9b && seg[2] == 1) // NAT64 à usage local
        || (seg[0] == 0x2001 && seg[1] == 0) // Teredo
        || (seg[0] == 0x0100 && seg[1..4] == [0, 0, 0]) // préfixe de rejet
        || (seg[0] & 0xfff0) == 0x3ff0 // documentation (3fff::/20)
        || (seg[0] == 0x2001 && seg[1] == 0x0db8) // documentation
        || seg[..6] == [0, 0, 0, 0, 0, 0]) // IPv4 compatible (obsolète)
}

/// Résout l'hôte et choisit l'adresse de connexion. En `PublicOnly`, toutes
/// les adresses résolues doivent être publiques, et la connexion se fait à
/// l'adresse vérifiée (pas de seconde résolution qui en donnerait une autre).
/// Adresses de `host`, toutes vérifiées : une seule adresse locale ou privée
/// fait refuser un hôte d'image (relecture A2).
fn resolve_addrs(host: &str, port: u16, reach: Reach) -> Result<Vec<SocketAddr>, String> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<SocketAddr> =
        (bare, port).to_socket_addrs().map_err(|e| tr!("serveur « {host} » introuvable : {e}", host = host, e = e))?.collect();
    if addrs.is_empty() {
        return Err(tr!("serveur « {host} » introuvable", host = host));
    }
    if reach == Reach::PublicOnly && addrs.iter().any(|a| !is_public(a.ip())) {
        return Err(tr!("adresse locale ou privée refusée : {host}", host = host));
    }
    Ok(addrs)
}

/// Première adresse de `host` qui répond (audit S7 : seule la première était
/// essayée ; une adresse IPv6 injoignable rendait le serveur injoignable).
fn connect_any(host: &str, addrs: &[SocketAddr], timeout: Duration) -> Result<TcpStream, String> {
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect_timeout(addr, timeout) {
            Ok(tcp) => return Ok(tcp),
            Err(e) => last = Some(e),
        }
    }
    Err(last.map_or_else(|| tr!("serveur « {host} » introuvable", host = host), |e| format!("{host} : {e}")))
}

/// Lit la réponse ; le booléen dit si la connexion s'est terminée proprement.
/// `total` borne la durée de toute la lecture (audit S7) : le délai de chaque
/// lecture ne suffit pas contre un serveur qui envoie au goutte-à-goutte.
pub(crate) fn read_capped<S: Read>(mut stream: S, max_bytes: usize, total: Duration) -> Result<(Vec<u8>, bool), String> {
    let started = std::time::Instant::now();
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        if started.elapsed() > total {
            return Err(tr!("le serveur n'a pas répondu à temps").into());
        }
        match stream.read(&mut buf) {
            Ok(0) => return Ok((raw, true)),
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > max_bytes + 16 * 1024 {
                    return Err(tr!("réponse trop volumineuse").into());
                }
            }
            // Coupure (ou fermeture TLS sans `close_notify`) : ce qui est
            // arrivé ne compte que si la réponse prouve qu'elle est complète.
            Err(_) if !raw.is_empty() => return Ok((raw, false)),
            // Délai dépassé (« Ressource temporairement non disponible » sous Linux).
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err(tr!("le serveur n'a pas répondu à temps").into());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Réponse brute → réponse vérifiée complète (relecture A7).
fn complete_response(raw: &[u8], clean_end: bool) -> Result<Response, String> {
    let response = parse_response(raw)?;
    if !clean_end && !self_delimited(&response) {
        return Err(tr!("réponse HTTP tronquée").into());
    }
    Ok(response)
}

/// Contrôle d'une adresse d'image venue d'un mail, sans la télécharger :
/// adresse bien formée et aucune adresse locale ou privée.
pub(crate) fn check_image_url(url: &str) -> Result<(), String> {
    let (_, host, port, _) = split_url(url)?;
    resolve_addrs(&host, port, Reach::PublicOnly).map(|_| ())
}

fn request_once(url: &str, max_bytes: usize) -> Result<Response, String> {
    send_once("GET", url, &[("Accept", "image/*,*/*;q=0.5".to_string())], None, max_bytes, Reach::PublicOnly)
}

/// Connexion vers un serveur, en clair ou en TLS.
enum Conn {
    Plain(TcpStream),
    Tls(Box<native_tls::TlsStream<TcpStream>>),
}

impl Conn {
    fn tcp(&self) -> &TcpStream {
        match self {
            Self::Plain(tcp) => tcp,
            Self::Tls(tls) => tls.get_ref(),
        }
    }

    /// Vrai si le serveur n'a ni fermé la connexion ni rien envoyé depuis la
    /// dernière réponse : elle peut porter une nouvelle requête.
    fn still_open(&self) -> bool {
        let tcp = self.tcp();
        if tcp.set_nonblocking(true).is_err() {
            return false;
        }
        let mut probe = [0u8; 1];
        let idle = matches!(tcp.peek(&mut probe), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock);
        tcp.set_nonblocking(false).is_ok() && idle
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.read(buf),
            Self::Tls(tls) => tls.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.write(buf),
            Self::Tls(tls) => tls.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(tcp) => tcp.flush(),
            Self::Tls(tls) => tls.flush(),
        }
    }
}

/// Connexions gardées ouvertes entre deux requêtes (audit P7 : une
/// négociation TLS par requête `WebDAV` ou Outlook.com). Seulement vers les
/// serveurs de l'utilisateur : une image venue d'un mail ouvre toujours sa
/// propre connexion, fermée après la réponse.
mod pool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use super::Conn;

    /// Schéma (`https`), hôte en minuscules et port.
    pub(super) type Origin = (bool, String, u16);

    /// Au-delà, une connexion inutilisée est fermée : la plupart des serveurs
    /// ferment les leurs après 5 à 60 secondes.
    const IDLE_MAX: Duration = Duration::from_secs(30);
    const PER_ORIGIN: usize = 2;
    const TOTAL: usize = 8;

    struct Idle {
        origin: Origin,
        conn: Conn,
        since: Instant,
    }

    static IDLE: Mutex<Vec<Idle>> = Mutex::new(Vec::new());

    /// Connexion ouverte vers `origin`, encore utilisable, s'il y en a une.
    pub(super) fn take(origin: &Origin) -> Option<Conn> {
        loop {
            let found = {
                // Verrou empoisonné : la réserve n'est qu'un raccourci, on s'en passe.
                let mut idle = IDLE.lock().ok()?;
                idle.retain(|i| i.since.elapsed() < IDLE_MAX);
                let at = idle.iter().rposition(|i| i.origin == *origin)?;
                idle.swap_remove(at).conn
            };
            // Vérifiée hors du verrou ; fermée par le serveur, on essaie la suivante.
            if found.still_open() {
                return Some(found);
            }
        }
    }

    /// Rend une connexion dont la réponse a été lue en entier.
    pub(super) fn give_back(origin: Origin, conn: Conn) {
        let Ok(mut idle) = IDLE.lock() else { return };
        idle.retain(|i| i.since.elapsed() < IDLE_MAX);
        if idle.iter().filter(|i| i.origin == origin).count() >= PER_ORIGIN {
            return;
        }
        if idle.len() >= TOTAL {
            idle.remove(0);
        }
        idle.push(Idle { origin, conn, since: Instant::now() });
    }
}

/// Corps d'une réponse HTTP/1.1 : comment savoir qu'il est fini.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyEnd {
    /// Pas de corps (204, 304, réponse à `HEAD`).
    Empty,
    /// `Content-Length`.
    Length(usize),
    /// `Transfer-Encoding: chunked` ; position du prochain bloc à lire.
    Chunked(usize),
    /// Ni l'un ni l'autre : fin de connexion.
    Close,
}

/// Délimitation d'une réponse, lue sur son en-tête.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Framing {
    /// Réponse provisoire (1xx, par exemple 103 Early Hints) : la vraie suit.
    interim: bool,
    body_start: usize,
    end: BodyEnd,
    /// HTTP/1.1 sans `Connection: close` : le serveur garde la connexion.
    keep_alive: bool,
}

impl Framing {
    /// Délimitation, une fois l'en-tête entier reçu.
    fn read(raw: &[u8], head_request: bool) -> Result<Option<Self>, String> {
        let Some(head_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else { return Ok(None) };
        let response = parse_head(&raw[..head_end])?;
        let interim = (100..=199).contains(&response.status);
        let body_start = head_end + 4;
        let chunked = response.header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
        let end = if head_request || interim || matches!(response.status, 204 | 304) {
            BodyEnd::Empty
        } else if chunked {
            BodyEnd::Chunked(body_start)
        } else if let Some(length) = content_length(&response.headers)? {
            BodyEnd::Length(length)
        } else {
            BodyEnd::Close
        };
        let http11 = raw.starts_with(b"HTTP/1.1 ");
        let close = response.header("connection").is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("close")));
        Ok(Some(Self { interim, body_start, end, keep_alive: http11 && !close && end != BodyEnd::Close }))
    }

    /// Fin de la réponse dans `raw`, si elle est arrivée.
    fn end_in(&mut self, raw: &[u8]) -> Result<Option<usize>, String> {
        match self.end {
            BodyEnd::Empty => Ok(Some(self.body_start)),
            BodyEnd::Length(length) => Ok(Some(self.body_start + length).filter(|end| raw.len() >= *end)),
            BodyEnd::Close => Ok(None),
            BodyEnd::Chunked(mut next) => loop {
                let Some(line) = raw.get(next..).and_then(|r| r.windows(2).position(|w| w == b"\r\n")) else {
                    self.end = BodyEnd::Chunked(next);
                    return Ok(None);
                };
                let size_text = String::from_utf8_lossy(&raw[next..next + line]);
                let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
                    .map_err(|_| tr!("taille de bloc HTTP invalide"))?;
                let data = next + line + 2;
                if size == 0 {
                    // En-têtes de fin éventuels, puis une ligne vide.
                    let rest = &raw[data..];
                    if rest.starts_with(b"\r\n") {
                        return Ok(Some(data + 2));
                    }
                    self.end = BodyEnd::Chunked(next);
                    return Ok(rest.windows(4).position(|w| w == b"\r\n\r\n").map(|p| data + p + 4));
                }
                let after = data.checked_add(size).and_then(|n| n.checked_add(2)).ok_or(tr!("taille de bloc HTTP invalide"))?;
                if raw.len() < after {
                    self.end = BodyEnd::Chunked(next);
                    return Ok(None);
                }
                if &raw[after - 2..after] != b"\r\n" {
                    return Err(tr!("bloc HTTP mal terminé").into());
                }
                next = after;
            },
        }
    }
}

/// Code et en-têtes seuls (sans la ligne vide finale), avant l'arrivée du corps.
fn parse_head(head: &[u8]) -> Result<Response, String> {
    let text = String::from_utf8_lossy(head);
    let status: u16 =
        text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok()).ok_or(tr!("code HTTP illisible"))?;
    let headers =
        text.lines().skip(1).filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    Ok(Response { status, headers, body: Vec::new() })
}

/// Échec d'un échange sur une connexion.
#[derive(Debug, PartialEq, Eq)]
enum ExchangeError {
    /// La connexion était fermée : rien n'a pu être écrit, ou rien n'est
    /// revenu. Sur une connexion gardée, on en ouvre une neuve.
    Closed(String),
    /// Toute autre erreur : délai dépassé, réponse invalide ou trop longue.
    Failed(String),
}

impl ExchangeError {
    /// Message pour l'utilisateur, avec le serveur quand la connexion a été coupée.
    fn into_message(self, host: &str) -> String {
        match self {
            Self::Closed(cause) => format!("{host} : {cause}"),
            Self::Failed(message) => message,
        }
    }
}

impl From<String> for ExchangeError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl From<&str> for ExchangeError {
    fn from(message: &str) -> Self {
        Self::Failed(message.to_string())
    }
}

/// Réponse lue jusqu'à sa fin annoncée, sans attendre la fermeture de la
/// connexion. Rend les octets, si la connexion s'est terminée proprement
/// (même sens que [`read_capped`]) et si elle peut servir de nouveau.
fn read_response<S: Read>(
    stream: &mut S,
    max_bytes: usize,
    total: Duration,
    head_request: bool,
) -> Result<(Vec<u8>, bool, bool), ExchangeError> {
    let started = std::time::Instant::now();
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    let mut framing: Option<Framing> = None;
    loop {
        if framing.is_none() {
            framing = Framing::read(&raw, head_request)?;
        }
        if let Some(f) = framing.as_mut() {
            if let Some(end) = f.end_in(&raw)? {
                if f.interim {
                    // Réponse provisoire : écartée, la vraie réponse suit.
                    raw.drain(..end);
                    framing = None;
                    continue;
                }
                // Octets au-delà de la réponse : le serveur ne suit pas le
                // protocole, la connexion n'est pas réutilisée.
                let reusable = f.keep_alive && raw.len() == end;
                raw.truncate(end);
                return Ok((raw, true, reusable));
            }
        }
        if started.elapsed() > total {
            return Err(tr!("le serveur n'a pas répondu à temps").into());
        }
        match stream.read(&mut buf) {
            Ok(0) if raw.is_empty() => return Err(ExchangeError::Closed(tr!("connexion fermée sans réponse").into())),
            Ok(0) => return Ok((raw, true, false)),
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > max_bytes + 16 * 1024 {
                    return Err(tr!("réponse trop volumineuse").into());
                }
            }
            Err(e) if raw.is_empty() && matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err(tr!("le serveur n'a pas répondu à temps").into());
            }
            // Rien d'arrivé : la connexion était coupée ; la cause est gardée.
            Err(e) if raw.is_empty() => return Err(ExchangeError::Closed(e.to_string())),
            // Coupure en cours de réponse : ce qui est arrivé ne compte que
            // si la réponse prouve qu'elle est complète.
            Err(_) => return Ok((raw, false, false)),
        }
    }
}

/// Une requête sur une connexion : la réponse, et si la connexion peut
/// servir de nouveau.
fn exchange(
    conn: &mut Conn,
    request: &[u8],
    max_bytes: usize,
    total: Duration,
    head_request: bool,
) -> Result<(Response, bool), ExchangeError> {
    conn.write_all(request).map_err(|e| ExchangeError::Closed(e.to_string()))?;
    let (raw, clean_end, reusable) = read_response(conn, max_bytes, total, head_request)?;
    Ok((complete_response(&raw, clean_end)?, reusable))
}

/// Méthodes qu'on peut renvoyer sans risque si une connexion gardée s'est
/// fermée sous nos pieds (RFC 9110 §9.2.2).
fn idempotent(method: &str) -> bool {
    matches!(method, "GET" | "HEAD" | "PUT" | "DELETE" | "OPTIONS" | "PROPFIND" | "REPORT")
}

/// Une requête HTTP(S) quelconque (`WebDAV` : PROPFIND, REPORT, PUT, DELETE…).
fn send_once(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: Option<&[u8]>,
    max_bytes: usize,
    reach: Reach,
) -> Result<Response, String> {
    let (https, host, port, path) = split_url(url)?;
    let timeout = if reach == Reach::PublicOnly { TIMEOUT } else { SERVER_TIMEOUT };
    // Toute la réponse, quelle que soit la cadence d'envoi : 40 s pour une image,
    // 10 min pour un serveur de l'utilisateur (un carnet de 32 Mo sur une ligne lente).
    let total = if reach == Reach::PublicOnly { timeout.saturating_mul(4) } else { Duration::from_mins(10) };
    let keep = reach == Reach::Any;
    let default_port = if https { 443 } else { 80 };
    let host_header = if port == default_port { host.clone() } else { format!("{host}:{port}") };
    let connection = if keep { "keep-alive" } else { "close" };
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: Mozilla/5.0 (Vortex Mail)\r\nConnection: {connection}\r\n");
    for (name, value) in headers {
        let _ = write!(request, "{name}: {value}\r\n");
    }
    if body.is_some() || !matches!(method, "GET" | "DELETE") {
        let _ = write!(request, "Content-Length: {}\r\n", body.map_or(0, <[u8]>::len));
    }
    request.push_str("\r\n");
    let mut request = request.into_bytes();
    if let Some(body) = body {
        request.extend_from_slice(body);
    }
    let head_request = method == "HEAD";
    let origin: pool::Origin = (https, host.to_ascii_lowercase(), port);

    // Connexion gardée : seulement pour une requête qu'on peut renvoyer si
    // le serveur l'avait fermée entre-temps.
    if keep && idempotent(method) {
        if let Some(mut conn) = pool::take(&origin) {
            match exchange(&mut conn, &request, max_bytes, total, head_request) {
                Ok((response, reusable)) => {
                    if reusable {
                        pool::give_back(origin, conn);
                    }
                    return Ok(response);
                }
                // Fermée par le serveur entre deux requêtes : une connexion neuve suit.
                Err(ExchangeError::Closed(_)) => {}
                Err(ExchangeError::Failed(message)) => return Err(message),
            }
        }
    }

    let addrs = resolve_addrs(&host, port, reach)?;
    let tcp = connect_any(&host, &addrs, timeout)?;
    tcp.set_read_timeout(Some(timeout)).ok();
    tcp.set_write_timeout(Some(timeout)).ok();
    let mut conn = if https {
        let connector = crate::transport::tls_connector()?;
        Conn::Tls(Box::new(connector.connect(&host, tcp).map_err(|e| tr!("TLS avec {host} : {e}", host = host, e = e))?))
    } else {
        Conn::Plain(tcp)
    };
    // Connexion neuve : une coupure est une erreur, avec sa cause.
    let (response, reusable) = exchange(&mut conn, &request, max_bytes, total, head_request).map_err(|e| e.into_message(&host))?;
    if keep && reusable {
        pool::give_back(origin, conn);
    }
    Ok(response)
}

/// Schéma, hôte (en minuscules) et port d'une adresse.
pub(crate) fn origin(url: &str) -> Result<(bool, String, u16), String> {
    let (https, host, port, _) = split_url(url)?;
    Ok((https, host.to_ascii_lowercase(), port))
}

/// En-têtes à envoyer après une redirection de `from` vers `to` : refus de
/// https → http, identifiants retirés si l'origine change (relecture A1).
fn redirected_headers<'a>(
    start: &(bool, String, u16),
    from: &str,
    to: &str,
    headers: &[(&'a str, String)],
) -> Result<Vec<(&'a str, String)>, String> {
    let next = origin(to)?;
    if origin(from)?.0 && !next.0 {
        return Err(tr!("redirection de https vers http refusée : {url}", url = to));
    }
    let mut kept = headers.to_vec();
    if next != *start {
        kept.retain(|(name, _)| !CREDENTIAL_HEADERS.iter().any(|c| name.eq_ignore_ascii_case(c)));
    }
    Ok(kept)
}

/// Résout une redirection relative par rapport à l'adresse d'origine.
pub(crate) fn resolve(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    let Ok((https, host, port, path)) = split_url(base) else { return location.to_string() };
    let scheme = if https { "https" } else { "http" };
    let default_port = if https { 443 } else { 80 };
    let origin = if port == default_port { format!("{scheme}://{host}") } else { format!("{scheme}://{host}:{port}") };
    if let Some(rest) = location.strip_prefix("//") {
        format!("{scheme}://{rest}")
    } else if location.starts_with('/') {
        format!("{origin}{location}")
    } else {
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
        format!("{origin}{dir}/{location}")
    }
}

/// Requête quelconque, en suivant les redirections (même méthode, sauf 303) ;
/// corps de réponse limité à `max_bytes`.
pub(crate) fn send(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: Option<&[u8]>,
    max_bytes: usize,
) -> Result<(Response, String), String> {
    let start = origin(url)?;
    let mut current = url.to_string();
    let mut method = method.to_string();
    let mut body = body;
    let mut forwarded = headers.to_vec();
    for _ in 0..=MAX_REDIRECTS {
        let response = send_once(&method, &current, &forwarded, body, max_bytes, Reach::Any)?;
        if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
            if let Some(location) = response.header("location") {
                let next = resolve(&current, location);
                // Une fois retirés, les identifiants ne reviennent plus.
                forwarded = redirected_headers(&start, &current, &next, &forwarded)?;
                current = next;
                if response.status == 303 {
                    method = "GET".into();
                    body = None;
                }
                continue;
            }
        }
        if response.body.len() > max_bytes {
            return Err(tr!("réponse trop volumineuse").into());
        }
        return Ok((response, current));
    }
    Err(tr!("trop de redirections").into())
}

/// `GET` en suivant les redirections ; corps limité à `max_bytes`.
pub(crate) fn get(url: &str, max_bytes: usize) -> Result<Response, String> {
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let response = request_once(&current, max_bytes)?;
        if (300..400).contains(&response.status) {
            if let Some(location) = response.header("location") {
                let next = resolve(&current, location);
                // Une image demandée en https ne repart pas en clair.
                redirected_headers(&origin(url)?, &current, &next, &[])?;
                current = next;
                continue;
            }
        }
        if response.body.len() > max_bytes {
            return Err(tr!("réponse trop volumineuse").into());
        }
        return Ok(response);
    }
    Err(tr!("trop de redirections").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serveur qui envoie un octet toutes les `pause`.
    struct Trickle(std::time::Duration);
    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(self.0);
            buf[0] = b'x';
            Ok(1)
        }
    }

    /// Lecteur qui rend les octets par morceaux de `step`.
    struct Pieces {
        data: Vec<u8>,
        at: usize,
        step: usize,
    }
    impl Read for Pieces {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.step.min(buf.len()).min(self.data.len() - self.at);
            buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }

    fn read_in_pieces(raw: &[u8], step: usize) -> (Vec<u8>, bool) {
        let mut reader = Pieces { data: raw.to_vec(), at: 0, step };
        let (bytes, _, reusable) = read_response(&mut reader, 1 << 20, Duration::from_secs(5), false).expect("réponse lisible");
        (bytes, reusable)
    }

    /// Lecteur dont la connexion a été coupée par le serveur.
    struct Reset;
    impl Read for Reset {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connexion réinitialisée par le serveur"))
        }
    }

    #[test]
    fn j12_coupure_avant_la_reponse_garde_sa_cause() {
        let closed = read_response(&mut Reset, 1024, Duration::from_secs(5), false).expect_err("rien reçu");
        assert_eq!(closed, ExchangeError::Closed("connexion réinitialisée par le serveur".into()), "cause gardée");
        assert_eq!(closed.into_message("dav.exemple.fr"), "dav.exemple.fr : connexion réinitialisée par le serveur");
        let mut empty = Pieces { data: Vec::new(), at: 0, step: 1 };
        let quiet = read_response(&mut empty, 1024, Duration::from_secs(5), false).expect_err("fermée sans un octet");
        assert!(matches!(quiet, ExchangeError::Closed(_)), "fermeture sans réponse : {quiet:?}");
        // Délai dépassé : ce n'est pas une connexion fermée, on ne réessaie pas.
        assert!(matches!(
            read_response(&mut Trickle(Duration::from_millis(5)), 1 << 20, Duration::from_millis(30), false),
            Err(ExchangeError::Failed(_))
        ));
    }

    #[test]
    fn p7_reponse_lue_jusqu_a_sa_fin_annoncee() {
        for step in [1, 3, 1024] {
            let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
            assert_eq!(read_in_pieces(raw, step), (raw.to_vec(), true), "longueur annoncée, morceaux de {step}");
            let chunked =
                b"HTTP/1.1 207 Multi-Status\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x=1\r\nde\r\n0\r\nX-Fin: 1\r\n\r\n";
            let (bytes, reusable) = read_in_pieces(chunked, step);
            assert_eq!((bytes.len(), reusable), (chunked.len(), true), "chunked et en-têtes de fin, morceaux de {step}");
            assert_eq!(parse_response(&bytes).expect("réponse complète").body, b"abcde");
            let empty = b"HTTP/1.1 204 No Content\r\n\r\n";
            assert_eq!(read_in_pieces(empty, step), (empty.to_vec(), true), "sans corps, morceaux de {step}");
        }
        let early = b"HTTP/1.1 103 Early Hints\r\nLink: </a.css>\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
        let (bytes, reusable) = read_in_pieces(early, 4);
        assert_eq!((parse_response(&bytes).expect("réponse finale").status, reusable), (200, true), "réponse provisoire écartée");
    }

    #[test]
    fn p7_connexion_gardee_seulement_si_le_serveur_la_garde() {
        let cases: [(&[u8], &str); 4] = [
            (b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok", "Connection: close"),
            (b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok", "HTTP/1.0"),
            (b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nokXX", "octets en trop"),
            (b"HTTP/1.1 200 OK\r\n\r\nfin par fermeture", "sans longueur annoncée"),
        ];
        for (raw, why) in cases {
            assert!(!read_in_pieces(raw, 1024).1, "{why}");
        }
    }

    /// Serveur local qui répond « ok » à chaque requête et ferme chaque
    /// connexion après `per_connection` réponses ; rend son adresse et le
    /// nombre de connexions acceptées.
    fn keep_alive_server(per_connection: usize) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("port local");
        let url = format!("http://127.0.0.1:{}/dav/", listener.local_addr().expect("adresse locale").port());
        let accepted = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&accepted);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                counter.fetch_add(1, Ordering::SeqCst);
                for _ in 0..per_connection {
                    let mut request = Vec::new();
                    let mut byte = [0u8; 1];
                    while !request.ends_with(b"\r\n\r\n") {
                        if !matches!(stream.read(&mut byte), Ok(1)) {
                            break;
                        }
                        request.push(byte[0]);
                    }
                    if !request.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
                }
            }
        });
        (url, accepted)
    }

    #[test]
    fn p7_connexion_reutilisee_vers_un_serveur_de_l_utilisateur() {
        use std::sync::atomic::Ordering;
        let (url, accepted) = keep_alive_server(2);
        for _ in 0..2 {
            assert_eq!(send("PROPFIND", &url, &[], None, 1024).expect("réponse").0.body, b"ok");
        }
        assert_eq!(accepted.load(Ordering::SeqCst), 1, "deux requêtes, une seule connexion");
        // Le serveur a fermé après sa deuxième réponse : la requête suivante en ouvre une neuve.
        assert_eq!(send("PROPFIND", &url, &[], None, 1024).expect("réponse après fermeture").0.body, b"ok");
        assert_eq!(accepted.load(Ordering::SeqCst), 2, "connexion fermée par le serveur remplacée");
    }

    #[test]
    fn s7_reponse_au_goutte_a_goutte_bornee() {
        let started = std::time::Instant::now();
        let result = read_capped(Trickle(std::time::Duration::from_millis(5)), 1024 * 1024, std::time::Duration::from_millis(50));
        assert!(result.is_err(), "abandonné au bout du délai total");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn s7_adresse_suivante_essayee() {
        // Une adresse qui refuse la connexion, puis une qui répond.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let open = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addrs = [closed, open.local_addr().unwrap()];
        assert!(connect_any("exemple", &addrs, std::time::Duration::from_secs(2)).is_ok(), "la seconde adresse répond");
        assert!(connect_any("exemple", &addrs[..1], std::time::Duration::from_secs(2)).is_err());
    }

    #[test]
    fn urls() {
        assert_eq!(split_url("https://img.exemple.fr/a/b.png?x=1").unwrap(), (true, "img.exemple.fr".into(), 443, "/a/b.png?x=1".into()));
        assert_eq!(split_url("http://h:8080").unwrap(), (false, "h".into(), 8080, "/".into()));
        assert_eq!(split_url("//cdn.x/i.gif").unwrap(), (true, "cdn.x".into(), 443, "/i.gif".into()));
        assert!(split_url("javascript:alert(1)").is_err());
        assert_eq!(resolve("https://a.fr/x/y.png", "/z.png"), "https://a.fr/z.png");
        assert_eq!(resolve("https://a.fr/x/y.png", "z.png"), "https://a.fr/x/z.png");
        assert_eq!(resolve("http://a.fr:81/y", "https://b.fr/c"), "https://b.fr/c");
    }

    #[test]
    fn responses() {
        let r = parse_response(b"HTTP/1.1 302 Found\r\nLocation: /n.png\r\nContent-Length: 0\r\n\r\n").unwrap();
        assert_eq!((r.status, r.header("location")), (302, Some("/n.png")));
    }

    #[test]
    fn adresses_avec_caracteres_de_controle_refusees() {
        assert!(split_url("http://127.0.0.1:11434/x HTTP/1.1\r\nHost: a\r\n\r\nPOST /api HTTP/1.1").is_err());
        assert!(split_url("http://h/a\tb").is_err());
        assert!(split_url("http://h\x7f/").is_err());
        assert_eq!(split_url("http://h/a b.png").unwrap().3, "/a%20b.png");
        assert!(split_url("http://a b/").is_err());
    }

    #[test]
    fn reponses_tronquees() {
        // Content-Length non atteint : erreur, même sur une fin propre.
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc").is_err());
        let r = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabcdef").unwrap();
        assert_eq!(r.body, b"abc");
        // chunked sans bloc final : erreur.
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n").is_err());
        let r = parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n").unwrap();
        assert_eq!(r.body, b"abc");
        // Sans longueur annoncée : seule une fin de connexion propre vaut fin de réponse.
        let raw = b"HTTP/1.1 207 Multi-Status\r\n\r\n<multistatus>";
        assert!(complete_response(raw, false).is_err());
        assert!(complete_response(raw, true).is_ok());
        // Coupure TLS après une réponse complète et délimitée : acceptée.
        assert!(complete_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok", false).is_ok());
    }

    #[test]
    fn redirections() {
        let start = origin("https://dav.exemple.fr/cal/").unwrap();
        let headers = vec![("Authorization", "Basic secret".to_string()), ("Depth", "1".to_string())];
        // Même origine : tout est gardé.
        let kept = redirected_headers(&start, "https://dav.exemple.fr/cal/", "https://DAV.exemple.fr/cal2/", &headers).unwrap();
        assert_eq!(kept.len(), 2);
        // Autre hôte, autre port : identifiants retirés, le reste gardé.
        for to in ["https://cdn.exemple.net/cal/", "https://dav.exemple.fr:8443/cal/"] {
            let kept = redirected_headers(&start, "https://dav.exemple.fr/cal/", to, &headers).unwrap();
            assert_eq!(kept, vec![("Depth", "1".to_string())], "{to}");
        }
        // https → http : refusé.
        assert!(redirected_headers(&start, "https://dav.exemple.fr/cal/", "http://dav.exemple.fr/cal/", &headers).is_err());
        // http → https sur le même hôte : origine différente (schéma), identifiants retirés.
        let plain = origin("http://dav.exemple.fr/").unwrap();
        let kept = redirected_headers(&plain, "http://dav.exemple.fr/", "https://dav.exemple.fr/", &headers).unwrap();
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn adresses_locales_et_privees() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.1.1",
            "64:ff9b::7f00:1",
            "2002:c0a8:101::1",
            "fec0::1",
            "64:ff9b:1::a00:1",
            "2001:0:4136:e378::1",
            "100::1",
            "3fff::1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111", "::ffff:8.8.8.8"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
        assert!(resolve_addrs("127.0.0.1", 80, Reach::PublicOnly).is_err());
        assert!(resolve_addrs("[::1]", 80, Reach::PublicOnly).is_err());
        assert!(resolve_addrs("192.168.1.10", 80, Reach::Any).is_ok());
    }
}
