//! Connexions réseau : TCP, SSL/TLS, STARTTLS, authentification IMAP et SMTP.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use imap_lite::Client;

use crate::config::{AuthMethod, Security, ServerConfig};

/// Flux réseau quelconque (clair ou chiffré).
pub(crate) trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

pub(crate) type Session = Client<Box<dyn Stream>>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const IO_TIMEOUT: Duration = Duration::from_secs(60);
/// Attente d'une réponse SMTP : la RFC 5321 (§4.5.3.2) demande de patienter
/// jusqu'à 10 minutes après la fin du message, le temps que le serveur
/// l'analyse ; abandonner avant ferait croire à un échec un envoi accepté
/// (relecture complète n° 9).
const SMTP_REPLY_TIMEOUT: Duration = Duration::from_mins(10);

fn tcp(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs = (host, port).to_socket_addrs().map_err(|e| tr!("serveur « {host} » introuvable : {e}", host = host, e = e))?;
    let mut last_error = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => {
                stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
                stream.set_write_timeout(Some(IO_TIMEOUT)).ok();
                return Ok(stream);
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(match last_error {
        Some(e) => tr!("connexion à {host}:{port} impossible : {e}", host = host, port = port, e = e),
        None => tr!("aucune adresse pour {host}", host = host),
    })
}

/// Connecteur TLS de toute l'application (IMAP, SMTP, HTTP, jetons) : TLS 1.2
/// au moins, comme le demandent les RFC 8996 et 8314 (relecture complète
/// n° 34). La vérification du certificat et du nom d'hôte reste celle par
/// défaut de la bibliothèque du système.
///
/// # Errors
///
/// Bibliothèque TLS du système indisponible.
pub(crate) fn tls_connector() -> Result<native_tls::TlsConnector, String> {
    native_tls::TlsConnector::builder()
        .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
        .build()
        .map_err(|e| tr!("TLS indisponible : {e}", e = e))
}

/// Connexion SMTP : comme `tcp`, avec l'attente de réponse de la RFC 5321.
fn smtp_tcp(host: &str, port: u16) -> Result<TcpStream, String> {
    let stream = tcp(host, port)?;
    stream.set_read_timeout(Some(SMTP_REPLY_TIMEOUT)).ok();
    Ok(stream)
}

/// Chiffrement d'une connexion. `pin` vide : certificat et nom vérifiés par la
/// bibliothèque du système. Sinon (Proton Mail Bridge, lot I3), voir [`tls_pinned`].
fn tls<S: Read + Write + std::fmt::Debug + 'static>(host: &str, pin: &str, stream: S) -> Result<native_tls::TlsStream<S>, String> {
    if !pin.is_empty() {
        return tls_pinned(host, pin, stream);
    }
    let connector = tls_connector()?;
    connector.connect(host, stream).map_err(|e| handshake_failed(host, &e))
}

fn handshake_failed<S: std::fmt::Debug + 'static>(host: &str, e: &native_tls::HandshakeError<S>) -> String {
    tr!("négociation SSL/TLS avec {host} échouée : {e}", host = host, e = e)
}

// ---------------------------------------------------------------- Proton Mail Bridge (lot I3)

/// Hôte de Proton Mail Bridge : la boucle locale de cet ordinateur.
const BRIDGE_HOST: &str = "127.0.0.1";

/// Adresse IP de la boucle locale (`127.0.0.1`, `::1`…) : seul cas où un certificat
/// épinglé remplace la vérification habituelle. Un nom (`localhost`) n'est pas
/// admis : il passe par le résolveur du système.
pub(crate) fn is_loopback(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Empreinte SHA-256 d'un certificat (DER), en majuscules par paires séparées de
/// « : », comme l'affichent les navigateurs et `openssl x509 -fingerprint -sha256`.
pub(crate) fn fingerprint(der: &[u8]) -> String {
    let hex: Vec<String> = crate::crypto::sha256(der).iter().map(|b| format!("{b:02X}")).collect();
    hex.join(":")
}

/// Empreinte bien formée : 32 paires hexadécimales séparées de « : ».
pub(crate) fn is_fingerprint(s: &str) -> bool {
    let pairs: Vec<&str> = s.trim().split(':').collect();
    pairs.len() == 32 && pairs.iter().all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Connecteur sans vérification du certificat ni du nom, réservé à [`tls_pinned`]
/// et à [`bridge_fingerprint`] : l'empreinte du certificat est contrôlée juste
/// après la négociation, avant tout identifiant. La négociation prouve que le
/// serveur détient la clé privée du certificat présenté.
fn unverified_connector() -> Result<native_tls::TlsConnector, String> {
    native_tls::TlsConnector::builder()
        .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|e| tr!("TLS indisponible : {e}", e = e))
}

/// Empreinte du certificat présenté par le serveur d'une connexion chiffrée.
fn presented<S: Read + Write>(stream: &native_tls::TlsStream<S>) -> Result<String, String> {
    match stream.peer_certificate() {
        Ok(Some(cert)) => cert.to_der().map(|der| fingerprint(&der)).map_err(|e| e.to_string()),
        Ok(None) => Err(tr!("Proton Mail Bridge n'a présenté aucun certificat.").into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Chiffrement avec un certificat épinglé : Proton Mail Bridge présente un
/// certificat auto-signé, que la bibliothèque du système refuserait. Admis sur la
/// boucle locale seulement ; la connexion est refusée si l'empreinte du certificat
/// diffère de `pin`, confirmée par l'utilisateur à l'ajout du compte.
fn tls_pinned<S: Read + Write + std::fmt::Debug + 'static>(host: &str, pin: &str, stream: S) -> Result<native_tls::TlsStream<S>, String> {
    if !is_loopback(host) {
        return Err(tr!(
            "Certificat épinglé refusé pour {host} : il n'est admis que pour Proton Mail Bridge, sur cet ordinateur (127.0.0.1).",
            host = host
        ));
    }
    let stream = unverified_connector()?.connect(host, stream).map_err(|e| handshake_failed(host, &e))?;
    let found = presented(&stream)?;
    if !found.eq_ignore_ascii_case(pin.trim()) {
        return Err(tr!(
            "Le certificat de Proton Mail Bridge a changé (empreinte {found}, {pin} attendue). Si vous avez réinstallé ou réinitialisé Bridge, \
             modifiez le compte et acceptez le nouveau certificat ; sinon, ne vous connectez pas.",
            found = found,
            pin = pin.trim()
        ));
    }
    Ok(stream)
}

/// Empreinte du certificat de Proton Mail Bridge, lue sur son serveur IMAP
/// (`127.0.0.1:port`) pour être montrée à l'utilisateur avant d'être épinglée.
/// Aucun identifiant n'est envoyé.
///
/// # Errors
///
/// Bridge absent ou arrêté, chiffrement demandé non proposé, négociation échouée.
pub(crate) fn bridge_fingerprint(port: u16, security: Security) -> Result<String, String> {
    let absent = || {
        tr!("Proton Mail Bridge ne répond pas sur cet ordinateur (port {port}). Lancez Bridge et connectez-y votre compte Proton, puis réessayez.", port = port)
    };
    let tcp = probe_tcp(BRIDGE_HOST, port).ok_or_else(absent)?;
    let connector = unverified_connector()?;
    match security {
        Security::SslTls => presented(&connector.connect(BRIDGE_HOST, tcp).map_err(|e| handshake_failed(BRIDGE_HOST, &e))?),
        Security::StartTls => {
            let mut plain = Client::connect(tcp).map_err(imap_err)?;
            if plain.capabilities().is_empty() {
                plain.refresh_capabilities().map_err(imap_err)?;
            }
            if !plain.has_capability("STARTTLS") {
                return Err(tr!("{host}:{port} ne propose pas STARTTLS. Essayez SSL/TLS (port 993).", host = BRIDGE_HOST, port = port));
            }
            let raw = plain.starttls().map_err(imap_err)?;
            presented(&connector.connect(BRIDGE_HOST, raw).map_err(|e| handshake_failed(BRIDGE_HOST, &e))?)
        }
        Security::Auto | Security::Unencrypted => Err(tr!("choisissez SSL/TLS ou STARTTLS pour un certificat épinglé").into()),
    }
}

/// Chiffrement réellement utilisé quand l'utilisateur a choisi « Auto ».
fn resolve(security: Security, port: u16, implicit_tls_port: u16) -> Security {
    match security {
        Security::Auto if port == implicit_tls_port => Security::SslTls,
        Security::Auto => Security::StartTls,
        other => other,
    }
}

pub(crate) fn security_label(s: Security) -> &'static str {
    match s {
        Security::Auto => "Auto",
        Security::SslTls => "SSL/TLS",
        Security::StartTls => "STARTTLS",
        Security::Unencrypted => tr!("aucun chiffrement"),
    }
}

#[expect(clippy::needless_pass_by_value, reason = "utilisée avec map_err, qui passe l'erreur par valeur")]
fn imap_err(e: imap_lite::Error) -> String {
    e.to_string()
}

/// Mot de passe, ou jeton d'accès pour un compte `OAuth2` (Gmail ou Outlook.com).
fn secret(cfg: &ServerConfig) -> Result<String, String> {
    if cfg.auth == AuthMethod::OAuth2 {
        return if crate::google::is_google_host(&cfg.host) {
            crate::google::access_token(cfg.username.trim())
        } else {
            crate::oauth::access_token(cfg.username.trim())
        };
    }
    if cfg.password.is_empty() {
        return Err(tr!("Mot de passe absent du trousseau du système. \
            Ouvrez Paramètres, modifiez le compte et saisissez-le à nouveau.")
        .into());
    }
    Ok(cfg.password.clone())
}

/// Ouvre une session IMAP authentifiée. Renvoie aussi le chiffrement utilisé.
pub(crate) fn open_imap(cfg: &ServerConfig) -> Result<(Session, Security), String> {
    let secret = secret(cfg)?;
    let host = cfg.host.trim();
    let security = resolve(cfg.security, cfg.port, 993);
    let mut session: Session = match security {
        Security::SslTls => {
            let stream: Box<dyn Stream> = Box::new(tls(host, &cfg.pinned_cert, tcp(host, cfg.port)?)?);
            Client::connect(stream).map_err(imap_err)?
        }
        Security::StartTls => {
            let mut plain = Client::connect(tcp(host, cfg.port)?).map_err(imap_err)?;
            if plain.capabilities().is_empty() {
                plain.refresh_capabilities().map_err(imap_err)?;
            }
            if !plain.has_capability("STARTTLS") {
                return Err(tr!("{host}:{port} ne propose pas STARTTLS. Essayez SSL/TLS (port 993).", host = host, port = cfg.port));
            }
            let raw = plain.starttls().map_err(imap_err)?;
            let stream: Box<dyn Stream> = Box::new(tls(host, &cfg.pinned_cert, raw)?);
            let mut client = Client::resume(stream);
            client.refresh_capabilities().map_err(imap_err)?;
            client
        }
        Security::Unencrypted | Security::Auto => {
            let stream: Box<dyn Stream> = Box::new(tcp(host, cfg.port)?);
            Client::connect(stream).map_err(imap_err)?
        }
    };

    if session.capabilities().is_empty() {
        session.refresh_capabilities().map_err(imap_err)?;
    }
    let user = cfg.username.trim();
    let result = match cfg.auth {
        // LOGINDISABLED : le serveur juge la connexion non sûre pour un mot de
        // passe ; AUTHENTICATE PLAIN l'enverrait tout autant en clair.
        AuthMethod::Normal | AuthMethod::Plain if session.has_capability("LOGINDISABLED") => {
            return Err(tr!("Le serveur refuse le mot de passe sur une connexion non chiffrée. \
                        Choisissez SSL/TLS ou STARTTLS.")
            .into());
        }
        AuthMethod::Normal => session.login(user, &secret),
        AuthMethod::Plain => session.authenticate_plain(user, &secret),
        AuthMethod::OAuth2 => session.authenticate_xoauth2(user, &secret),
    };
    // Jeton refusé (révoqué entre-temps) : oublié et renouvellement tenté, ce qui
    // propose « Reconnecter » si l'autorisation elle-même a été retirée.
    if matches!(result, Err(imap_lite::Error::No(_))) && cfg.auth == AuthMethod::OAuth2 {
        oauth_refused(cfg, user);
    }
    result.map_err(|e| match e {
        imap_lite::Error::No(t) if cfg.auth == AuthMethod::OAuth2 && crate::google::is_google_host(&cfg.host) => tr!(
            "Connexion Google refusée par le serveur IMAP ({t}). Causes probables : l'accès au \
             courrier n'a pas été autorisé (cliquez sur « Se reconnecter » et cochez « Gmail » sur \
             la page Google), ou l'API Gmail n'est pas ajoutée à votre application Google.",
            t = t
        ),
        imap_lite::Error::No(t) if cfg.auth == AuthMethod::OAuth2 => tr!(
            "Connexion Outlook.com refusée par le serveur IMAP ({t}). Causes probables : \
             la connexion a été faite avec un autre compte dans le navigateur \
             (cliquez sur « Se reconnecter » et choisissez le bon compte), ou l'accès IMAP \
             n'est pas activé dans Outlook.com (Paramètres → Courrier → Transfert et IMAP).",
            t = t
        ),
        imap_lite::Error::No(t) => tr!("Identifiant ou mot de passe refusé ({t})", t = t),
        other => other.to_string(),
    })?;
    Ok((session, security))
}

// ---------------------------------------------------------------- SMTP

/// Plafonds d'une réponse SMTP : une ligne fait au plus 512 octets selon la
/// RFC 5321 (marge large ici), et `EHLO` en annonce une vingtaine.
const SMTP_MAX_LINE: usize = 4096;
const SMTP_MAX_LINES: usize = 200;

pub(crate) fn smtp_reply(s: &mut dyn Stream) -> Result<(u16, Vec<String>), String> {
    let mut lines = Vec::new();
    loop {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let n = s.read(&mut byte).map_err(|e| tr!("SMTP : {e}", e = e))?;
            if n == 0 {
                return Err(tr!("SMTP : connexion fermée par le serveur").into());
            }
            if byte[0] == b'\n' {
                break;
            }
            line.push(byte[0]);
            if line.len() > SMTP_MAX_LINE {
                return Err(tr!("SMTP : réponse trop longue").into());
            }
        }
        let text = String::from_utf8_lossy(&line).trim_end_matches('\r').to_string();
        let code: u16 =
            text.get(..3).and_then(|c| c.parse().ok()).ok_or_else(|| tr!("SMTP : réponse illisible « {text} »", text = text))?;
        let last = text.as_bytes().get(3) != Some(&b'-');
        lines.push(text.get(4..).unwrap_or("").to_string());
        if last {
            return Ok((code, lines));
        }
        if lines.len() >= SMTP_MAX_LINES {
            return Err(tr!("SMTP : réponse trop longue").into());
        }
    }
}

fn smtp_cmd(s: &mut dyn Stream, cmd: &str, expect: u16) -> Result<Vec<String>, String> {
    smtp_cmd_codes(s, cmd, &[expect])
}

fn smtp_cmd_codes(s: &mut dyn Stream, cmd: &str, accepted: &[u16]) -> Result<Vec<String>, String> {
    let expect = accepted[0];
    s.write_all(format!("{cmd}\r\n").as_bytes()).map_err(|e| tr!("SMTP : {e}", e = e))?;
    s.flush().map_err(|e| tr!("SMTP : {e}", e = e))?;
    let (code, lines) = smtp_reply(s)?;
    if !accepted.contains(&code) {
        // Ne jamais afficher les identifiants envoyés.
        let reply = lines.join(" ");
        return Err(if cmd.starts_with("AUTH") || expect == 334 || expect == 235 {
            tr!("SMTP authentification : {code} {reply}", code = code, reply = reply)
        } else {
            tr!("SMTP {cmd} : {code} {reply}", cmd = cmd, code = code, reply = reply)
        });
    }
    Ok(lines)
}

fn smtp_expect_greeting(s: &mut dyn Stream) -> Result<(), String> {
    let (code, lines) = smtp_reply(s)?;
    if code != 220 {
        return Err(tr!("SMTP : accueil inattendu {code} {reply}", code = code, reply = lines.join(" ")));
    }
    Ok(())
}

fn smtp_auth(s: &mut dyn Stream, ehlo: &[String], user: &str, password: &str, method: AuthMethod) -> Result<(), String> {
    if method == AuthMethod::OAuth2 {
        let token = imap_lite::base64_encode(format!("user={user}\x01auth=Bearer {password}\x01\x01").as_bytes());
        s.write_all(format!("AUTH XOAUTH2 {token}\r\n").as_bytes()).map_err(|e| tr!("SMTP : {e}", e = e))?;
        s.flush().map_err(|e| tr!("SMTP : {e}", e = e))?;
        let (code, lines) = smtp_reply(s)?;
        return match code {
            235 => Ok(()),
            // Détail de l'erreur en base64 : on l'acquitte, le serveur conclut par 535.
            334 => {
                s.write_all(b"\r\n").map_err(|e| tr!("SMTP : {e}", e = e))?;
                let (code, lines) = smtp_reply(s)?;
                Err(tr!("SMTP : connexion OAuth2 refusée ({code} {reply})", code = code, reply = lines.join(" ")))
            }
            _ => Err(tr!("SMTP : connexion OAuth2 refusée ({code} {reply})", code = code, reply = lines.join(" "))),
        };
    }
    let auth_line = ehlo.iter().find(|l| l.to_ascii_uppercase().starts_with("AUTH")).map_or_default(|l| l.to_ascii_uppercase());
    if auth_line.contains("PLAIN") {
        let token = imap_lite::base64_encode(format!("\0{user}\0{password}").as_bytes());
        smtp_cmd(s, &format!("AUTH PLAIN {token}"), 235)?;
    } else if auth_line.contains("LOGIN") {
        smtp_cmd(s, "AUTH LOGIN", 334)?;
        smtp_cmd(s, &imap_lite::base64_encode(user.as_bytes()), 334)?;
        smtp_cmd(s, &imap_lite::base64_encode(password.as_bytes()), 235)?;
    } else {
        return Err(tr!("Le serveur SMTP ne propose ni AUTH PLAIN ni AUTH LOGIN").into());
    }
    Ok(())
}

/// Vérifie le serveur sortant : connexion, chiffrement et authentification.
pub(crate) fn check_smtp(cfg: &ServerConfig) -> Result<Security, String> {
    let mut c = smtp_open(cfg)?;
    let _ = smtp_cmd(c.stream.as_mut(), "QUIT", 221);
    Ok(c.security)
}

/// Le service a-t-il coupé l'envoi SMTP de cette boîte (Outlook.com, comptes
/// récents : « 535 5.7.139 … `SmtpClientAuthentication` is disabled ») ? Les
/// messages partent alors par l'API d'Outlook.com.
#[must_use]
pub(crate) fn smtp_disabled_by_service(cfg: &ServerConfig, error: &str) -> bool {
    cfg.auth == AuthMethod::OAuth2 && !crate::google::is_google_host(&cfg.host) && error.contains("5.7.139")
}

/// Serveur d'Outlook.com ou d'O365 : il range lui-même le message envoyé
/// dans « Éléments envoyés ».
#[must_use]
pub(crate) fn is_microsoft_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    ["outlook.com", "office365.com"].iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// Nom annoncé par `EHLO` : l'adresse locale de la connexion, en littéral
/// (`[192.0.2.1]`, `[IPv6:2001:db8::1]`), comme Thunderbird. `vortexmail`
/// n'était ni un nom de domaine complet ni un littéral, et certains
/// serveurs le refusent (RFC 5321 §4.1.1.1 ; relecture complète n° 8).
fn hello_name(stream: &TcpStream) -> String {
    match stream.local_addr().map(|a| a.ip()) {
        Ok(std::net::IpAddr::V4(ip)) => format!("[{ip}]"),
        Ok(std::net::IpAddr::V6(ip)) => match ip.to_ipv4_mapped() {
            Some(v4) => format!("[{v4}]"),
            None => format!("[IPv6:{ip}]"),
        },
        Err(_) => "[127.0.0.1]".to_string(),
    }
}

/// `EHLO`, puis `HELO` si le serveur ne connaît pas `EHLO` (RFC 5321 §3.2) ;
/// renvoie les extensions annoncées (aucune après `HELO`).
fn smtp_hello(s: &mut dyn Stream, name: &str) -> Result<Vec<String>, String> {
    s.write_all(format!("EHLO {name}\r\n").as_bytes()).map_err(|e| tr!("SMTP : {e}", e = e))?;
    s.flush().map_err(|e| tr!("SMTP : {e}", e = e))?;
    let (code, lines) = smtp_reply(s)?;
    match code {
        250 => Ok(lines),
        500 | 502 => {
            smtp_cmd(s, &format!("HELO {name}"), 250)?;
            Ok(Vec::new())
        }
        _ => Err(tr!("SMTP EHLO : {code} {reply}", code = code, reply = lines.join(" "))),
    }
}

// ---------------------------------------------------------------- essais sans mot de passe

/// Attente d'un serveur essayé pendant la recherche des réglages d'un compte (lot I1).
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Connexion TCP courte, pour un essai : chaque adresse du nom à son tour (une
/// adresse IPv6 injoignable ne cache pas l'IPv4), trois au plus.
fn probe_tcp(host: &str, port: u16) -> Option<TcpStream> {
    let stream = (host, port).to_socket_addrs().ok()?.take(3).find_map(|addr| TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).ok())?;
    stream.set_read_timeout(Some(PROBE_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(PROBE_TIMEOUT)).ok()?;
    Some(stream)
}

/// Le serveur IMAP `host:port` répond avec le chiffrement `security`, certificat
/// valable pour ce nom ; aucun identifiant n'est envoyé (lot I1).
pub(crate) fn probe_imap(host: &str, port: u16, security: Security) -> bool {
    let Some(tcp) = probe_tcp(host, port) else { return false };
    match security {
        Security::SslTls => tls(host, "", tcp).is_ok_and(|stream| Client::connect(stream).is_ok()),
        Security::StartTls => {
            let Ok(mut plain) = Client::connect(tcp) else { return false };
            if plain.capabilities().is_empty() && plain.refresh_capabilities().is_err() {
                return false;
            }
            plain.has_capability("STARTTLS") && plain.starttls().is_ok_and(|raw| tls(host, "", raw).is_ok())
        }
        Security::Auto | Security::Unencrypted => false,
    }
}

/// Le serveur SMTP `host:port` accueille avec le chiffrement `security`, certificat
/// valable pour ce nom ; aucun identifiant n'est envoyé (lot I1).
pub(crate) fn probe_smtp(host: &str, port: u16, security: Security) -> bool {
    let Some(tcp) = probe_tcp(host, port) else { return false };
    let name = hello_name(&tcp);
    match security {
        Security::SslTls => tls(host, "", tcp).is_ok_and(|mut stream| smtp_expect_greeting(&mut stream).is_ok()),
        Security::StartTls => {
            let mut plain = tcp;
            smtp_expect_greeting(&mut plain).is_ok()
                && smtp_hello(&mut plain, &name).is_ok_and(|ehlo| ehlo.iter().any(|l| l.eq_ignore_ascii_case("STARTTLS")))
                && smtp_cmd(&mut plain, "STARTTLS", 220).is_ok()
                && tls(host, "", plain).is_ok()
        }
        Security::Auto | Security::Unencrypted => false,
    }
}

/// Envoie un message déjà préparé par `mail_core::compose::smtp_data`.
pub(crate) fn send_smtp(cfg: &ServerConfig, from: &str, recipients: &[String], data: &[u8]) -> Result<Security, String> {
    let mut c = smtp_open(cfg)?;
    smtp_transaction(c.stream.as_mut(), &c.extensions, from, recipients, data)?;
    Ok(c.security)
}

/// Extension annoncée par `EHLO` (premier mot de la ligne, ex. `SMTPUTF8`).
fn has_extension(ehlo: &[String], name: &str) -> bool {
    ehlo.iter().any(|l| l.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case(name)))
}

/// `MAIL FROM`, `RCPT TO`, `DATA` et `QUIT` sur une connexion authentifiée.
/// Une adresse accentuée avant le @ demande SMTPUTF8 (RFC 6531, lot H2) : sans
/// cette extension, l'envoi est refusé avant `MAIL FROM`, rien n'est envoyé.
fn smtp_transaction(s: &mut dyn Stream, ehlo: &[String], from: &str, recipients: &[String], data: &[u8]) -> Result<(), String> {
    let utf8 = !from.is_ascii() || recipients.iter().any(|r| !r.is_ascii());
    if utf8 && !has_extension(ehlo, "SMTPUTF8") {
        let _ = smtp_cmd(s, "QUIT", 221);
        return Err(tr!("Le serveur d'envoi n'accepte pas les adresses accentuées avant le @ (extension SMTPUTF8 absente).").into());
    }
    let mail_from = if utf8 { format!("MAIL FROM:<{from}> SMTPUTF8") } else { format!("MAIL FROM:<{from}>") };
    smtp_cmd(s, &mail_from, 250).map_err(|e| tr!("Expéditeur refusé par le serveur : {e}", e = e))?;
    for r in recipients {
        smtp_cmd_codes(s, &format!("RCPT TO:<{r}>"), &[250, 251]).map_err(|e| tr!("Destinataire refusé ({r}) : {e}", r = r, e = e))?;
    }
    smtp_cmd(s, "DATA", 354)?;
    s.write_all(data).map_err(|e| tr!("SMTP : {e}", e = e))?;
    s.flush().map_err(|e| tr!("SMTP : {e}", e = e))?;
    let (code, lines) = smtp_reply(s)?;
    if code != 250 {
        return Err(tr!("Message refusé par le serveur : {code} {reply}", code = code, reply = lines.join(" ")));
    }
    let _ = smtp_cmd(s, "QUIT", 221);
    Ok(())
}

/// Connexion SMTP authentifiée, prête pour `MAIL FROM`.
struct SmtpConnection {
    stream: Box<dyn Stream>,
    security: Security,
    /// Extensions annoncées par `EHLO` (lot H2 : `SMTPUTF8`).
    extensions: Vec<String>,
}

/// Ouvre une [`SmtpConnection`] : connexion, chiffrement, `EHLO` et authentification.
fn smtp_open(cfg: &ServerConfig) -> Result<SmtpConnection, String> {
    let secret = secret(cfg)?;
    let host = cfg.host.trim();
    let security = resolve(cfg.security, cfg.port, 465);
    let tcp = smtp_tcp(host, cfg.port)?;
    let name = hello_name(&tcp);
    let mut stream: Box<dyn Stream> = match security {
        Security::SslTls => Box::new(tls(host, &cfg.pinned_cert, tcp)?),
        Security::StartTls => {
            let mut plain = tcp;
            smtp_expect_greeting(&mut plain)?;
            let ehlo = smtp_hello(&mut plain, &name)?;
            if !ehlo.iter().any(|l| l.eq_ignore_ascii_case("STARTTLS")) {
                return Err(tr!("{host}:{port} ne propose pas STARTTLS. Essayez SSL/TLS (port 465).", host = host, port = cfg.port));
            }
            smtp_cmd(&mut plain, "STARTTLS", 220)?;
            Box::new(tls(host, &cfg.pinned_cert, plain)?)
        }
        Security::Unencrypted | Security::Auto => Box::new(tcp),
    };
    if security != Security::StartTls {
        smtp_expect_greeting(stream.as_mut())?;
    }
    let ehlo = smtp_hello(stream.as_mut(), &name)?;
    if let Err(e) = smtp_auth(stream.as_mut(), &ehlo, cfg.username.trim(), &secret, cfg.auth) {
        if cfg.auth == AuthMethod::OAuth2 {
            oauth_refused(cfg, cfg.username.trim());
        }
        return Err(e);
    }
    Ok(SmtpConnection { stream, security, extensions: ehlo })
}

/// Jeton `OAuth2` refusé par un serveur de courrier : voir `access_refused`.
fn oauth_refused(cfg: &ServerConfig, user: &str) {
    if crate::google::is_google_host(&cfg.host) {
        crate::google::access_refused(user);
    } else {
        crate::oauth::access_refused(user);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smtp(host: &str, auth: AuthMethod) -> ServerConfig {
        ServerConfig {
            host: host.into(),
            port: 587,
            security: Security::StartTls,
            auth,
            username: "f@outlook.fr".into(),
            password: String::new(),
            pinned_cert: String::new(),
        }
    }

    /// Serveur SMTP joué d'avance : ses réponses, et ce que le client écrit.
    struct Scripted {
        replies: std::io::Cursor<Vec<u8>>,
        sent: Vec<u8>,
    }

    impl Scripted {
        fn new(replies: &str) -> Self {
            Self { replies: std::io::Cursor::new(replies.as_bytes().to_vec()), sent: Vec::new() }
        }
        fn sent(&self) -> String {
            String::from_utf8_lossy(&self.sent).into_owned()
        }
    }

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.replies.read(buf)
        }
    }

    impl Write for Scripted {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.sent.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Certificat auto-signé pour 127.0.0.1, comme celui de Proton Mail Bridge :
    /// identité du serveur de test et certificat en DER.
    ///
    /// L'identité passe par un PKCS#12 : sous Windows (schannel),
    /// `Identity::from_pkcs8` n'importe que des clés RSA, et rcgen génère une
    /// clé ECDSA P-256.
    fn self_signed() -> (native_tls::Identity, Vec<u8>) {
        const ALIAS: &str = "serveur de test";
        const PASSWORD: &str = "test";
        let key = rcgen::KeyPair::generate().expect("clé de test");
        let params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()]).expect("paramètres du certificat");
        let cert = params.self_signed(&key).expect("certificat de test");
        let der = cert.der().to_vec();
        let chain = p12_keystore::PrivateKeyChain::new(
            ALIAS,
            p12_keystore::PrivateKey::from_der(&key.serialize_der()).expect("clé de test en PKCS#8"),
            [p12_keystore::Certificate::from_der(&der).expect("certificat de test en DER")],
        );
        let mut store = p12_keystore::KeyStore::new();
        store.add_entry(ALIAS, p12_keystore::KeyStoreEntry::PrivateKeyChain(chain));
        let pfx = store.writer(PASSWORD).write().expect("PKCS#12 du serveur de test");
        let identity = native_tls::Identity::from_pkcs12(&pfx, PASSWORD).expect("identité du serveur de test");
        (identity, der)
    }

    /// Serveur SSL/TLS d'une seule connexion sur 127.0.0.1 ; rend son port.
    fn serve_once(identity: native_tls::Identity) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("port local");
        let port = listener.local_addr().expect("adresse locale").port();
        let acceptor = native_tls::TlsAcceptor::new(identity).expect("accepteur TLS");
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                if let Ok(mut tls) = acceptor.accept(stream) {
                    let _ = tls.write_all(b"* OK\r\n");
                }
            }
        });
        port
    }

    fn connect_pinned(pin: &str) -> Result<(), String> {
        let (identity, _) = self_signed();
        let port = serve_once(identity);
        let stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        tls("127.0.0.1", pin, stream).map(drop)
    }

    #[test]
    fn i3_empreintes() {
        // SHA-256 de « abc » (FIPS 180-2, annexe B.1).
        let abc = fingerprint(b"abc");
        assert!(abc.starts_with("BA:78:16:BF:8F:01:CF:EA:") && abc.ends_with(":F2:00:15:AD"), "{abc}");
        assert!(is_fingerprint(&abc) && is_fingerprint(&abc.to_lowercase()));
        assert!(!is_fingerprint(&abc[3..]) && !is_fingerprint(&abc.replace(':', "")) && !is_fingerprint(&abc.replace("BA", "ZZ")));
        assert!(is_loopback("127.0.0.1") && is_loopback("::1") && is_loopback("127.0.0.2"));
        assert!(!is_loopback("localhost") && !is_loopback("192.168.1.10") && !is_loopback("imap.exemple.fr"));
    }

    #[test]
    fn i3_certificat_epingle() {
        // L'empreinte du certificat du serveur : la connexion passe.
        let (identity, der) = self_signed();
        let port = serve_once(identity);
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connexion locale");
        let mut tls = tls("127.0.0.1", &fingerprint(&der).to_lowercase(), stream).expect("certificat épinglé accepté");
        let mut greeting = [0u8; 6];
        tls.read_exact(&mut greeting).expect("accueil du serveur");
        assert_eq!(&greeting, b"* OK\r\n");

        // Une autre empreinte : refus, rien n'est envoyé.
        let other = fingerprint(b"autre certificat");
        let e = connect_pinned(&other).expect_err("empreinte différente");
        assert!(e.contains("a changé") && e.contains(&other), "{e}");

        // Sans empreinte, un certificat auto-signé reste refusé par la bibliothèque du système.
        assert!(connect_pinned("").is_err(), "certificat auto-signé accepté sans épinglage");
    }

    #[test]
    fn i3_epinglage_hors_boucle_locale_refuse() {
        // Refusé avant toute négociation : aucun octet n'est écrit.
        let stream = std::io::Cursor::new(Vec::new());
        let e = tls("imap.exemple.fr", &fingerprint(b"x"), stream).expect_err("hors de 127.0.0.1");
        assert!(e.contains("127.0.0.1"), "{e}");
    }

    #[test]
    fn i3_compte_avec_certificat_epingle() {
        use crate::config::AccountConfig;
        let server = |host: &str, port: u16, pin: &str| ServerConfig {
            host: host.into(),
            port,
            security: Security::StartTls,
            auth: AuthMethod::Normal,
            username: "moi@proton.me".into(),
            password: "mot de passe de Bridge".into(),
            pinned_cert: pin.into(),
        };
        let pin = fingerprint(b"bridge");
        let account = |imap: ServerConfig| AccountConfig {
            id: String::new(),
            name: "Proton".into(),
            email: "moi@proton.me".into(),
            display_name: String::new(),
            color: "#000".into(),
            imap,
            smtp: server("127.0.0.1", 1025, &pin),
            smtp_same_credentials: true,
            oauth_client_id: String::new(),
            dav_url: String::new(),
            card_url: String::new(),
        };
        assert!(account(server("127.0.0.1", 1143, &pin)).validate().is_ok());
        assert!(account(server("imap.exemple.fr", 993, &pin)).validate().is_err(), "épinglage hors de la boucle locale");
        assert!(account(server("127.0.0.1", 1143, "AB:CD")).validate().is_err(), "empreinte tronquée");
        let mut clear = server("127.0.0.1", 1143, &pin);
        clear.security = Security::Unencrypted;
        assert!(account(clear).validate().is_err(), "épinglage sans chiffrement");
    }

    #[test]
    fn h2_smtputf8() {
        let ok = "250 ok\r\n250 ok\r\n354 go\r\n250 queued\r\n221 bye\r\n";
        let data = "To: rené@x.fr\r\n\r\nBonjour\r\n.\r\n".as_bytes();
        let ehlo = vec!["mail.example".to_string(), "8BITMIME".into(), "SMTPUTF8".into(), "SIZE 1000".into()];

        // Adresse accentuée et serveur qui annonce SMTPUTF8 : le paramètre est donné.
        let mut s = Scripted::new(ok);
        smtp_transaction(&mut s, &ehlo, "moi@x.fr", &["rené@x.fr".into()], data).unwrap();
        assert!(s.sent().starts_with("MAIL FROM:<moi@x.fr> SMTPUTF8\r\nRCPT TO:<rené@x.fr>\r\n"), "{}", s.sent());

        // Adresses en ASCII : envoi ordinaire, sans le paramètre.
        let mut s = Scripted::new(ok);
        smtp_transaction(&mut s, &ehlo, "moi@x.fr", &["a@x.fr".into()], data).unwrap();
        assert!(s.sent().starts_with("MAIL FROM:<moi@x.fr>\r\n"), "{}", s.sent());

        // Serveur sans SMTPUTF8 : refus clair, aucune adresse envoyée.
        let mut s = Scripted::new("221 bye\r\n");
        let err = smtp_transaction(&mut s, &ehlo[..2], "moi@x.fr", &["rené@x.fr".into()], data).unwrap_err();
        assert!(err.contains("SMTPUTF8"), "{err}");
        assert_eq!(s.sent(), "QUIT\r\n", "ni MAIL FROM ni RCPT TO");
        assert!(!has_extension(&["SMTPUTF8X".into()], "SMTPUTF8"), "nom exact");
    }

    #[test]
    fn serveurs_d_outlook_com() {
        assert!(is_microsoft_host("smtp-mail.outlook.com"), "Outlook.com");
        assert!(is_microsoft_host("SMTP.Office365.com."), "O365");
        assert!(!is_microsoft_host("outlook.com.example"), "autre domaine");
        assert!(!is_microsoft_host("monoutlook.com"), "nom proche");
        assert!(!is_microsoft_host("smtp.mailo.com"), "autre service");
    }

    #[test]
    fn envoi_smtp_coupe_par_outlook_com() {
        let refus = "SMTP : connexion OAuth2 refusée (535 5.7.139 Authentication unsuccessful, \
                     SmtpClientAuthentication is disabled for the Mailbox.)";
        assert!(smtp_disabled_by_service(&smtp("smtp-mail.outlook.com", AuthMethod::OAuth2), refus));
        // Mot de passe, compte Google ou autre refus : pas de repli par l'API.
        assert!(!smtp_disabled_by_service(&smtp("smtp-mail.outlook.com", AuthMethod::Normal), refus));
        assert!(!smtp_disabled_by_service(&smtp("smtp.gmail.com", AuthMethod::OAuth2), refus));
        assert!(!smtp_disabled_by_service(&smtp("smtp-mail.outlook.com", AuthMethod::OAuth2), "535 5.7.3 Authentication unsuccessful"));
    }
}
