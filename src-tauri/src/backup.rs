//! Sauvegarde complète de la configuration, pour installer Vortex Mail sur
//! une autre machine : comptes (mots de passe compris), connexions Outlook.com
//! et Google, réglages de l'interface, agenda et contacts « Sur cet
//! ordinateur ». Le fichier est chiffré par un mot de passe (`crypto.rs`).
//!
//! Le courrier n'y est pas : il se resynchronise depuis les serveurs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::cache::{Cache, PimItem};
use crate::config::{self, AccountConfig, AuthMethod};
use crate::json::{self, Json};
use crate::router::lock_cache;

pub(crate) const EXTENSION: &str = "vxbackup";

/// Entrée du trousseau sauvegardée telle quelle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Secret {
    pub key: String,
    pub label: String,
    pub value: String,
}

/// Élément « Sur cet ordinateur » (contact ou rendez-vous).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalItem {
    pub kind: String,
    pub collection: String,
    pub item: PimItem,
}

/// Contenu d'une sauvegarde. Les comptes restent en JSON (format de `accounts.json`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Snapshot {
    pub created: i64,
    pub accounts_json: String,
    pub secrets: Vec<Secret>,
    /// Réglages de l'interface (stockage local de la fenêtre) : clé → valeur.
    pub settings: Vec<(String, String)>,
    pub local: Vec<LocalItem>,
}

// ================================================================ format

pub(crate) fn to_payload(s: &Snapshot) -> Result<String, String> {
    let accounts = json::parse(&s.accounts_json).map_err(|e| tr!("comptes illisibles : {e}", e = e))?;
    let secrets: Vec<Json> = s
        .secrets
        .iter()
        .map(|x| Json::obj([("key", x.key.clone().into()), ("label", x.label.clone().into()), ("value", x.value.clone().into())]))
        .collect();
    let settings = Json::Obj(s.settings.iter().map(|(k, v)| (k.clone(), Json::Str(v.clone()))).collect());
    let local: Vec<Json> = s
        .local
        .iter()
        .map(|x| {
            Json::obj([
                ("kind", x.kind.clone().into()),
                ("collection", x.collection.clone().into()),
                ("id", x.item.id.clone().into()),
                ("url", x.item.url.clone().into()),
                ("etag", x.item.etag.clone().into()),
                ("raw", x.item.raw.clone().into()),
                ("data", x.item.data.clone().into()),
            ])
        })
        .collect();
    Ok(Json::obj([
        ("app", "Vortex Mail".into()),
        ("created", s.created.into()),
        ("accounts", accounts),
        ("secrets", secrets.into()),
        ("settings", settings),
        ("local", local.into()),
    ])
    .to_text())
}

pub(crate) fn from_payload(text: &str) -> Result<Snapshot, String> {
    let v = json::parse(text).map_err(|_| tr!("Sauvegarde illisible.").to_string())?;
    if v.get("app").text() != "Vortex Mail" {
        return Err(tr!("Sauvegarde illisible.").into());
    }
    let accounts = v.get("accounts");
    if !matches!(accounts, Json::Arr(_)) {
        return Err(tr!("Sauvegarde sans comptes.").into());
    }
    let secrets = v
        .get("secrets")
        .items()
        .iter()
        .filter(|x| !x.get("key").text().is_empty())
        .map(|x| Secret {
            key: x.get("key").text().to_string(),
            label: x.get("label").text().to_string(),
            value: x.get("value").text().to_string(),
        })
        .collect();
    let settings = match v.get("settings") {
        Json::Obj(pairs) => pairs.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect(),
        _ => Vec::new(),
    };
    let local = v
        .get("local")
        .items()
        .iter()
        .filter(|x| !x.get("id").text().is_empty())
        .map(|x| LocalItem {
            kind: x.get("kind").text().to_string(),
            collection: x.get("collection").text().to_string(),
            item: PimItem {
                id: x.get("id").text().to_string(),
                url: x.get("url").text().to_string(),
                etag: x.get("etag").text().to_string(),
                raw: x.get("raw").text().to_string(),
                data: x.get("data").text().to_string(),
            },
        })
        .collect();
    let created = v.get("created").as_i64().unwrap_or(0);
    Ok(Snapshot { created, accounts_json: accounts.to_text(), secrets, settings, local })
}

/// Réglages envoyés par l'interface : objet JSON `{ "clé": "valeur" }`.
pub(crate) fn parse_settings(text: &str) -> Vec<(String, String)> {
    match json::parse(text) {
        Ok(Json::Obj(pairs)) => pairs.into_iter().filter_map(|(k, v)| Some((k, v.as_str()?.to_string()))).collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn settings_json(settings: &[(String, String)]) -> String {
    Json::Obj(settings.iter().map(|(k, v)| (k.clone(), Json::Str(v.clone()))).collect()).to_text()
}

// ================================================================ trousseau

/// Identifiant de connexion (adresse) d'un compte.
fn login(c: &AccountConfig) -> String {
    crate::pim::google_user(c).to_string()
}

/// Entrées du trousseau propres à un compte (hors mots de passe IMAP/SMTP,
/// qui voyagent avec la configuration du compte).
fn account_entries(c: &AccountConfig) -> Vec<(String, String)> {
    let user = login(c);
    let mut out = Vec::new();
    if c.imap.auth == AuthMethod::OAuth2 {
        out.extend(crate::oauth::backup_entries(&user));
        out.extend(crate::o365::backup_entries(&user));
    }
    if crate::google::is_google_host(&c.imap.host) {
        out.extend(crate::google::backup_entries(&user));
    }
    out
}

fn read_secret(key: &str, label: &str) -> Result<Option<String>, String> {
    crate::secrets::read(key, label)
}

fn write_secret(s: &Secret) -> Result<(), String> {
    crate::secrets::write(&s.key, &s.label, &s.value)
}

/// Toutes les entrées du trousseau connues (jetons, ID d'application…),
/// sans doublon : celles de la sauvegarde.
fn known_entries(configs: &[AccountConfig]) -> Vec<(String, String)> {
    let mut wanted: Vec<(String, String)> = configs.iter().flat_map(account_entries).collect();
    wanted.push(crate::oauth::backup_global());
    wanted.push(crate::google::backup_global());
    let mut seen = std::collections::HashSet::new();
    wanted.retain(|(key, _)| seen.insert(key.clone()));
    wanted
}

/// Au démarrage : lit une fois chaque entrée connue, ce qui reprend tout de
/// suite celles des versions précédentes (service `fluxmail`), même les
/// entrées rarement lues comme l'ID d'application.
pub(crate) fn take_over_old_keyring(configs: &[AccountConfig]) {
    for (key, label) in known_entries(configs) {
        if let Err(e) = read_secret(&key, &label) {
            crate::diag::report(&e);
        }
    }
    // Jeton agenda et contacts rangé sous l'ancienne clé : repris par sa lecture.
    for c in configs.iter().filter(|c| c.imap.auth == AuthMethod::OAuth2) {
        let _ = crate::o365::is_signed_in(&login(c));
    }
}

// ================================================================ export

/// Rassemble tout ce qu'il faut sauvegarder.
pub(crate) fn collect(configs: &[AccountConfig], cache: &Arc<Mutex<Cache>>, settings: Vec<(String, String)>) -> Result<Snapshot, String> {
    let mut secrets = Vec::new();
    for (key, label) in known_entries(configs) {
        if secrets.iter().any(|s: &Secret| s.key == key) {
            continue;
        }
        if let Some(value) = read_secret(&key, &label)? {
            secrets.push(Secret { key, label, value });
        }
    }
    let local = lock_cache(cache)
        .pim_items_of(crate::pim::LOCAL)?
        .into_iter()
        .map(|(kind, collection, item)| LocalItem { kind, collection, item })
        .collect();
    let accounts_json = serde_json::to_string(configs).map_err(|e| e.to_string())?;
    Ok(Snapshot { created: now(), accounts_json, secrets, settings, local })
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Nom de fichier libre dans `dir` : « Vortex Mail - sauvegarde AAAA-MM-JJ.vxbackup ».
pub(crate) fn file_name(dir: &Path, stamp: &str) -> PathBuf {
    let date = format!("{}-{}-{}", &stamp[0..4], &stamp[4..6], &stamp[6..8]);
    let base = format!("Vortex Mail - sauvegarde {date}");
    let mut path = dir.join(format!("{base}.{EXTENSION}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} ({n}).{EXTENSION}"));
        n += 1;
    }
    path
}

/// Comptes à mot de passe dont le mot de passe n'est pas connu (trousseau
/// verrouillé au démarrage…) : la sauvegarde ne le contiendra pas, l'export le
/// signale (relecture C4).
#[must_use]
pub(crate) fn missing_passwords(configs: &[AccountConfig]) -> Vec<String> {
    configs
        .iter()
        .filter(|c| c.imap.auth != AuthMethod::OAuth2)
        .filter(|c| {
            c.imap.password.is_empty() || (!c.smtp_same_credentials && !c.smtp.username.trim().is_empty() && c.smtp.password.is_empty())
        })
        .map(|c| if c.name.trim().is_empty() { c.email.clone() } else { c.name.clone() })
        .collect()
}

/// Écrit la sauvegarde chiffrée dans `dir` ; renvoie son chemin et les comptes
/// dont le mot de passe manque (voir `missing_passwords`).
pub(crate) fn export(
    configs: &[AccountConfig],
    cache: &Arc<Mutex<Cache>>,
    settings: Vec<(String, String)>,
    password: &str,
    dir: &Path,
) -> Result<(PathBuf, Vec<String>), String> {
    let snapshot = collect(configs, cache, settings)?;
    let file = crate::crypto::seal(password, to_payload(&snapshot)?.as_bytes())?;
    std::fs::create_dir_all(dir).map_err(|e| tr!("dossier {dir} inaccessible : {e}", dir = dir.display(), e = e))?;
    let path = file_name(dir, &crate::pim::utc_stamp(snapshot.created));
    std::fs::write(&path, file).map_err(|e| tr!("écriture de {file} impossible : {e}", file = path.display(), e = e))?;
    Ok((path, missing_passwords(configs)))
}

// ================================================================ import

/// Ce que contient une sauvegarde, avant de l'appliquer.
#[derive(Debug, Clone)]
pub(crate) struct Preview {
    pub accounts: Vec<AccountConfig>,
    pub snapshot: Snapshot,
}

/// Déchiffre et lit une sauvegarde (rien n'est encore modifié).
pub(crate) fn read(content: &str, password: &str) -> Result<Preview, String> {
    let plain = crate::crypto::open(password, content)?;
    let text = String::from_utf8(plain).map_err(|_| tr!("Sauvegarde illisible.").to_string())?;
    let snapshot = from_payload(&text)?;
    let accounts: Vec<AccountConfig> = serde_json::from_str(&snapshot.accounts_json).map_err(|e| tr!("comptes illisibles : {e}", e = e))?;
    // Contrôle minimal : un mot de passe manquant se ressaisit ensuite dans Paramètres.
    if accounts.iter().any(|c| c.id.trim().is_empty() || !c.email.contains('@')) {
        return Err(tr!("Sauvegarde illisible : compte incomplet.").into());
    }
    let snapshot = screen(&accounts, snapshot);
    Ok(Preview { accounts, snapshot })
}

/// Ne garde d'une sauvegarde que ce que l'application y met elle-même : les
/// entrées du trousseau de ses comptes (avec leur libellé habituel) et les
/// réglages `vortex.*`. Une sauvegarde fabriquée n'écrit aucune autre entrée.
fn screen(accounts: &[AccountConfig], mut snapshot: Snapshot) -> Snapshot {
    let known = known_entries(accounts);
    snapshot.secrets = snapshot
        .secrets
        .into_iter()
        .filter_map(|s| {
            known.iter().find(|(key, _)| *key == s.key).map(|(key, label)| Secret {
                key: key.clone(),
                label: label.clone(),
                value: s.value,
            })
        })
        .collect();
    snapshot.settings.retain(|(key, _)| key.starts_with("vortex."));
    snapshot
}

/// Remplace la configuration actuelle par celle de la sauvegarde. L'application
/// doit ensuite redémarrer (comptes et connexions sont chargés au démarrage).
///
/// Ce qui efface (comptes absents de la sauvegarde) passe en dernier, une fois
/// la nouvelle configuration enregistrée : un import interrompu en cours de
/// route ne laisse pas de compte sans mot de passe ni d'agenda vidé (C2).
pub(crate) fn apply(preview: &Preview, current: &[AccountConfig], config_file: &Path, cache: &Arc<Mutex<Cache>>) -> Result<(), String> {
    // 1. Mots de passe des comptes, puis connexions OAuth et applications. Un
    //    mot de passe absent de la sauvegarde n'efface pas celui déjà enregistré (C4).
    for c in &preview.accounts {
        crate::secrets::store_account_keeping(c, current.iter().find(|o| o.id == c.id))?;
    }
    for s in &preview.snapshot.secrets {
        write_secret(s)?;
    }
    // 2. Agenda et contacts « Sur cet ordinateur », remplacés d'un seul bloc.
    let local: Vec<(String, String, PimItem)> =
        preview.snapshot.local.iter().map(|x| (x.kind.clone(), x.collection.clone(), x.item.clone())).collect();
    lock_cache(cache).pim_replace_account(crate::pim::LOCAL, &local)?;
    // 3. Fichier des comptes : la nouvelle configuration est complète.
    config::save(config_file, &preview.accounts)?;
    // 4. Comptes actuels absents de la sauvegarde : secrets et cache retirés.
    //    Les jetons OAuth sont rangés par adresse : ceux d'une adresse reprise
    //    par un compte de la sauvegarde (même boîte, autre identifiant), écrits
    //    à l'étape 1, sont gardés (relecture complète n° 19).
    for old in current.iter().filter(|o| !preview.accounts.iter().any(|n| n.id == o.id)) {
        let user = login(old);
        let _ = crate::secrets::delete_account(&old.id);
        if !preview.accounts.iter().any(|n| login(n).eq_ignore_ascii_case(&user)) {
            let _ = crate::oauth::forget(&user);
            let _ = crate::o365::forget(&user);
            let _ = crate::google::forget(&user);
        }
        if let Err(e) = lock_cache(cache).delete_account(&old.id) {
            crate::diag::report(&e);
        }
        let _ = crate::pim::forget_account(cache, &old.id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_backup_writes_only_known_entries() {
        let mut s = sample();
        let (key, label) = crate::oauth::backup_global();
        s.secrets = vec![
            Secret { key: key.clone(), label: "autre libellé".into(), value: "id".into() },
            Secret { key: "ssh/github".into(), label: "x".into(), value: "piège".into() },
        ];
        s.settings.push(("autre.cle".into(), "x".into()));
        let kept = screen(&[], s);
        assert_eq!(kept.secrets, vec![Secret { key, label, value: "id".into() }]);
        assert_eq!(kept.settings.len(), 2);
        assert!(kept.settings.iter().all(|(k, _)| k.starts_with("vortex.")));
    }

    fn sample() -> Snapshot {
        Snapshot {
            created: 1_793_797_000,
            accounts_json: r#"[{"id":"imap1","name":"Gmail 1","imap":{"password":"s3cret"}}]"#.into(),
            secrets: vec![Secret { key: "oauth/client-id".into(), label: "Vortex Mail — ID".into(), value: "1234-abcd".into() }],
            settings: vec![("vortex.theme".into(), "dark".into()), ("vortex.mail".into(), r#"{"syncMinutes":5}"#.into())],
            local: vec![LocalItem {
                kind: "contacts".into(),
                collection: "local:contacts".into(),
                item: PimItem {
                    id: "local|abc".into(),
                    url: String::new(),
                    etag: String::new(),
                    raw: "BEGIN:VCARD\r\nFN:Zoé\r\nEND:VCARD\r\n".into(),
                    data: r#"{"name":"Zoé"}"#.into(),
                },
            }],
        }
    }

    #[test]
    fn payload_round_trip() {
        let s = sample();
        let text = to_payload(&s).unwrap();
        let back = from_payload(&text).unwrap();
        assert_eq!(back.created, s.created);
        assert_eq!(back.secrets, s.secrets);
        assert_eq!(back.settings, s.settings);
        assert_eq!(back.local, s.local);
        assert_eq!(json::parse(&back.accounts_json).unwrap(), json::parse(&s.accounts_json).unwrap());
        assert!(from_payload(r#"{"app":"Autre"}"#).is_err());
    }

    #[test]
    fn sealed_backup_opens_with_the_password() {
        let text = to_payload(&sample()).unwrap();
        let file = crate::crypto::seal_with("mot de passe solide", text.as_bytes(), 1000).unwrap();
        assert!(!file.contains("s3cret"));
        let plain = crate::crypto::open("mot de passe solide", &file).unwrap();
        assert_eq!(from_payload(&String::from_utf8(plain).unwrap()).unwrap().settings, sample().settings);
    }

    #[test]
    fn settings_from_the_interface() {
        let pairs = parse_settings(r#"{"vortex.theme":"light","vortex.unifiedView":"true","bad":3}"#);
        assert_eq!(pairs, vec![("vortex.theme".into(), "light".into()), ("vortex.unifiedView".into(), "true".into())]);
        assert_eq!(parse_settings("pas du json"), Vec::new());
        assert_eq!(settings_json(&pairs), r#"{"vortex.theme":"light","vortex.unifiedView":"true"}"#);
    }

    #[test]
    fn export_reports_missing_passwords() {
        let accounts: Vec<AccountConfig> = serde_json::from_str(
            r#"[{"id":"a","name":"Mailo","email":"a@mailo.com","color":"",
                 "imap":{"host":"h","port":993,"security":"sslTls","auth":"normal","username":"a","password":"s3cret"},
                 "smtp":{"host":"h","port":465,"security":"sslTls","auth":"normal","username":"a"}},
                {"id":"b","name":"Pro","email":"b@x.fr","color":"",
                 "imap":{"host":"h","port":993,"security":"sslTls","auth":"normal","username":"b"},
                 "smtp":{"host":"h","port":465,"security":"sslTls","auth":"normal","username":"b"}}]"#,
        )
        .unwrap();
        assert_eq!(missing_passwords(&accounts), vec!["Pro".to_string()]);
    }

    #[test]
    fn file_names_do_not_overwrite() {
        let dir = std::env::temp_dir().join(format!("vx-backup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = file_name(&dir, "20261104T130000Z");
        assert_eq!(first.file_name().unwrap(), "Vortex Mail - sauvegarde 2026-11-04.vxbackup");
        std::fs::write(&first, b"x").unwrap();
        assert_eq!(file_name(&dir, "20261104T130000Z").file_name().unwrap(), "Vortex Mail - sauvegarde 2026-11-04 (2).vxbackup");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
