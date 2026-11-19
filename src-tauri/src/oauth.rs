//! Connexion `OAuth2` des comptes Outlook.com (le service refuse les mots de
//! passe en IMAP et SMTP pour les comptes personnels).
//!
//! - Autorisation : code + PKCE (S256), retour sur `http://localhost:<port>`
//!   (application « publique » de bureau, sans secret).
//! - Le jeton de renouvellement (refresh token) et l'ID d'application sont
//!   rangés dans le trousseau du système, sous `oauth/<adresse>`.
//! - Le jeton d'accès (valable environ une heure) reste en mémoire et se
//!   renouvelle tout seul.
//!
//! Aucune dépendance supplémentaire : SHA-256, base64url et un petit client
//! HTTPS (`native-tls`) sont ici.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub(crate) use crate::endpoints::{LOGIN_HOST, TENANT};
use crate::endpoints::{MAIL_SCOPE as SCOPE, MAIL_SIGN_IN_SCOPE as SIGN_IN_SCOPE, PIM_CONSENT, SEND_SCOPE};
/// Temps laissé pour se connecter dans le navigateur.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

// ================================================================ outils purs

/// SHA-256 du défi PKCE : l'implémentation de `crypto`, seule copie du dépôt (lot 10d).
pub(crate) use crate::crypto::sha256;

/// Base64 « URL » sans remplissage (RFC 4648 §5), utilisé par PKCE.
pub(crate) fn base64url(data: &[u8]) -> String {
    mail_core::compose::base64(data).replace('+', "-").replace('/', "_").trim_end_matches('=').to_string()
}

/// Encodage des paramètres d'URL et de formulaire.
pub(crate) fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

pub(crate) fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Paramètres d'une chaîne de requête (`a=1&b=2`).
pub(crate) fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (url_decode(k), url_decode(v)),
            None => (url_decode(p), String::new()),
        })
        .collect()
}

pub(crate) fn form(params: &[(&str, &str)]) -> String {
    params.iter().map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v))).collect::<Vec<_>>().join("&")
}

/// Corps HTTP en `Transfer-Encoding: chunked`.
pub(crate) fn decode_chunked(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut rest = body;
    loop {
        let line_end = rest.windows(2).position(|w| w == b"\r\n").ok_or(tr!("réponse HTTP tronquée"))?;
        let size_text = String::from_utf8_lossy(&rest[..line_end]);
        let size =
            usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16).map_err(|_| tr!("taille de bloc HTTP invalide"))?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size {
            return Err(tr!("réponse HTTP tronquée").into());
        }
        out.extend_from_slice(&rest[..size]);
        rest = rest.get(size + 2..).unwrap_or(&[]);
    }
}

/// Sépare une réponse HTTP/1.1 : code et corps (déjà décodé si `chunked`).
pub(crate) fn parse_http_response(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or(tr!("réponse HTTP illisible"))?;
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let status: u16 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or(tr!("code HTTP illisible"))?;
    let body = &raw[head_end + 4..];
    let chunked = head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    Ok((status, if chunked { decode_chunked(body)? } else { body.to_vec() }))
}

/// Octets aléatoires du système (sel et nonce du coffre et des sauvegardes,
/// PKCE). Aucun repli : sans source sûre, l'opération échoue plutôt
/// que de produire des valeurs devinables (relecture C6).
pub(crate) fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).map_err(|e| tr!("générateur aléatoire du système indisponible : {e}", e = e))?;
    Ok(buf)
}

// ================================================================ HTTPS minimal

pub(crate) fn https_post(host: &str, path: &str, body: &str) -> Result<(u16, Vec<u8>), String> {
    // Délai de connexion, et toutes les adresses du serveur essayées (audit S7) :
    // un serveur de jetons muet figeait la voie jusqu'au délai du système.
    let tcp = crate::transport::tcp(host, 443)?;
    tcp.set_read_timeout(Some(Duration::from_secs(30))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(30))).ok();
    let connector = crate::transport::tls_connector()?;
    let mut tls = connector.connect(host, tcp).map_err(|e| tr!("TLS avec {host} : {e}", host = host, e = e))?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Vortex Mail\r\nAccept: application/json\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tls.write_all(request.as_bytes()).map_err(|e| tr!("HTTPS : {e}", e = e))?;
    // Certains serveurs ferment sans `close_notify` : on garde ce qui a été
    // reçu. Réponse d'1 Mo au plus (un jeton en fait quelques Kio).
    let (raw, _) = crate::http::read_capped(&mut tls, 1024 * 1024, Duration::from_mins(2))?;
    parse_http_response(&raw)
}

#[derive(Deserialize)]
pub(crate) struct TokenResponse {
    pub(crate) access_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_in: Option<u64>,
    pub(crate) id_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub(crate) fn token_request(params: &[(&str, &str)]) -> Result<TokenResponse, String> {
    token_request_ex(params).map_err(|(_, e)| e)
}

/// Refus définitif d'un jeton (expiré, révoqué, application refusée) : il faut
/// se reconnecter. Les autres erreurs du service (`temporarily_unavailable`,
/// `server_error`…) sont des pannes passagères, sans bandeau « Reconnecter » (A3).
#[must_use]
pub(crate) fn is_rejection(error: &str) -> bool {
    matches!(
        error,
        "invalid_grant" | "interaction_required" | "invalid_client" | "unauthorized_client" | "consent_required" | "login_required"
    )
}

/// Comme `token_request` ; l'erreur indique aussi si le service a refusé la
/// demande (jeton expiré ou révoqué : il faut se reconnecter) plutôt qu'une panne.
pub(crate) fn token_request_ex(params: &[(&str, &str)]) -> Result<TokenResponse, (bool, String)> {
    let path = format!("/{TENANT}/oauth2/v2.0/token");
    let (status, body) = https_post(LOGIN_HOST, &path, &form(params)).map_err(|e| (false, e))?;
    let text = String::from_utf8_lossy(&body);
    let response: TokenResponse =
        serde_json::from_str(&text).map_err(|_| (false, tr!("réponse d'Outlook.com illisible (HTTP {status})", status = status)))?;
    if let Some(error) = &response.error {
        let detail = response.error_description.as_deref().unwrap_or("").lines().next().unwrap_or("");
        return Err((
            is_rejection(error),
            tr!("Outlook.com a refusé la connexion : {error} {detail}", error = error, detail = detail).trim().to_string(),
        ));
    }
    if response.access_token.is_none() {
        return Err((false, tr!("réponse d'Outlook.com sans jeton (HTTP {status})", status = status)));
    }
    Ok(response)
}

/// Champs utiles d'un jeton d'identité (JWT).
#[derive(Deserialize)]
struct Claims {
    email: Option<String>,
    preferred_username: Option<String>,
    /// Application destinataire : un identifiant, ou une liste.
    #[serde(default)]
    aud: Option<Audience>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

/// Charge utile d'un jeton d'identité (signature non vérifiée : le jeton
/// vient directement du serveur de jetons, par HTTPS ; RFC 8252 §8.10).
fn claims(id_token: &str) -> Option<Claims> {
    let payload = id_token.split('.').nth(1)?;
    let json = mail_core::compose::base64_decode(payload).ok()?;
    serde_json::from_slice(&json).ok()
}

/// Adresse du compte connecté, lue dans le jeton d'identité.
pub(crate) fn id_token_email(id_token: &str) -> Option<String> {
    let claims = claims(id_token)?;
    claims.email.or(claims.preferred_username).map(|e| e.trim().to_lowercase())
}

/// Adresse réellement connectée par le navigateur, d'après le jeton
/// d'identité. Il doit être là (la connexion demande `openid email`), lisible,
/// et destiné à cette application (`aud`, `OpenID Connect` Core §3.1.3.7) :
/// sinon la connexion est refusée, au lieu de ranger sans contrôle le jeton
/// d'un autre compte sous l'adresse saisie (relecture complète n° 51).
///
/// # Errors
///
/// Jeton absent, illisible, sans adresse, ou destiné à une autre application.
pub(crate) fn signed_in_as(id_token: Option<&str>, client_id: &str) -> Result<String, String> {
    let unknown =
        || tr!("Le service n'a pas indiqué quel compte s'est connecté : connexion refusée par prudence. Recommencez.").to_string();
    let claims = id_token.and_then(claims).ok_or_else(unknown)?;
    let for_us = match &claims.aud {
        Some(Audience::One(aud)) => aud == client_id,
        Some(Audience::Many(list)) => list.iter().any(|aud| aud == client_id),
        None => false,
    };
    if !for_us {
        return Err(tr!("Jeton d'identité destiné à une autre application : connexion refusée.").into());
    }
    claims.email.or(claims.preferred_username).map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()).ok_or_else(unknown)
}

// ================================================================ stockage

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    client_id: String,
    refresh_token: String,
}

fn key(user: &str) -> String {
    format!("oauth/{}", user.trim().to_lowercase())
}

fn load(user: &str) -> Result<Option<Stored>, String> {
    Ok(crate::secrets::read(&key(user), &token_label(user))?.and_then(|json| serde_json::from_str(&json).ok()))
}

fn token_label(user: &str) -> String {
    tr!("Vortex Mail — {user} (connexion Outlook.com)", user = user.trim().to_lowercase())
}

/// Libellé de l'ID d'application dans le trousseau (dans la langue de l'interface).
fn client_label() -> &'static str {
    tr!("Vortex Mail — ID d'application Outlook.com")
}

fn save(user: &str, stored: &Stored) -> Result<(), String> {
    let json = serde_json::to_string(stored).map_err(|e| e.to_string())?;
    crate::secrets::write(&key(user), &token_label(user), &json)
}

/// Libellé lisible pour un jeton enregistré par une version précédente.
pub(crate) fn relabel(user: &str) -> Result<(), String> {
    crate::secrets::relabel(&key(user), &token_label(user))
}

pub(crate) fn relabel_client_id() -> Result<(), String> {
    crate::secrets::relabel(DEFAULT_CLIENT, client_label())
}

/// Jetons d'accès en mémoire : adresse → (jeton, expiration).
static ACCESS: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::new(None);

fn cache_get(user: &str) -> Option<String> {
    let guard = ACCESS.lock().ok()?;
    let (token, expiry) = guard.as_ref()?.get(&key(user))?;
    (Instant::now() + Duration::from_secs(120) < *expiry).then(|| token.clone())
}

fn cache_put(user: &str, token: &str, lifetime: u64) {
    if let Ok(mut guard) = ACCESS.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(key(user), (token.to_string(), Instant::now() + Duration::from_secs(lifetime.max(60))));
    }
}

/// Jeton d'accès refusé par un serveur (révoqué entre-temps) : il est oublié et
/// un renouvellement est tenté ; si le service refuse aussi le jeton de
/// renouvellement, l'interface propose « Reconnecter ».
pub(crate) fn access_refused(user: &str) {
    if let Ok(mut guard) = ACCESS.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(&key(user));
        }
    }
    let _ = access_token(user);
}

/// Entrée du trousseau gardant le dernier ID d'application utilisé.
const DEFAULT_CLIENT: &str = "oauth/client-id";

/// Entrées du trousseau d'un compte à sauvegarder : (clé, libellé).
pub(crate) fn backup_entries(user: &str) -> Vec<(String, String)> {
    vec![(key(user), token_label(user))]
}

/// ID d'application Outlook.com, commun aux comptes.
pub(crate) fn backup_global() -> (String, String) {
    (DEFAULT_CLIENT.to_string(), client_label().to_string())
}

/// Range l'ID d'application dans le trousseau (jamais dans un fichier).
pub(crate) fn remember_client_id(client_id: &str) -> Result<(), String> {
    crate::secrets::write(DEFAULT_CLIENT, client_label(), client_id.trim())
}

/// ID d'application connu pour cette adresse, sinon le dernier utilisé.
pub(crate) fn known_client_id(user: &str) -> Option<String> {
    if let Ok(Some(stored)) = load(user) {
        return Some(stored.client_id);
    }
    crate::secrets::read(DEFAULT_CLIENT, client_label()).ok().flatten()
}

/// Connecté (jeton de renouvellement présent) ?
pub(crate) fn is_signed_in(user: &str) -> bool {
    matches!(load(user), Ok(Some(_)))
}

/// Oublie la connexion d'une adresse (compte supprimé).
pub(crate) fn forget(user: &str) -> Result<(), String> {
    crate::auth::forgotten("o365", crate::auth::MAIL, user);
    crate::auth::sign_out(&key(user), || {
        if let Ok(mut guard) = ACCESS.lock() {
            if let Some(map) = guard.as_mut() {
                map.remove(&key(user));
            }
        }
        crate::secrets::remove(&key(user), &token_label(user))
    })
}

/// Jeton d'accès valide pour IMAP et SMTP (renouvelé si nécessaire).
/// Un renouvellement de jeton à la fois (audit S7) : à l'expiration, plusieurs
/// voies demandaient un jeton en même temps, et un fournisseur qui fait
/// tourner les jetons de renouvellement pouvait invalider l'un d'eux. La voie
/// qui attend retrouve ensuite le jeton neuf en mémoire. Partagé avec Google.
pub(crate) static REFRESHING: Mutex<()> = Mutex::new(());

pub(crate) fn access_token(user: &str) -> Result<String, String> {
    if let Some(token) = cache_get(user) {
        return Ok(token);
    }
    // Garde sans donnée : une panique ailleurs ne laisse rien à moitié écrit.
    let _refreshing = REFRESHING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(token) = cache_get(user) {
        return Ok(token);
    }
    let signed_out = crate::auth::sign_out_count(&key(user));
    let stored = load(user)?.ok_or_else(|| {
        crate::auth::sign_in_required("o365", crate::auth::MAIL, user);
        tr!("Compte Outlook.com non connecté. Ouvrez Paramètres, modifiez le compte \
             et cliquez sur « Se connecter à Outlook.com ».")
        .to_string()
    })?;
    let response = token_request_ex(&[
        ("client_id", &stored.client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", &stored.refresh_token),
        ("scope", SCOPE),
    ])
    .map_err(|(rejected, e)| {
        if rejected {
            crate::auth::sign_in_required("o365", crate::auth::MAIL, user);
        }
        tr!("{e}. Reconnectez le compte dans Paramètres.", e = e)
    })?;
    let token = response.access_token.clone().unwrap_or_default();
    // Rien n'est gardé si le compte a été déconnecté pendant la demande (A8).
    crate::auth::unless_signed_out(&key(user), signed_out, || {
        // Le service renouvelle aussi le jeton de renouvellement : on garde le dernier.
        if let Some(refresh) = response.refresh_token.as_deref().filter(|r| *r != stored.refresh_token) {
            save(user, &Stored { client_id: stored.client_id.clone(), refresh_token: refresh.to_string() })?;
        }
        cache_put(user, &token, response.expires_in.unwrap_or(3600));
        Ok(())
    })?;
    Ok(token)
}

/// Jeton de l'API d'Outlook.com pour envoyer un message, tiré du jeton de
/// renouvellement du courrier. Sert quand le service a coupé l'envoi SMTP de
/// la boîte (comptes récents, code 5.7.139). Une connexion faite avant cette
/// autorisation doit être refaite (« Se connecter à Outlook.com »).
///
/// # Errors
///
/// Compte non connecté, ou envoi par l'API pas encore autorisé.
pub(crate) fn send_token(user: &str) -> Result<String, String> {
    let stored = load(user)?.ok_or_else(|| {
        tr!("Compte Outlook.com non connecté. Ouvrez Paramètres, modifiez le compte \
             et cliquez sur « Se connecter à Outlook.com ».")
        .to_string()
    })?;
    let response = token_request(&[
        ("client_id", &stored.client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", &stored.refresh_token),
        ("scope", SEND_SCOPE),
    ])
    .map_err(|e| {
        tr!(
            "Outlook.com n'accepte pas l'envoi SMTP pour cette boîte ; l'envoi par l'API d'Outlook.com \
             n'est pas encore autorisé ({e}). Ouvrez Paramètres, modifiez le compte et cliquez sur \
             « Se connecter à Outlook.com ».",
            e = e
        )
    })?;
    Ok(response.access_token.unwrap_or_default())
}

// ================================================================ connexion dans le navigateur

/// Ouvre une adresse dans le navigateur par défaut, sans interpréteur de
/// commandes (`ShellExecuteW` sous Windows, `xdg-open` ou `open` ailleurs).
pub(crate) fn open_browser(url: &str) -> Result<(), String> {
    opener::open(url).map_err(|e| tr!("impossible d'ouvrir le navigateur : {e}", e = e))
}

/// Page affichée dans le navigateur au retour de la connexion (textes traduits).
fn page(title: &str, text: &str) -> String {
    format!(
        "<!doctype html><html lang={}><meta charset=utf-8><title>Vortex Mail</title>\
         <body style=\"font-family:system-ui,sans-serif;text-align:center;padding-top:15vh\">\
         <h2>{title}</h2><p>{text}</p>",
        crate::i18n::lang()
    )
}

pub(crate) fn page_ok() -> String {
    page(tr!("Connexion réussie"), tr!("Vous pouvez fermer cet onglet et revenir à Vortex Mail."))
}

pub(crate) fn page_ko() -> String {
    page(tr!("Connexion annulée ou refusée"), tr!("Revenez à Vortex Mail pour réessayer."))
}

pub(crate) fn reply(stream: &mut TcpStream, page: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
}

/// Attend le retour du navigateur sur `http://localhost:<port>/?code=…&state=…`.
fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, String> {
    wait_for_code_from(listener, state, "Outlook.com")
}

/// Idem, pour un fournisseur nommé dans les messages d'erreur.
pub(crate) fn wait_for_code_from(listener: &TcpListener, state: &str, provider: &str) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + SIGN_IN_TIMEOUT;
    loop {
        if Instant::now() > deadline {
            return Err(tr!("Délai dépassé : la connexion {provider} n'a pas été terminée.", provider = provider));
        }
        let (mut stream, _) = match listener.accept() {
            Ok(conn) => conn,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        stream.set_nonblocking(false).ok();
        stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let target = request.split_whitespace().nth(1).unwrap_or("");
        let Some((_, query)) = target.split_once('?') else {
            // favicon.ico ou autre : on ignore.
            let _ = write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            continue;
        };
        let params = parse_query(query);
        if params.get("state").map(String::as_str) != Some(state) {
            let _ = write!(stream, "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            continue;
        }
        if let Some(code) = params.get("code") {
            reply(&mut stream, &page_ok());
            return Ok(code.clone());
        }
        reply(&mut stream, &page_ko());
        let error = params.get("error").cloned().unwrap_or_default();
        let detail = params.get("error_description").cloned().unwrap_or_default();
        return Err(if error == "access_denied" {
            tr!("Connexion annulée dans le navigateur.").into()
        } else {
            tr!("{provider} a refusé la connexion : {error} {detail}", provider = provider, error = error, detail = detail)
                .trim()
                .to_string()
        });
    }
}

// Autorisations demandées en plus à la connexion (PIM_CONSENT) : agenda et
// contacts, accordés dans la même page ; leur jeton s'obtient ensuite à part.

/// Résultat d'une connexion Outlook.com.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignInReport {
    /// Agenda et contacts connectés en même temps que le courrier.
    pub calendar: bool,
}

/// Un passage dans le navigateur ; renvoie la réponse du service (jetons du courrier).
fn authorize(client_id: &str, user: &str, consent: &str) -> Result<TokenResponse, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| tr!("port local indisponible : {e}", e = e))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://localhost:{port}");
    let verifier = base64url(&random_bytes::<32>()?);
    let challenge = base64url(&sha256(verifier.as_bytes()));
    let state = base64url(&random_bytes::<16>()?);
    let scope = if consent.is_empty() { SIGN_IN_SCOPE.to_string() } else { format!("{SIGN_IN_SCOPE} {consent}") };
    let url = format!(
        "https://{LOGIN_HOST}/{TENANT}/oauth2/v2.0/authorize?{}",
        form(&[
            ("client_id", client_id),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            ("response_mode", "query"),
            ("scope", &scope),
            ("login_hint", user.trim()),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("prompt", "select_account"),
        ])
    );
    open_browser(&url)?;
    let code = wait_for_code(&listener, &state)?;
    // Le jeton échangé ne vaut que pour un service : celui du courrier.
    token_request(&[
        ("client_id", client_id),
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect),
        ("code_verifier", &verifier),
        ("scope", SIGN_IN_SCOPE),
    ])
}

/// Ouvre la page de connexion Outlook.com dans le navigateur et enregistre les
/// jetons du courrier, puis ceux de l'agenda et des contacts (même passage).
pub(crate) fn sign_in(client_id: &str, user: &str) -> Result<SignInReport, String> {
    // Champ laissé vide : ID déjà rangé dans le trousseau.
    let known = if client_id.trim().is_empty() { known_client_id(user) } else { None };
    let client_id = known.as_deref().unwrap_or(client_id).trim();
    if client_id.len() < 30 || !client_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(tr!(
            "ID d'application (client) invalide : copiez-le depuis l'inscription de l'application (portail d'administration O365)."
        )
        .into());
    }
    let mut combined = true;
    let response = match authorize(client_id, user, PIM_CONSENT) {
        Ok(r) => r,
        // Autorisations de deux services refusées ensemble : courrier seul, puis agenda à part.
        Err(e) if e.contains("invalid_scope") || e.contains("AADSTS70011") || e.contains("AADSTS28000") => {
            crate::diag::report(&format_args!("connexion groupée refusée ({e}), courrier seul"));
            combined = false;
            authorize(client_id, user, "")?
        }
        Err(e) => return Err(e),
    };
    // Le navigateur était peut-être déjà connecté avec un autre compte.
    let who = signed_in_as(response.id_token.as_deref(), client_id)?;
    if who != user.trim().to_lowercase() {
        return Err(tr!(
            "Le navigateur a connecté le compte « {who} » et non « {user} ». \
             Recommencez en choisissant le bon compte sur la page de connexion \
             (ou déconnectez-vous d'abord de {who} sur outlook.com).",
            who = who,
            user = user.trim()
        ));
    }
    let refresh =
        response.refresh_token.ok_or(tr!("Outlook.com n'a pas fourni de jeton de renouvellement (autorisation offline_access ?)"))?;
    save(user, &Stored { client_id: client_id.to_string(), refresh_token: refresh.clone() })?;
    if let Err(e) = remember_client_id(client_id) {
        crate::diag::report(&e);
    }
    cache_put(user, &response.access_token.unwrap_or_default(), response.expires_in.unwrap_or(3600));

    // Agenda et contacts : jeton tiré du même passage, sinon second passage.
    let calendar = if combined {
        match crate::o365::adopt(user, client_id, &refresh) {
            Ok(()) => Ok(()),
            Err(e) => {
                crate::diag::report(&format_args!("agenda et contacts non accordés d'un coup ({e}), second passage"));
                crate::o365::sign_in(user)
            }
        }
    } else {
        crate::o365::sign_in(user)
    };
    match calendar {
        Ok(()) => Ok(SignInReport { calendar: true }),
        Err(e) => {
            // Le courrier est connecté ; l'agenda pourra l'être plus tard.
            crate::diag::report(&format_args!("agenda et contacts : {e}"));
            Ok(SignInReport { calendar: false })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_definitive_refusals_ask_to_sign_in_again() {
        // Relecture A3 : une panne passagère du service ne demande pas de se reconnecter.
        for refused in ["invalid_grant", "interaction_required", "invalid_client", "unauthorized_client"] {
            assert!(is_rejection(refused), "{refused}");
        }
        for transient in ["temporarily_unavailable", "server_error", "invalid_request", "slow_down"] {
            assert!(!is_rejection(transient), "{transient}");
        }
    }

    #[test]
    fn pkce_challenge_rfc7636() {
        // Exemple de l'annexe B de la RFC 7636.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(base64url(&sha256(verifier.as_bytes())), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn query_and_form() {
        let q = parse_query("code=M.C1%2Fab-cd&state=xyz&session_state=a+b");
        assert_eq!(q["code"], "M.C1/ab-cd");
        assert_eq!(q["state"], "xyz");
        assert_eq!(q["session_state"], "a b");
        assert_eq!(form(&[("scope", "a b/c"), ("x", "é")]), "scope=a%20b%2Fc&x=%C3%A9");
        assert_eq!(url_decode("100%"), "100%");
    }

    #[test]
    fn email_from_id_token() {
        // En-tête et signature factices : seule la charge utile est lue.
        let payload = base64url(br#"{"preferred_username":"Fabrice@Example.org","name":"F"}"#);
        assert_eq!(id_token_email(&format!("e30.{payload}.sig")).as_deref(), Some("fabrice@example.org"));
        assert_eq!(id_token_email("illisible"), None);
    }

    #[test]
    fn jeton_d_identite_controle() {
        // Relecture complète n° 51 : absent, autre application, sans adresse : refusé.
        let token = |json: &str| format!("e30.{}.sig", base64url(json.as_bytes()));
        let ok = token(r#"{"email":"A@x.fr","aud":"client-1"}"#);
        assert_eq!(signed_in_as(Some(&ok), "client-1").as_deref(), Ok("a@x.fr"));
        let many = token(r#"{"preferred_username":"a@x.fr","aud":["autre","client-1"]}"#);
        assert_eq!(signed_in_as(Some(&many), "client-1").as_deref(), Ok("a@x.fr"));
        assert!(signed_in_as(None, "client-1").is_err(), "jeton absent");
        assert!(signed_in_as(Some("illisible"), "client-1").is_err(), "jeton illisible");
        assert!(signed_in_as(Some(&ok), "client-2").is_err(), "autre application");
        assert!(signed_in_as(Some(&token(r#"{"email":"a@x.fr"}"#)), "client-1").is_err(), "sans aud");
        assert!(signed_in_as(Some(&token(r#"{"aud":"client-1"}"#)), "client-1").is_err(), "sans adresse");
    }

    #[test]
    fn http_responses() {
        let plain = b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\n\r\n{\"error\":\"x\"}";
        assert_eq!(parse_http_response(plain).unwrap(), (400, b"{\"error\":\"x\"}".to_vec()));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        assert_eq!(parse_http_response(chunked).unwrap(), (200, b"{\"a\":1}".to_vec()));
    }
}
