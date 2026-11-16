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
fn resolve_addr(host: &str, port: u16, reach: Reach) -> Result<SocketAddr, String> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<SocketAddr> =
        (bare, port).to_socket_addrs().map_err(|e| tr!("serveur « {host} » introuvable : {e}", host = host, e = e))?.collect();
    let first = *addrs.first().ok_or_else(|| tr!("serveur « {host} » introuvable", host = host))?;
    if reach == Reach::PublicOnly && addrs.iter().any(|a| !is_public(a.ip())) {
        return Err(tr!("adresse locale ou privée refusée : {host}", host = host));
    }
    Ok(first)
}

/// Lit la réponse ; le booléen dit si la connexion s'est terminée proprement.
pub(crate) fn read_capped<S: Read>(mut stream: S, max_bytes: usize) -> Result<(Vec<u8>, bool), String> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
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
    resolve_addr(&host, port, Reach::PublicOnly).map(|_| ())
}

fn request_once(url: &str, max_bytes: usize) -> Result<Response, String> {
    send_once("GET", url, &[("Accept", "image/*,*/*;q=0.5".to_string())], None, max_bytes, Reach::PublicOnly)
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
    let addr = resolve_addr(&host, port, reach)?;
    let timeout = if reach == Reach::PublicOnly { TIMEOUT } else { SERVER_TIMEOUT };
    let tcp = TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("{host} : {e}"))?;
    tcp.set_read_timeout(Some(timeout)).ok();
    tcp.set_write_timeout(Some(timeout)).ok();
    let default_port = if https { 443 } else { 80 };
    let host_header = if port == default_port { host.clone() } else { format!("{host}:{port}") };
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: Mozilla/5.0 (Vortex Mail)\r\nConnection: close\r\n");
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
    let (raw, clean_end) = if https {
        let connector = crate::transport::tls_connector()?;
        let mut tls = connector.connect(&host, tcp).map_err(|e| tr!("TLS avec {host} : {e}", host = host, e = e))?;
        tls.write_all(&request).map_err(|e| e.to_string())?;
        read_capped(tls, max_bytes)?
    } else {
        let mut tcp = tcp;
        tcp.write_all(&request).map_err(|e| e.to_string())?;
        read_capped(tcp, max_bytes)?
    };
    complete_response(&raw, clean_end)
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
        assert!(resolve_addr("127.0.0.1", 80, Reach::PublicOnly).is_err());
        assert!(resolve_addr("[::1]", 80, Reach::PublicOnly).is_err());
        assert!(resolve_addr("192.168.1.10", 80, Reach::Any).is_ok());
    }
}
