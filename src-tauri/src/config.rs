//! Comptes configurés, enregistrés dans `~/.config/fr.vortexmail.client/accounts.json`.
//!
//! Le fichier ne contient aucun mot de passe : ils sont dans le trousseau du
//! système (voir `secrets.rs`). Les anciens fichiers qui en contiennent encore
//! sont migrés au démarrage.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Type de connexion chiffrée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Security {
    /// SSL/TLS si port 993 (IMAP) ou 465 (SMTP), sinon STARTTLS.
    Auto,
    /// TLS dès la connexion (IMAP 993, SMTP 465).
    SslTls,
    /// Connexion en clair puis passage en TLS (IMAP 143, SMTP 587).
    StartTls,
    /// Aucun chiffrement : mot de passe en clair sur le réseau.
    #[serde(rename = "none")]
    Unencrypted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthMethod {
    /// « Mot de passe normal » : IMAP LOGIN, SMTP AUTH PLAIN ou LOGIN.
    Normal,
    /// SASL PLAIN explicite (IMAP AUTHENTICATE PLAIN).
    Plain,
    /// `OAuth2` (XOAUTH2) : connexion Outlook.com, sans mot de passe enregistré.
    #[serde(rename = "oauth2")]
    OAuth2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub auth: AuthMethod,
    pub username: String,
    /// En mémoire seulement ; jamais écrit dans le fichier.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// Empreinte SHA-256 du certificat attendu (`AA:BB:…`), pour Proton Mail Bridge
    /// sur cet ordinateur seulement (lot I3). Vide : vérification habituelle du
    /// certificat et du nom par la bibliothèque TLS du système.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pinned_cert: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountConfig {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub email: String,
    /// Nom affiché aux destinataires (« De : »). Vide : l'adresse seule.
    #[serde(default)]
    pub display_name: String,
    pub color: String,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
    #[serde(default = "default_true")]
    pub smtp_same_credentials: bool,
    /// ID d'application Outlook.com (comptes `OAuth2`) ; pas un secret.
    #[serde(default)]
    pub oauth_client_id: String,
    /// Agenda (`CalDAV`), et contacts si `card_url` est vide : adresse du serveur. Vide :
    /// recherche automatique d'après le domaine de l'adresse ; `-` : désactivé.
    #[serde(default)]
    pub dav_url: String,
    /// Serveur des contacts (`CardDAV`) quand il diffère de celui de l'agenda
    /// (Mailo…). Vide : le même que `dav_url` ; `-` : contacts désactivés.
    #[serde(default)]
    pub card_url: String,
}

fn default_true() -> bool {
    true
}

impl AccountConfig {
    /// Contrôles de base avant toute tentative de connexion.
    pub fn validate(&self) -> Result<(), String> {
        let check = |label: &str, s: &ServerConfig| -> Result<(), String> {
            if s.host.trim().is_empty() {
                return Err(tr!("{label} : le nom du serveur est vide", label = label));
            }
            if s.port == 0 {
                return Err(tr!("{label} : port invalide", label = label));
            }
            if s.username.trim().is_empty() {
                return Err(tr!("{label} : identifiant requis", label = label));
            }
            if s.auth != AuthMethod::OAuth2 && s.password.is_empty() {
                return Err(tr!("{label} : identifiant et mot de passe requis", label = label));
            }
            if !s.pinned_cert.is_empty() {
                // Certificat épinglé : jamais une exception TLS générale (lot I3).
                if !crate::transport::is_loopback(s.host.trim()) {
                    return Err(tr!(
                        "{label} : un certificat épinglé n'est admis que pour Proton Mail Bridge, sur cet ordinateur (127.0.0.1).",
                        label = label
                    ));
                }
                if !crate::transport::is_fingerprint(&s.pinned_cert) {
                    return Err(tr!("{label} : empreinte de certificat invalide", label = label));
                }
                if matches!(s.security, Security::Auto | Security::Unencrypted) {
                    return Err(tr!("{label} : choisissez SSL/TLS ou STARTTLS pour un certificat épinglé", label = label));
                }
            }
            Ok(())
        };
        if self.name.trim().is_empty() || !self.email.contains('@') {
            return Err(tr!("Nom du compte et adresse e-mail requis").into());
        }
        check(tr!("Serveur entrant (IMAP)"), &self.imap)?;
        // Avec « mêmes identifiants », ce sont ceux de l'IMAP qui comptent.
        check(tr!("Serveur sortant (SMTP)"), &self.effective_smtp())
    }

    /// Identifiants SMTP effectifs (copiés de l'IMAP si demandé).
    pub fn effective_smtp(&self) -> ServerConfig {
        let mut smtp = self.smtp.clone();
        if self.smtp_same_credentials {
            smtp.username.clone_from(&self.imap.username);
            smtp.password.clone_from(&self.imap.password);
            // La connexion Outlook.com vaut pour l'IMAP comme pour le SMTP.
            if self.imap.auth == AuthMethod::OAuth2 {
                smtp.auth = AuthMethod::OAuth2;
            }
        }
        smtp
    }
}

pub(crate) fn load(path: &Path) -> Result<Vec<AccountConfig>, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| tr!("{file} illisible : {e}", file = path.display(), e = e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(tr!("lecture de {file} impossible : {e}", file = path.display(), e = e)),
    }
}

pub(crate) fn save(path: &Path, accounts: &[AccountConfig]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| tr!("création de {dir} impossible : {e}", dir = dir.display(), e = e))?;
    }
    // Aucun mot de passe sur le disque : ils sont dans le trousseau.
    let without_passwords: Vec<AccountConfig> = accounts
        .iter()
        .cloned()
        .map(|mut c| {
            c.imap.password.clear();
            c.smtp.password.clear();
            // L'ID d'application Outlook.com est rangé dans le trousseau (oauth.rs).
            c.oauth_client_id.clear();
            c
        })
        .collect();
    let json = serde_json::to_string_pretty(&without_passwords).map_err(|e| e.to_string())?;
    // Écriture atomique : fichier temporaire puis renommage.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| tr!("écriture de {file} impossible : {e}", file = tmp.display(), e = e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, path).map_err(|e| tr!("enregistrement de {file} impossible : {e}", file = path.display(), e = e))
}

/// Même serveur et même identifiant : un mot de passe enregistré peut resservir
/// (hôte, port, identifiant et certificat épinglé inchangés). Un autre certificat
/// de Proton Mail Bridge peut être celui d'un autre programme à l'écoute sur le
/// même port (lot I3) : le mot de passe de Bridge est alors redemandé.
pub(crate) fn same_server(a: &ServerConfig, b: &ServerConfig) -> bool {
    a.host.trim().eq_ignore_ascii_case(b.host.trim())
        && a.port == b.port
        && a.username == b.username
        && a.pinned_cert.trim().eq_ignore_ascii_case(b.pinned_cert.trim())
}
