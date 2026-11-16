//! Connexion `OAuth2` Google des comptes Gmail : agenda (`CalDAV`), contacts
//! (`CardDAV`) et, si le compte est réglé en `OAuth2`, courrier (IMAP et SMTP en
//! XOAUTH2, sans mot de passe d'application).
//!
//! - Application Google « de bureau » : ID client + code secret client
//!   (Google l'exige même avec PKCE). Les deux sont rangés dans le trousseau,
//!   sous `google/client`, jamais dans un fichier ni renvoyés à l'interface.
//! - Retour du navigateur sur `http://127.0.0.1:<port>`, code + PKCE (S256).
//! - Jeton de renouvellement dans le trousseau sous `google/<adresse>` ;
//!   jeton d'accès (une heure) en mémoire.

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::oauth::{base64url, form, https_post, open_browser, random_bytes, sha256, signed_in_as, wait_for_code_from};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_HOST: &str = "oauth2.googleapis.com";
const TOKEN_PATH: &str = "/token";
const SCOPE: &str = "openid email https://www.googleapis.com/auth/calendar https://www.googleapis.com/auth/carddav";
/// Accès au courrier (IMAP, SMTP), demandé seulement pour un compte réglé en `OAuth2`.
const MAIL_SCOPE: &str = "https://mail.google.com/";

/// Point de départ `CalDAV` (découverte du principal).
pub(crate) const CALDAV_START: &str = "https://apidata.googleusercontent.com/caldav/v2/";
/// Point de départ `CardDAV` (redirigé vers le principal).
pub(crate) const CARDDAV_START: &str = "https://www.googleapis.com/.well-known/carddav";

const CLIENT_KEY: &str = "google/client";
/// Libellé de l'application Google dans le trousseau (dans la langue de l'interface).
fn client_label() -> &'static str {
    tr!("Vortex Mail — application Google")
}

// ================================================================ trousseau

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Client {
    client_id: String,
    client_secret: String,
}

fn token_key(user: &str) -> String {
    format!("google/{}", user.trim().to_lowercase())
}

fn token_label(user: &str) -> String {
    tr!("Vortex Mail — {user} (connexion Google)", user = user.trim().to_lowercase())
}

/// Entrées du trousseau d'un compte à sauvegarder : (clé, libellé).
pub(crate) fn backup_entries(user: &str) -> Vec<(String, String)> {
    vec![(token_key(user), token_label(user)), (user_client_key(user), user_client_label(user))]
}

/// Application Google (ID client et secret), commune aux comptes.
pub(crate) fn backup_global() -> (String, String) {
    (CLIENT_KEY.to_string(), client_label().to_string())
}

fn read(key: &str, label: &str) -> Result<Option<String>, String> {
    crate::secrets::read(key, label)
}

fn write(key: &str, label: &str, value: &str) -> Result<(), String> {
    crate::secrets::write(key, label, value)
}

fn load_client() -> Option<Client> {
    let json = read(CLIENT_KEY, client_label()).ok()??;
    serde_json::from_str::<Client>(&json).ok().filter(|c| !c.client_id.is_empty() && !c.client_secret.is_empty())
}

fn save_client(client: &Client) -> Result<(), String> {
    write(CLIENT_KEY, client_label(), &serde_json::to_string(client).map_err(|e| e.to_string())?)
}

/// Application Google qui a délivré le jeton d'un compte : un jeton ne se
/// renouvelle qu'avec elle. Gardée par compte, pour qu'une nouvelle
/// application saisie pour un autre compte ne le déconnecte pas (A4).
fn user_client_key(user: &str) -> String {
    format!("google/client/{}", user.trim().to_lowercase())
}

fn user_client_label(user: &str) -> String {
    tr!("Vortex Mail — {user} (application Google)", user = user.trim().to_lowercase())
}

/// Application du compte ; à défaut (connexion faite par une version
/// précédente), l'application commune.
fn load_user_client(user: &str) -> Option<Client> {
    read(&user_client_key(user), &user_client_label(user))
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<Client>(&json).ok())
        .filter(|c| !c.client_id.is_empty() && !c.client_secret.is_empty())
        .or_else(load_client)
}

fn save_user_client(user: &str, client: &Client) -> Result<(), String> {
    write(&user_client_key(user), &user_client_label(user), &serde_json::to_string(client).map_err(|e| e.to_string())?)
}

fn load_refresh(user: &str) -> Result<Option<String>, String> {
    read(&token_key(user), &token_label(user))
}

/// Application Google déjà enregistrée (inutile de ressaisir ID et secret) ?
pub(crate) fn has_client() -> bool {
    load_client().is_some()
}

/// Connecté à Google (jeton de renouvellement présent) ?
pub(crate) fn is_signed_in(user: &str) -> bool {
    matches!(load_refresh(user), Ok(Some(_)))
}

/// Compte Gmail (ou Google Workspace configuré sur imap.gmail.com) ?
/// Comparaison exacte ou par sous-domaine : `imap.mygmail.com` n'est pas
/// Google et ne doit jamais recevoir de jeton Google (relecture A6).
pub(crate) fn is_google_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    ["gmail.com", "googlemail.com"]
        .iter()
        .any(|domain| host == *domain || host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.')))
}

// ================================================================ jetons d'accès

static ACCESS: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::new(None);

fn cache_get(user: &str) -> Option<String> {
    let guard = ACCESS.lock().ok()?;
    let (token, expiry) = guard.as_ref()?.get(&token_key(user))?;
    (Instant::now() + Duration::from_secs(120) < *expiry).then(|| token.clone())
}

fn cache_put(user: &str, token: &str, lifetime: u64) {
    if let Ok(mut guard) = ACCESS.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(token_key(user), (token.to_string(), Instant::now() + Duration::from_secs(lifetime.max(60))));
    }
}

fn cache_drop(user: &str) {
    if let Ok(mut guard) = ACCESS.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(&token_key(user));
        }
    }
}

/// Jeton d'accès refusé par un serveur (révoqué entre-temps) : il est oublié et
/// un renouvellement est tenté ; si le service refuse aussi le jeton de
/// renouvellement, l'interface propose « Reconnecter ».
pub(crate) fn access_refused(user: &str) {
    if let Ok(mut guard) = ACCESS.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(&token_key(user));
        }
    }
    let _ = access_token(user);
}

/// Comme `access_refused`, quand seul le jeton refusé est connu.
pub(crate) fn token_refused(token: &str) {
    let user = ACCESS
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref()?.iter().find(|(_, (t, _))| t == token).map(|(k, _)| k.trim_start_matches("google/").to_string()));
    if let Some(user) = user {
        access_refused(&user);
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    id_token: Option<String>,
    /// Autorisations réellement accordées (cases cochées sur la page Google).
    scope: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

fn token_request(params: &[(&str, &str)]) -> Result<TokenResponse, String> {
    token_request_ex(params).map_err(|(_, e)| e)
}

/// Comme `token_request` ; l'erreur indique aussi si Google a refusé la demande
/// (autorisation expirée ou révoquée : il faut se reconnecter) plutôt qu'une panne.
fn token_request_ex(params: &[(&str, &str)]) -> Result<TokenResponse, (bool, String)> {
    let (status, body) = https_post(TOKEN_HOST, TOKEN_PATH, &form(params)).map_err(|e| (false, e))?;
    let text = String::from_utf8_lossy(&body);
    let response: TokenResponse =
        serde_json::from_str(&text).map_err(|_| (false, tr!("réponse Google illisible (HTTP {status})", status = status)))?;
    if let Some(error) = &response.error {
        let detail = response.error_description.as_deref().unwrap_or("");
        return Err((
            crate::oauth::is_rejection(error),
            match error.as_str() {
                "invalid_client" => tr!("Google refuse l'application : vérifiez l'ID client et le code secret.").into(),
                "invalid_grant" => tr!("Autorisation Google expirée ou révoquée").into(),
                _ => tr!("Google a refusé la connexion : {error} {detail}", error = error, detail = detail).trim().to_string(),
            },
        ));
    }
    if response.access_token.is_none() {
        return Err((false, tr!("réponse Google sans jeton (HTTP {status})", status = status)));
    }
    Ok(response)
}

/// Jeton d'accès valide pour `CalDAV`, `CardDAV` et, s'il a été autorisé, le
/// courrier (renouvelé si nécessaire).
pub(crate) fn access_token(user: &str) -> Result<String, String> {
    if let Some(token) = cache_get(user) {
        return Ok(token);
    }
    let signed_out = crate::auth::sign_out_count(&token_key(user));
    let not_connected = || {
        crate::auth::sign_in_required("google", crate::auth::ALL, user);
        tr!("Compte Google non connecté : ouvrez Paramètres, modifiez le compte \
             et cliquez sur « Se connecter avec Google ».")
        .to_string()
    };
    let refresh = load_refresh(user)?.ok_or_else(not_connected)?;
    let client = load_user_client(user).ok_or_else(not_connected)?;
    let response = token_request_ex(&[
        ("client_id", &client.client_id),
        ("client_secret", &client.client_secret),
        ("grant_type", "refresh_token"),
        ("refresh_token", &refresh),
    ])
    .map_err(|(rejected, e)| {
        if rejected {
            crate::auth::sign_in_required("google", crate::auth::ALL, user);
        }
        tr!("{e}. Reconnectez le compte à Google dans Paramètres.", e = e)
    })?;
    let token = response.access_token.clone().unwrap_or_default();
    // Rien n'est gardé si le compte a été déconnecté pendant la demande (A8).
    crate::auth::unless_signed_out(&token_key(user), signed_out, || {
        if let Some(new_refresh) = response.refresh_token.as_deref().filter(|r| *r != refresh) {
            write(&token_key(user), &token_label(user), new_refresh)?;
        }
        cache_put(user, &token, response.expires_in.unwrap_or(3600));
        Ok(())
    })?;
    Ok(token)
}

/// Oublie la connexion Google d'une adresse (compte supprimé ou déconnexion).
pub(crate) fn forget(user: &str) -> Result<(), String> {
    crate::auth::forgotten("google", crate::auth::ALL, user);
    crate::auth::sign_out(&token_key(user), || {
        cache_drop(user);
        let _ = crate::secrets::remove(&user_client_key(user), &user_client_label(user));
        crate::secrets::remove(&token_key(user), &token_label(user))
    })
}

// ================================================================ connexion

fn valid_client_id(id: &str) -> bool {
    id.ends_with(".apps.googleusercontent.com")
        && id.len() > 40
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
}

/// Ouvre la page de connexion Google dans le navigateur et enregistre le jeton.
/// ID et secret vides : ceux déjà rangés dans le trousseau. `mail` : demander
/// aussi l'accès au courrier (compte réglé en `OAuth2`).
pub(crate) fn sign_in(user: &str, client_id: &str, client_secret: &str, mail: bool) -> Result<(), String> {
    let scope = if mail { format!("{SCOPE} {MAIL_SCOPE}") } else { SCOPE.to_string() };
    let (client_id, client_secret) = (client_id.trim(), client_secret.trim());
    let client = if client_id.is_empty() && client_secret.is_empty() {
        load_user_client(user).ok_or(tr!("Saisissez l'ID client et le code secret de votre application Google."))?
    } else {
        if !valid_client_id(client_id) {
            return Err(tr!("ID client invalide : il se termine par « .apps.googleusercontent.com ».").into());
        }
        if client_secret.len() < 10 || client_secret.chars().any(char::is_whitespace) {
            return Err(tr!("Code secret client invalide : copiez-le depuis la console Google Cloud.").into());
        }
        Client { client_id: client_id.into(), client_secret: client_secret.into() }
    };
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| tr!("port local indisponible : {e}", e = e))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let verifier = base64url(&random_bytes::<32>()?);
    let challenge = base64url(&sha256(verifier.as_bytes()));
    let state = base64url(&random_bytes::<16>()?);
    let url = format!(
        "{AUTH_URL}?{}",
        form(&[
            ("client_id", &client.client_id),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            ("scope", &scope),
            ("login_hint", user.trim()),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            // Jeton de renouvellement à chaque connexion.
            ("access_type", "offline"),
            ("prompt", "consent select_account"),
            ("include_granted_scopes", "true"),
        ])
    );
    open_browser(&url)?;
    let code = wait_for_code_from(&listener, &state, "Google")?;
    let response = token_request(&[
        ("client_id", &client.client_id),
        ("client_secret", &client.client_secret),
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect),
        ("code_verifier", &verifier),
    ])?;
    let who = signed_in_as(response.id_token.as_deref(), &client.client_id)?;
    if gmail_identity(&who) != gmail_identity(user) {
        return Err(tr!(
            "Le navigateur a connecté le compte Google « {who} » et non « {user} ». \
             Recommencez en choisissant le bon compte sur la page Google.",
            who = who,
            user = user.trim()
        ));
    }
    let missing = missing_scopes(response.scope.as_deref().unwrap_or(&scope), mail);
    if !missing.is_empty() {
        return Err(tr!(
            "Accès non accordé pour : {what}. Recommencez et cochez toutes les cases sur la page Google \
             (ou « Tout sélectionner ») avant de cliquer sur Continuer.",
            what = join_labels(&missing)
        ));
    }
    let refresh = response.refresh_token.ok_or(tr!("Google n'a pas fourni de jeton de renouvellement. Retirez l'accès de Vortex Mail \
         sur myaccount.google.com/permissions puis reconnectez-vous."))?;
    write(&token_key(user), &token_label(user), &refresh)?;
    // Application du compte (ses jetons ne se renouvellent qu'avec elle), et
    // application proposée par défaut pour les prochaines connexions.
    save_user_client(user, &client)?;
    save_client(&client)?;
    cache_put(user, &response.access_token.unwrap_or_default(), response.expires_in.unwrap_or(3600));
    Ok(())
}

/// Autorisations nécessaires absentes de la réponse de Google.
fn missing_scopes(granted: &str, mail: bool) -> Vec<&'static str> {
    let granted: Vec<&str> = granted.split_whitespace().collect();
    [
        (MAIL_SCOPE, "le courrier", mail),
        ("https://www.googleapis.com/auth/calendar", "l'agenda", true),
        ("https://www.googleapis.com/auth/carddav", "les contacts", true),
    ]
    .into_iter()
    .filter(|(scope, _, wanted)| *wanted && !granted.contains(scope))
    .map(|(_, label, _)| label)
    .collect()
}

/// Libellé traduit d'une autorisation renvoyée par `missing_scopes` (clé française).
fn scope_label(fr: &'static str) -> &'static str {
    match fr {
        "le courrier" => tr!("le courrier"),
        "l'agenda" => tr!("l'agenda"),
        "les contacts" => tr!("les contacts"),
        _ => fr,
    }
}

/// « le courrier », « le courrier et l'agenda », « le courrier, l'agenda et les contacts ».
fn join_labels(missing: &[&'static str]) -> String {
    let l: Vec<&str> = missing.iter().map(|m| scope_label(m)).collect();
    match l.as_slice() {
        [a] => a.to_string(),
        [a, b] => tr!("{a} et {b}", a = a, b = b),
        [a, b, c] => tr!("{a}, {b} et {c}", a = a, b = b, c = c),
        _ => l.join(", "),
    }
}

/// Boîte Gmail désignée par une adresse : `googlemail.com` est `gmail.com`,
/// et les points de la partie locale n'y comptent pas (`prenom.nom` =
/// `prenomnom`) ; une adresse d'un autre domaine est seulement mise en
/// minuscules (relecture complète n° 51).
fn gmail_identity(address: &str) -> String {
    let address = address.trim().to_lowercase();
    match address.rsplit_once('@') {
        Some((local, "gmail.com" | "googlemail.com")) => {
            let local = local.split('+').next().unwrap_or(local).replace('.', "");
            format!("{local}@gmail.com")
        }
        _ => address,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_id_shape() {
        assert!(valid_client_id("123456789012-abcdefghijklmnopqrstuvwxyz012345.apps.googleusercontent.com"));
        assert!(!valid_client_id("123456789012"));
        assert!(!valid_client_id("x y.apps.googleusercontent.com"));
    }

    #[test]
    fn unchecked_boxes() {
        assert_eq!(missing_scopes(SCOPE, false).len(), 0);
        assert_eq!(missing_scopes("openid https://www.googleapis.com/auth/calendar", false), vec!["les contacts"]);
        assert_eq!(missing_scopes("openid email", false), vec!["l'agenda", "les contacts"]);
        // Compte en OAuth2 : le courrier est exigé aussi.
        assert_eq!(missing_scopes(SCOPE, true), vec!["le courrier"]);
        assert_eq!(missing_scopes(&format!("{SCOPE} {MAIL_SCOPE}"), true).len(), 0);
    }

    #[test]
    fn google_hosts() {
        assert!(is_google_host("imap.gmail.com"));
        assert!(is_google_host("IMAP.googlemail.com "));
        assert!(!is_google_host("imap.mailo.com"));
        assert!(!is_google_host("gmail.com.exemple.fr"));
        assert!(is_google_host("gmail.com"));
        assert!(is_google_host("imap.gmail.com."));
        assert!(!is_google_host("imap.mygmail.com"));
        assert!(!is_google_host("mail.evilgmail.com"));
        assert!(!is_google_host("notgooglemail.com"));
    }

    #[test]
    fn meme_boite_gmail() {
        assert_eq!(gmail_identity("Prenom.Nom@googlemail.com"), gmail_identity("prenomnom@gmail.com"));
        assert_ne!(gmail_identity("a@gmail.com"), gmail_identity("b@gmail.com"));
        assert_eq!(gmail_identity("Jean.Dupont@Exemple.fr"), "jean.dupont@exemple.fr");
    }
}
