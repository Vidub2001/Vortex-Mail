//! Connexions `OAuth2` à refaire : quand un service refuse le jeton de
//! renouvellement (expiré, révoqué) ou qu'il manque, l'interface est prévenue
//! (événement `auth-required`) pour proposer « Reconnecter » en un clic.
//!
//! Aussi : les alertes des serveurs IMAP (`[ALERT]`, événement `server-alert`),
//! montrées une fois par compte et par session (relecture complète n° 22).
//!
//! Aussi : les déconnexions, pour qu'un renouvellement de jeton commencé avant
//! ne remette pas le jeton d'un compte déconnecté ou supprimé (relecture A8).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

static APP: OnceLock<AppHandle> = OnceLock::new();
static LAST: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
/// Connexions à refaire, lues par l'interface à son ouverture (un événement
/// envoyé avant qu'elle n'écoute serait perdu) : (service, portée, adresse).
static PENDING: Mutex<Vec<(String, &'static str, String)>> = Mutex::new(Vec::new());
/// Alertes des serveurs déjà montrées dans cette session : (adresse, texte).
static ALERTS_SHOWN: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// Alertes différentes montrées au plus par session (un serveur bavard ne
/// remplit ni la mémoire ni l'écran).
const MAX_ALERTS: usize = 50;
/// Une seule alerte par compte et par service dans ce délai.
const QUIET: Duration = Duration::from_mins(10);

/// Ce que sert le jeton refusé : le courrier (`mail`), l'agenda et les
/// contacts (`pim`), ou les deux (`all`, jeton Google). L'interface ne masque
/// les erreurs de synchronisation du courrier que pour `mail` et `all` (A3).
pub(crate) const MAIL: &str = "mail";
pub(crate) const PIM: &str = "pim";
pub(crate) const ALL: &str = "all";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthRequired {
    /// `google` ou `o365`.
    pub provider: String,
    pub user: String,
    /// `mail`, `pim` ou `all`.
    pub scope: String,
}

/// Alerte d'un serveur de courrier, pour l'interface.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServerAlert {
    /// Adresse du compte.
    pub account: String,
    /// Texte du serveur, tel quel (sans caractère de contrôle).
    pub text: String,
}

/// Montre une alerte du serveur IMAP de ce compte, une seule fois par session.
pub(crate) fn server_alert(account: &str, text: &str) {
    {
        let mut shown = lock(&ALERTS_SHOWN);
        if shown.len() >= MAX_ALERTS || shown.iter().any(|(a, t)| a == account && t == text) {
            return;
        }
        shown.push((account.to_string(), text.to_string()));
    }
    if let Some(app) = APP.get() {
        let _ = app.emit("server-alert", ServerAlert { account: account.to_string(), text: text.to_string() });
    }
}

/// Verrou utilisable après une panique : les listes qu'il protège ne changent
/// que par un ajout ou un retrait d'un seul tenant (lot 10).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Connexions à refaire en ce moment.
pub(crate) fn pending() -> Vec<AuthRequired> {
    lock(&PENDING)
        .iter()
        .map(|(provider, scope, user)| AuthRequired { provider: provider.clone(), user: user.clone(), scope: (*scope).to_string() })
        .collect()
}

pub(crate) fn set_app(app: AppHandle) {
    let _ = APP.set(app);
}

/// Signale qu'une connexion doit être refaite.
pub(crate) fn sign_in_required(provider: &str, scope: &'static str, user: &str) {
    let user = user.trim().to_lowercase();
    {
        let mut p = lock(&PENDING);
        if !p.iter().any(|(pr, sc, u)| pr == provider && *sc == scope && *u == user) {
            p.push((provider.to_string(), scope, user.clone()));
        }
    }
    let key = format!("{provider}|{scope}|{user}");
    {
        let mut guard = lock(&LAST);
        let map = guard.get_or_insert_with(HashMap::new);
        if map.get(&key).is_some_and(|t| t.elapsed() < QUIET) {
            return;
        }
        map.insert(key, Instant::now());
    }
    if let Some(app) = APP.get() {
        let _ = app.emit("auth-required", AuthRequired { provider: provider.to_string(), user, scope: scope.to_string() });
    }
}

/// Retire les connexions à refaire d'un service pour cette adresse (toutes
/// portées si `scope` est `None`) : la prochaine panne sera signalée sans attendre.
fn clear(provider: &str, scope: Option<&str>, user: &str) {
    let user = user.trim().to_lowercase();
    let matches = |pr: &str, sc: &str, u: &str| pr == provider && scope.is_none_or(|s| s == sc) && u == user;
    lock(&PENDING).retain(|(pr, sc, u)| !matches(pr, sc, u));
    if let Some(map) = lock(&LAST).as_mut() {
        map.retain(|key, _| {
            let mut parts = key.splitn(3, '|');
            let (pr, sc, u) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            !matches(pr, sc, u)
        });
    }
}

/// Connexion refaite : plus rien à refaire pour ce service et cette adresse.
pub(crate) fn signed_in(provider: &str, user: &str) {
    clear(provider, None, user);
}

/// Jeton oublié (déconnexion, compte supprimé) : plus de bandeau « Reconnecter » pour lui.
pub(crate) fn forgotten(provider: &str, scope: &str, user: &str) {
    clear(provider, Some(scope), user);
}

// ------------------------------------------------------------------ déconnexions

/// Nombre de déconnexions de chaque jeton (clé du trousseau).
static SIGN_OUTS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

fn sign_outs_of(map: Option<&HashMap<String, u64>>, key: &str) -> u64 {
    map.and_then(|m| m.get(key).copied()).unwrap_or(0)
}

/// Nombre de déconnexions de ce jeton jusqu'ici : à relever avant un renouvellement.
#[must_use]
pub(crate) fn sign_out_count(key: &str) -> u64 {
    sign_outs_of(lock(&SIGN_OUTS).as_ref(), key)
}

/// Déconnexion : `remove` retire le jeton, sans qu'un renouvellement en cours
/// puisse le réécrire juste après.
pub(crate) fn sign_out<T>(key: &str, remove: impl FnOnce() -> T) -> T {
    let mut guard = lock(&SIGN_OUTS);
    *guard.get_or_insert_with(HashMap::new).entry(key.to_string()).or_insert(0) += 1;
    remove()
}

/// Garde le résultat d'un renouvellement commencé quand `count` déconnexions
/// avaient eu lieu (`store` enregistre le nouveau jeton), sauf si le jeton a
/// été déconnecté depuis (relecture A8).
///
/// # Errors
///
/// Jeton déconnecté depuis le début du renouvellement, ou échec de `store`.
pub(crate) fn unless_signed_out(key: &str, count: u64, store: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let guard = lock(&SIGN_OUTS);
    if sign_outs_of(guard.as_ref(), key) != count {
        return Err(tr!("Compte déconnecté pendant le renouvellement de la connexion.").into());
    }
    store()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_renewal_does_not_bring_back_a_signed_out_token() {
        let key = "test-auth/a8@example.com";
        let before = sign_out_count(key);
        let mut stored = false;
        assert!(unless_signed_out(key, before, || {
            stored = true;
            Ok(())
        })
        .is_ok());
        assert!(stored);

        // Déconnexion pendant le renouvellement : le jeton n'est pas réécrit.
        let during = sign_out_count(key);
        sign_out(key, || ());
        let mut rewritten = false;
        assert!(unless_signed_out(key, during, || {
            rewritten = true;
            Ok(())
        })
        .is_err());
        assert!(!rewritten);
    }

    #[test]
    fn pending_sign_ins_keep_their_scope() {
        let user = "Test-A3@Example.com";
        sign_in_required("o365", PIM, user);
        sign_in_required("o365", MAIL, user);
        let mine = |list: Vec<AuthRequired>| -> Vec<String> {
            let mut scopes: Vec<String> = list.into_iter().filter(|p| p.user == "test-a3@example.com").map(|p| p.scope).collect();
            scopes.sort();
            scopes
        };
        assert_eq!(mine(pending()), vec!["mail", "pim"]);
        // Agenda déconnecté : seul le courrier reste à refaire.
        forgotten("o365", PIM, user);
        assert_eq!(mine(pending()), vec!["mail"]);
        // Reconnexion : plus rien.
        signed_in("o365", user);
        assert_eq!(mine(pending()), Vec::<String>::new());
    }
}
