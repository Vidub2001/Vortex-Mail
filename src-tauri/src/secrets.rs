//! Mots de passe dans le trousseau du système (Secret Service : GNOME Keyring,
//! `KWallet`). Le fichier `accounts.json` n'en contient plus aucun.
//!
//! Entrées : service `vortexmail`, identifiant `<compte>/imap` ou `<compte>/smtp`.
//! Les entrées des versions précédentes (service `fluxmail`) sont reprises
//! à la première lecture, puis retirées.
//! Elles sont visibles dans « Mots de passe et clés » (Seahorse), sous un
//! libellé lisible : « Vortex Mail — adresse (mot de passe IMAP) ».

use std::path::Path;

use crate::config::{self, AccountConfig};

const SERVICE: &str = "vortexmail";
/// Service des versions précédentes (nom de projet « `FluxMail` »).
const OLD_SERVICE: &str = "fluxmail";

/// Entrée du trousseau créée avec un libellé lisible (affiché par Seahorse).
/// Mêmes attributs qu'une entrée ordinaire : lecture et suppression inchangées.
fn labeled_entry(service: &str, user: &str, label: &str) -> Result<keyring::Entry, String> {
    #[cfg(target_os = "linux")]
    {
        let mut credential = keyring::secret_service::SsCredential::new_with_target(None, service, user)
            .map_err(|e| tr!("trousseau inaccessible : {e}", e = e))?;
        credential.label = label.to_string();
        Ok(keyring::Entry::new_with_credential(Box::new(credential)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = label;
        keyring::Entry::new(service, user).map_err(|e| tr!("trousseau inaccessible : {e}", e = e))
    }
}

// ------------------------------------------------------------------ lecture et écriture

/// Taille maximale d'une valeur dans une seule entrée. Le Gestionnaire
/// d'identification de Windows limite chaque secret à 2 560 octets (UTF-16) :
/// les jetons OAuth plus longs sont répartis sur plusieurs entrées.
#[cfg(windows)]
const PIECE: usize = 1000;
#[cfg(not(windows))]
const PIECE: usize = usize::MAX;

/// Valeur de l'entrée principale d'un secret découpé : « vortex-pieces:<génération>:<n> ».
/// Les versions précédentes écrivaient « vortex-pieces:<n> », sans génération.
const PIECES: &str = "vortex-pieces:";

/// Clé d'un morceau : `clé#<génération>.<i>`, ou `clé#<i>` pour l'ancien format (génération 0).
fn piece_key(key: &str, generation: u64, i: usize) -> String {
    if generation == 0 {
        format!("{key}#{i}")
    } else {
        format!("{key}#{generation}.{i}")
    }
}

/// Génération et nombre de morceaux annoncés par l'entrée principale.
fn pieces_of(value: &str) -> Option<(u64, usize)> {
    let rest = value.strip_prefix(PIECES)?.trim();
    match rest.split_once(':') {
        Some((generation, n)) => Some((generation.parse().ok()?, n.parse().ok()?)),
        None => Some((0, rest.parse().ok()?)),
    }
}

/// Une seule écriture ou suppression de secret à la fois : deux renouvellements
/// de jeton simultanés ne mélangent pas leurs morceaux (relecture C1).
static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Le verrou ne protège aucune donnée : il ne fait qu'ordonner les écritures.
/// Après une panique, il reste donc utilisable sans risque.
fn writing() -> std::sync::MutexGuard<'static, ()> {
    WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Stockage des secrets : le trousseau du système (ou une table en mémoire pour les tests).
trait Store {
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    fn del(&self, key: &str) -> Result<(), String>;
}

/// Trousseau du système, avec le libellé lisible des nouvelles entrées.
struct Keyring<'a> {
    service: &'a str,
    label: &'a str,
}

impl Store for Keyring<'_> {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        match labeled_entry(self.service, key, self.label)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(tr!("lecture du trousseau impossible : {e}", e = e)),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        labeled_entry(self.service, key, self.label)?
            .set_password(value)
            .map_err(|e| tr!("enregistrement dans le trousseau impossible : {e}", e = e))
    }

    fn del(&self, key: &str) -> Result<(), String> {
        match labeled_entry(self.service, key, self.label)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(tr!("suppression dans le trousseau impossible : {e}", e = e)),
        }
    }
}

fn read_once(store: &dyn Store, key: &str) -> Result<Option<String>, String> {
    let Some(main) = store.get(key)? else { return Ok(None) };
    let value = match pieces_of(&main) {
        Some((generation, n)) => {
            let mut whole = String::new();
            for i in 1..=n {
                whole.push_str(&store.get(&piece_key(key, generation, i))?.ok_or(tr!("secret incomplet dans le trousseau"))?);
            }
            whole
        }
        None => main,
    };
    Ok(Some(value).filter(|v| !v.trim().is_empty()))
}

fn read_in(store: &dyn Store, key: &str) -> Result<Option<String>, String> {
    // Second essai : une écriture qui vient de basculer sur une nouvelle
    // génération a pu retirer les morceaux annoncés par l'entrée lue avant.
    read_once(store, key).or_else(|_| read_once(store, key))
}

fn write_in(store: &dyn Store, key: &str, value: &str, piece: usize) -> Result<(), String> {
    let _guard = writing();
    let previous = store.get(key)?.as_deref().and_then(pieces_of);
    let chars: Vec<char> = value.chars().collect();
    let parts: Vec<String> = if chars.len() <= piece { Vec::new() } else { chars.chunks(piece).map(|c| c.iter().collect()).collect() };
    // Nouvelle génération de morceaux, écrite à côté de l'ancienne ; l'entrée
    // principale ne bascule qu'une fois tous les morceaux écrits, et l'ancienne
    // génération n'est retirée qu'après. Une écriture interrompue laisse
    // l'ancien secret entier, jamais un mélange des deux (relecture C1).
    let generation = previous.map_or(0, |(g, _)| g).saturating_add(1);
    for (i, part) in parts.iter().enumerate() {
        store.set(&piece_key(key, generation, i + 1), part)?;
    }
    if parts.is_empty() {
        store.set(key, value)?;
    } else {
        store.set(key, &format!("{PIECES}{generation}:{}", parts.len()))?;
    }
    // Le nouveau secret est en place : un ancien morceau non retiré n'est
    // qu'un reste, signalé sans faire échouer l'enregistrement.
    if let Some((old_generation, old_count)) = previous {
        for i in 1..=old_count {
            if let Err(e) = store.del(&piece_key(key, old_generation, i)) {
                crate::diag::report(&e);
            }
        }
    }
    Ok(())
}

fn remove_in(store: &dyn Store, key: &str) -> Result<(), String> {
    let _guard = writing();
    if let Some((generation, count)) = store.get(key)?.as_deref().and_then(pieces_of) {
        for i in 1..=count {
            store.del(&piece_key(key, generation, i))?;
        }
    }
    store.del(key)
}

/// Nature d'une entrée du trousseau pour le journal (`imap`, `oauth`,
/// `google`…), sans l'adresse du compte que contient sa clé (relecture
/// complète n° 52).
fn entry_kind(key: &str) -> &str {
    key.rsplit('/').find(|part| !part.contains('@')).unwrap_or("?")
}

/// Lecture avec reprise de l'ancien service : un secret trouvé seulement dans
/// `old` est recopié dans `new`, puis retiré de `old` une fois la copie réussie.
fn read_migrating(new: &dyn Store, old: &dyn Store, key: &str, piece: usize) -> Result<Option<String>, String> {
    if let Some(v) = read_in(new, key)? {
        return Ok(Some(v));
    }
    let previous = match read_in(old, key) {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(None),
        Err(e) => {
            crate::diag::report(&tr!("ancienne entrée du trousseau illisible ({kind}) : {e}", kind = entry_kind(key), e = e));
            return Ok(None);
        }
    };
    match write_in(new, key, &previous, piece) {
        Ok(()) => {
            if let Err(e) = remove_in(old, key) {
                crate::diag::report(&tr!("ancienne entrée du trousseau non supprimée ({kind}) : {e}", kind = entry_kind(key), e = e));
            }
        }
        // Copie impossible : l'ancienne entrée reste, et sert encore cette fois.
        Err(e) => crate::diag::report(&tr!("reprise de l'entrée du trousseau impossible ({kind}) : {e}", kind = entry_kind(key), e = e)),
    }
    Ok(Some(previous))
}

/// Lit un secret (réassemblé s'il a été découpé) ; `None` s'il est absent ou vide.
pub(crate) fn read(key: &str, label: &str) -> Result<Option<String>, String> {
    if crate::portable::is_portable() {
        return read_in(&Vault, key);
    }
    read_migrating(&Keyring { service: SERVICE, label }, &Keyring { service: OLD_SERVICE, label }, key, PIECE)
}

/// Enregistre un secret, découpé si le trousseau du système l'exige.
pub(crate) fn write(key: &str, label: &str, value: &str) -> Result<(), String> {
    if crate::portable::is_portable() {
        return write_in(&Vault, key, value, usize::MAX);
    }
    write_in(&Keyring { service: SERVICE, label }, key, value, PIECE)
}

/// Supprime un secret (et ses morceaux), ancien service compris.
pub(crate) fn remove(key: &str, label: &str) -> Result<(), String> {
    if crate::portable::is_portable() {
        return remove_in(&Vault, key);
    }
    if let Err(e) = remove_in(&Keyring { service: OLD_SERVICE, label }, key) {
        crate::diag::report(&e);
    }
    remove_in(&Keyring { service: SERVICE, label }, key)
}

// ------------------------------------------------------------------ coffre du mode portable

/// Secrets du mode portable : un fichier chiffré par le mot de passe maître
/// (même format que les sauvegardes), ouvert en mémoire après déverrouillage.
struct VaultState {
    file: std::path::PathBuf,
    password: String,
    entries: std::collections::BTreeMap<String, String>,
}

static VAULT: std::sync::Mutex<Option<VaultState>> = std::sync::Mutex::new(None);

fn vault_file() -> Option<std::path::PathBuf> {
    crate::portable::data_dir().map(|d| d.join("secrets.vault"))
}

/// Coffre déjà créé (premier lancement : non) ?
pub(crate) fn vault_exists() -> bool {
    vault_file().is_some_and(|f| f.is_file())
}

pub(crate) fn vault_unlocked() -> bool {
    VAULT.lock().is_ok_and(|v| v.is_some())
}

type Entries = std::collections::BTreeMap<String, String>;

/// Copie de secours du coffre : sa version précédente (relecture C3).
fn vault_backup(file: &Path) -> std::path::PathBuf {
    file.with_extension("vault.bak")
}

/// Écrit un fichier et attend qu'il soit réellement sur le disque (clé USB comprise).
fn write_synced(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(data)?;
    f.sync_all()
}

fn save_vault(state: &VaultState) -> Result<(), String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let json = serde_json::to_string(&state.entries).map_err(|e| e.to_string())?;
    let sealed = crate::crypto::seal(&state.password, json.as_bytes())?;
    if let Some(dir) = state.file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| tr!("création de {path} impossible : {e}", path = dir.display(), e = e))?;
    }
    // Fichier temporaire propre à cette écriture, écrit jusqu'au disque, puis
    // renommé : jamais de coffre à moitié écrit, même si la clé USB est
    // retirée brutalement (relecture C3).
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = state.file.with_extension(format!("vault.{}-{n}.tmp", std::process::id()));
    if let Err(e) = write_synced(&tmp, sealed.as_bytes()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(tr!("écriture de {path} impossible : {e}", path = tmp.display(), e = e));
    }
    // La version précédente reste en copie de secours.
    if state.file.is_file() {
        let backup = vault_backup(&state.file);
        let copied = std::fs::copy(&state.file, &backup)
            .and_then(|_| std::fs::OpenOptions::new().write(true).open(&backup))
            .and_then(|f| f.sync_all());
        if let Err(e) = copied {
            crate::diag::report(&tr!("copie de secours du coffre impossible : {e}", e = e));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &state.file) {
        let _ = std::fs::remove_file(&tmp);
        return Err(tr!("écriture de {path} impossible : {e}", path = state.file.display(), e = e));
    }
    // Le renommage lui-même, écrit sur le disque.
    #[cfg(unix)]
    if let Some(dir) = state.file.parent() {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Pourquoi un fichier de coffre ne s'ouvre pas.
#[derive(Debug, PartialEq, Eq)]
enum VaultError {
    /// Fichier abîmé (vide, tronqué, rempli de zéros…).
    Damaged,
    /// Fichier intact, mais le mot de passe ne l'ouvre pas.
    WrongPassword,
    Other(String),
}

impl VaultError {
    fn message(self) -> String {
        match self {
            VaultError::Damaged => tr!("Le coffre des mots de passe (secrets.vault) est abîmé, et sa copie de secours aussi. \
                 Les mots de passe des comptes sont à ressaisir.")
            .to_string(),
            VaultError::WrongPassword => tr!("Mot de passe maître incorrect.").to_string(),
            VaultError::Other(message) => message,
        }
    }
}

/// Lit un fichier de coffre.
fn read_vault(path: &Path, password: &str) -> Result<Entries, VaultError> {
    let bytes =
        std::fs::read(path).map_err(|e| VaultError::Other(tr!("lecture de {path} impossible : {e}", path = path.display(), e = e)))?;
    let sealed = String::from_utf8(bytes).map_err(|_| VaultError::Damaged)?;
    let plain = match crate::crypto::open_sealed(password, &sealed) {
        Ok(plain) => plain,
        Err(crate::crypto::OpenError::WrongPassword) => return Err(VaultError::WrongPassword),
        Err(crate::crypto::OpenError::NotSealed) => return Err(VaultError::Damaged),
        Err(e @ crate::crypto::OpenError::NewerVersion) => return Err(VaultError::Other(e.message())),
    };
    serde_json::from_slice(&plain).map_err(|_| VaultError::Damaged)
}

/// Ouvre le coffre ; s'il est abîmé ou absent, sa copie de secours, qui est
/// remise en place (relecture C3). « Mot de passe incorrect » seulement quand
/// un fichier intact refuse le mot de passe.
fn open_vault(file: &Path, password: &str) -> Result<Entries, VaultError> {
    let main = file.is_file().then(|| read_vault(file, password));
    let main = match main {
        Some(Ok(entries)) => return Ok(entries),
        // Coffre intact qui refuse le mot de passe : la copie de secours n'est
        // pas essayée, elle peut encore porter l'ancien mot de passe maître
        // (relecture du 10 novembre 2026).
        Some(Err(VaultError::WrongPassword)) => return Err(VaultError::WrongPassword),
        other => other,
    };
    let backup = vault_backup(file);
    let spare = backup.is_file().then(|| read_vault(&backup, password));
    if let Some(Ok(entries)) = spare {
        crate::diag::report(&tr!("coffre des mots de passe abîmé ou absent, copie de secours reprise"));
        if let Err(e) = std::fs::copy(&backup, file) {
            crate::diag::report(&e);
        }
        return Ok(entries);
    }
    let refused = |r: &Option<Result<Entries, VaultError>>| matches!(r, Some(Err(VaultError::WrongPassword)));
    if refused(&main) || refused(&spare) {
        return Err(VaultError::WrongPassword);
    }
    match main {
        Some(Err(VaultError::Other(message))) => Err(VaultError::Other(message)),
        _ => Err(VaultError::Damaged),
    }
}

/// Ouvre le coffre avec le mot de passe maître, ou le crée au premier lancement.
/// `true` si cet appel l'a ouvert, `false` s'il l'était déjà : deux demandes
/// simultanées (fenêtre rechargée pendant le calcul) ne chargent les comptes
/// qu'une fois (relecture C5).
///
/// # Errors
///
/// Pas de mode portable, mot de passe incorrect, coffre et copie abîmés, ou écriture impossible.
pub(crate) fn unlock_vault(password: &str) -> Result<bool, String> {
    let file = vault_file().ok_or(tr!("Vortex Mail n'est pas en mode portable."))?;
    // Verrou tenu pendant tout le déverrouillage : une seconde demande attend la première.
    let mut guard = VAULT.lock().map_err(|_| tr!("état interne verrouillé après une erreur"))?;
    if let Some(state) = guard.as_ref() {
        return if state.password == password { Ok(false) } else { Err(tr!("Mot de passe maître incorrect.").into()) };
    }
    let existing = file.is_file() || vault_backup(&file).is_file();
    let entries = if existing { open_vault(&file, password).map_err(VaultError::message)? } else { Entries::new() };
    // Coffre chiffré avec moins d'itérations qu'aujourd'hui (200 000 avant
    // le lot F5) : réécrit tout de suite avec le réglage actuel (relecture
    // complète n° 26). Échec : le coffre reste lisible, essai au prochain déverrouillage.
    let outdated = existing
        && std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| crate::crypto::sealed_iterations(&text))
            .is_some_and(|n| n < crate::crypto::ITERATIONS);
    let state = VaultState { file, password: password.to_string(), entries };
    if !existing {
        save_vault(&state)?;
    } else if outdated {
        if let Err(e) = save_vault(&state) {
            crate::diag::report(&e);
        }
    }
    *guard = Some(state);
    Ok(true)
}

/// Change le mot de passe maître (le coffre est rechiffré).
pub(crate) fn change_vault_password(old: &str, new: &str) -> Result<(), String> {
    let mut guard = VAULT.lock().map_err(|_| tr!("état interne verrouillé après une erreur"))?;
    let state = guard.as_mut().ok_or(tr!("Vortex Mail est verrouillé : saisissez le mot de passe maître."))?;
    if state.password != old {
        return Err(tr!("Mot de passe maître actuel incorrect.").into());
    }
    let previous = std::mem::replace(&mut state.password, new.to_string());
    if let Err(e) = save_vault(state) {
        state.password = previous;
        return Err(e);
    }
    // La copie de secours portait l'ancien mot de passe : elle est remplacée
    // par le coffre que le nouveau protège.
    let backup = vault_backup(&state.file);
    if let Err(e) = std::fs::copy(&state.file, &backup).and_then(|_| std::fs::OpenOptions::new().write(true).open(&backup)?.sync_all()) {
        let _ = std::fs::remove_file(&backup);
        crate::diag::report(&tr!("copie de secours du coffre impossible : {e}", e = e));
    }
    Ok(())
}

/// Le coffre, vu comme un stockage de secrets.
struct Vault;

impl Vault {
    fn with<T>(f: impl FnOnce(&mut VaultState) -> Result<T, String>) -> Result<T, String> {
        let mut guard = VAULT.lock().map_err(|_| tr!("état interne verrouillé après une erreur"))?;
        let state = guard.as_mut().ok_or(tr!("Vortex Mail est verrouillé : saisissez le mot de passe maître."))?;
        f(state)
    }
}

impl Store for Vault {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Vault::with(|s| Ok(s.entries.get(key).cloned()))
    }
    // Écriture du fichier refusée (clé USB pleine ou retirée) : la table en
    // mémoire est remise comme avant, pour qu'une nouvelle tentative écrive
    // vraiment (relecture complète n° 39 ; règle des verrous du lot 10 : pas de
    // modification à moitié faite sous `VAULT`).
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        Vault::with(|s| {
            if s.entries.get(key).map(String::as_str) == Some(value) {
                return Ok(());
            }
            let previous = s.entries.insert(key.to_string(), value.to_string());
            save_vault(s).inspect_err(|_| match previous {
                Some(old) => {
                    s.entries.insert(key.to_string(), old);
                }
                None => {
                    s.entries.remove(key);
                }
            })
        })
    }
    fn del(&self, key: &str) -> Result<(), String> {
        Vault::with(|s| {
            let Some(old) = s.entries.remove(key) else { return Ok(()) };
            save_vault(s).inspect_err(|_| {
                s.entries.insert(key.to_string(), old);
            })
        })
    }
}

// ------------------------------------------------------------------ comptes

fn label(email: &str, kind: &str) -> String {
    if kind == "smtp" {
        tr!("Vortex Mail — {email} (mot de passe SMTP)", email = email)
    } else {
        tr!("Vortex Mail — {email} (mot de passe IMAP)", email = email)
    }
}

fn set(c: &AccountConfig, kind: &str, password: &str) -> Result<(), String> {
    write(&format!("{}/{kind}", c.id), &label(&c.email, kind), password)
}

/// Donne un libellé lisible à une entrée déjà présente : le libellé n'est fixé
/// qu'à la création, l'entrée est donc recréée. En cas d'échec, l'ancienne est remise.
pub(crate) fn relabel(user: &str, new_label: &str) -> Result<(), String> {
    if crate::portable::is_portable() {
        return Ok(()); // coffre portable : pas de libellés
    }
    let plain = keyring::Entry::new(SERVICE, user).map_err(|e| tr!("trousseau inaccessible : {e}", e = e))?;
    let secret = match plain.get_password() {
        Ok(secret) => secret,
        Err(keyring::Error::NoEntry) => return Ok(()),
        Err(e) => return Err(tr!("lecture du trousseau impossible : {e}", e = e)),
    };
    plain.delete_credential().map_err(|e| tr!("trousseau : {e}", e = e))?;
    if let Err(e) = labeled_entry(SERVICE, user, new_label)?.set_password(&secret) {
        let _ = plain.set_password(&secret);
        return Err(tr!("trousseau : {e}", e = e));
    }
    Ok(())
}

/// Une seule fois : libellés lisibles pour les entrées créées par les versions précédentes.
fn relabel_once(path: &Path, accounts: &[AccountConfig]) {
    // « -2 » : libellés « Outlook.com » des connexions OAuth2 (refaits une fois).
    let marker = path.with_file_name("keyring-labels-2");
    if marker.exists() {
        return;
    }
    let mut ok = true;
    for c in accounts {
        for kind in ["imap", "smtp"] {
            if let Err(e) = relabel(&format!("{}/{kind}", c.id), &label(&c.email, kind)) {
                crate::diag::report(&e);
                ok = false;
            }
        }
        if c.imap.auth == crate::config::AuthMethod::OAuth2 {
            if let Err(e) = crate::oauth::relabel(&c.imap.username) {
                crate::diag::report(&e);
                ok = false;
            }
        }
    }
    if let Err(e) = crate::oauth::relabel_client_id() {
        crate::diag::report(&e);
        ok = false;
    }
    if ok {
        let _ = std::fs::write(&marker, b"ok\n");
    }
}

fn get(account_id: &str, kind: &str) -> Result<Option<String>, String> {
    read(&format!("{account_id}/{kind}"), "")
}

fn delete(account_id: &str, kind: &str) -> Result<(), String> {
    remove(&format!("{account_id}/{kind}"), "")
}

/// Range les mots de passe d'un compte dans le trousseau.
pub(crate) fn store_account(c: &AccountConfig) -> Result<(), String> {
    // OAuth2 : pas de mot de passe ; les jetons sont gérés par `oauth.rs`.
    if c.imap.auth == crate::config::AuthMethod::OAuth2 {
        return delete_account(&c.id);
    }
    set(c, "imap", &c.imap.password)?;
    if c.smtp_same_credentials {
        delete(&c.id, "smtp")
    } else {
        set(c, "smtp", &c.smtp.password)
    }
}

/// Comme `store_account`, sans effacer un mot de passe déjà enregistré quand
/// celui du compte est vide : import d'une sauvegarde exportée alors que le
/// trousseau était verrouillé (relecture C4).
///
/// # Errors
///
/// Trousseau (ou coffre du mode portable) inaccessible.
///
/// Un mot de passe déjà enregistré n'est gardé que si `previous` (le compte de
/// même identifiant avant l'import) visait le même serveur : sinon une
/// sauvegarde modifiée pourrait le faire partir vers un autre hôte (relecture
/// complète n° 19). Il est alors effacé, et sera demandé à nouveau.
pub(crate) fn store_account_keeping(c: &AccountConfig, previous: Option<&AccountConfig>) -> Result<(), String> {
    if c.imap.auth == crate::config::AuthMethod::OAuth2 {
        return delete_account(&c.id);
    }
    let (imap, smtp) = keeping_plan(c, previous);
    for (kind, change) in [("imap", imap), ("smtp", smtp)] {
        match change {
            SecretChange::Set(password) => set(c, kind, password)?,
            SecretChange::Delete => delete(&c.id, kind)?,
            SecretChange::Keep => {}
        }
    }
    Ok(())
}

/// Ce que l'import fait d'un mot de passe du trousseau.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecretChange<'a> {
    /// Remplacé par celui de la sauvegarde.
    Set(&'a str),
    /// Gardé : absent de la sauvegarde, même serveur qu'avant (C4).
    Keep,
    /// Effacé : il sera demandé à nouveau.
    Delete,
}

/// Mots de passe IMAP puis SMTP d'un compte importé (`c`), d'après le compte de
/// même identifiant avant l'import (`previous`) : voir [`store_account_keeping`].
fn keeping_plan<'a>(c: &'a AccountConfig, previous: Option<&AccountConfig>) -> (SecretChange<'a>, SecretChange<'a>) {
    let same = |kind: &str| {
        previous.is_some_and(|p| match kind {
            "imap" => crate::config::same_server(&p.imap, &c.imap) && crate::config::same_server(&p.effective_smtp(), &c.effective_smtp()),
            _ => crate::config::same_server(&p.smtp, &c.smtp),
        })
    };
    let imap = if !c.imap.password.is_empty() {
        SecretChange::Set(&c.imap.password)
    } else if same("imap") {
        SecretChange::Keep
    } else {
        SecretChange::Delete
    };
    let smtp = if c.smtp_same_credentials {
        SecretChange::Delete
    } else if !c.smtp.password.is_empty() {
        SecretChange::Set(&c.smtp.password)
    } else if same("smtp") {
        SecretChange::Keep
    } else {
        SecretChange::Delete
    };
    (imap, smtp)
}

/// Complète les mots de passe manquants depuis le trousseau.
fn fill_account(c: &mut AccountConfig) -> Result<(), String> {
    if c.imap.password.is_empty() {
        if let Some(p) = get(&c.id, "imap")? {
            c.imap.password = p;
        }
    }
    if !c.smtp_same_credentials && c.smtp.password.is_empty() {
        if let Some(p) = get(&c.id, "smtp")? {
            c.smtp.password = p;
        }
    }
    Ok(())
}

/// Retire du trousseau les mots de passe d'un compte supprimé.
pub(crate) fn delete_account(account_id: &str) -> Result<(), String> {
    delete(account_id, "imap")?;
    delete(account_id, "smtp")
}

/// Au démarrage : lit les comptes, migre vers le trousseau les mots de passe
/// encore en clair dans le fichier, puis complète chaque compte depuis le
/// trousseau. Le fichier n'est réécrit sans mots de passe que si la
/// migration a entièrement réussi : rien n'est jamais perdu.
pub(crate) fn load_accounts(path: &Path) -> Vec<AccountConfig> {
    let mut accounts = match config::load(path) {
        Ok(accounts) => accounts,
        Err(e) => {
            crate::diag::report(&e);
            return Vec::new();
        }
    };
    let mut plaintext_found = false;
    let mut migration_ok = true;
    for c in &mut accounts {
        if !c.imap.password.is_empty() || !c.smtp.password.is_empty() {
            plaintext_found = true;
            if let Err(e) = store_account(c) {
                crate::diag::report(&tr!("compte « {name} » : {e}", name = c.name, e = e));
                migration_ok = false;
            }
        }
        if !c.oauth_client_id.is_empty() {
            plaintext_found = true;
            if crate::oauth::known_client_id(&c.imap.username).is_none() {
                if let Err(e) = crate::oauth::remember_client_id(&c.oauth_client_id) {
                    crate::diag::report(&e);
                    migration_ok = false;
                }
            }
        }
        if let Err(e) = fill_account(c) {
            crate::diag::report(&tr!("compte « {name} » : {e}", name = c.name, e = e));
        }
    }
    relabel_once(path, &accounts);
    if plaintext_found && migration_ok {
        match config::save(path, &accounts) {
            Ok(()) => crate::diag::report(&tr!("mots de passe déplacés dans le trousseau du système")),
            Err(e) => crate::diag::report(&e),
        }
    }
    accounts
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, String>>);

    impl Store for Memory {
        fn get(&self, key: &str) -> Result<Option<String>, String> {
            Ok(self.0.borrow().get(key).cloned())
        }
        fn set(&self, key: &str, value: &str) -> Result<(), String> {
            self.0.borrow_mut().insert(key.into(), value.into());
            Ok(())
        }
        fn del(&self, key: &str) -> Result<(), String> {
            self.0.borrow_mut().remove(key);
            Ok(())
        }
    }

    fn account(imap_host: &str, password: &str) -> AccountConfig {
        serde_json::from_value(serde_json::json!({
            "id": "a1", "name": "Test", "email": "moi@example.fr", "color": "#000",
            "smtpSameCredentials": true,
            "imap": { "host": imap_host, "port": 993, "security": "auto", "auth": "normal",
                      "username": "moi", "password": password },
            "smtp": { "host": "smtp.example.fr", "port": 465, "security": "auto", "auth": "normal",
                      "username": "moi", "password": "" },
        }))
        .expect("compte de test")
    }

    #[test]
    fn import_sans_mot_de_passe_garde_celui_du_trousseau() {
        // C4 : sauvegarde exportée trousseau verrouillé, importée là où le compte a son mot de passe.
        let before = account("imap.example.fr", "");
        let imported = account("imap.example.fr", "");
        assert_eq!(keeping_plan(&imported, Some(&before)).0, SecretChange::Keep, "même serveur : gardé");
        // Une sauvegarde modifiée qui vise un autre serveur ne l'emporte pas avec elle.
        let elsewhere = account("imap.pirate.example", "");
        assert_eq!(keeping_plan(&elsewhere, Some(&before)).0, SecretChange::Delete, "autre serveur : effacé");
        assert_eq!(keeping_plan(&imported, None).0, SecretChange::Delete, "compte nouveau : rien à garder");
        let with_password = account("imap.example.fr", "nouveau");
        assert_eq!(keeping_plan(&with_password, Some(&before)).0, SecretChange::Set("nouveau"), "mot de passe de la sauvegarde");
        assert_eq!(keeping_plan(&with_password, Some(&before)).1, SecretChange::Delete, "SMTP : mêmes identifiants qu'IMAP");
    }

    #[test]
    fn secrets_of_the_old_service_are_taken_over() {
        let (new, old) = (Memory::default(), Memory::default());
        write_in(&old, "acct/imap", "secret-très-long", 4).unwrap();
        assert_eq!(read_migrating(&new, &old, "acct/imap", 4).unwrap().as_deref(), Some("secret-très-long"));
        // Recopié (découpé de la même façon) dans le nouveau service, retiré de l'ancien.
        assert_eq!(read_in(&new, "acct/imap").unwrap().as_deref(), Some("secret-très-long"));
        assert!(old.0.borrow().is_empty());
        // Ensuite : lu directement dans le nouveau service.
        assert_eq!(read_migrating(&new, &old, "acct/imap", 4).unwrap().as_deref(), Some("secret-très-long"));
        assert_eq!(read_migrating(&new, &old, "absent", 4).unwrap(), None);
    }

    #[test]
    fn long_secrets_are_split_and_rejoined() {
        let m = Memory::default();
        write_in(&m, "oauth/a@b", "court", 4).unwrap();
        // « court » (5 caractères, morceaux de 4) : découpé en 2, génération 1.
        assert_eq!(m.0.borrow().get("oauth/a@b").unwrap(), "vortex-pieces:1:2");
        assert_eq!(read_in(&m, "oauth/a@b").unwrap().as_deref(), Some("court"));

        // Valeur plus longue puis plus courte : les morceaux de l'ancienne génération disparaissent.
        write_in(&m, "oauth/a@b", "jéton-très-long", 4).unwrap();
        assert_eq!(m.0.borrow().len(), 1 + 4);
        assert_eq!(read_in(&m, "oauth/a@b").unwrap().as_deref(), Some("jéton-très-long"));
        write_in(&m, "oauth/a@b", "abc", 4).unwrap();
        assert_eq!(m.0.borrow().len(), 1);
        assert_eq!(read_in(&m, "oauth/a@b").unwrap().as_deref(), Some("abc"));

        write_in(&m, "k", "0123456789", 4).unwrap();
        remove_in(&m, "k").unwrap();
        remove_in(&m, "oauth/a@b").unwrap();
        assert!(m.0.borrow().is_empty());
        assert_eq!(read_in(&m, "absent").unwrap(), None);
    }

    #[test]
    fn missing_piece_is_an_error() {
        let m = Memory::default();
        write_in(&m, "k", "0123456789", 4).unwrap();
        m.del("k#1.2").unwrap();
        assert!(read_in(&m, "k").is_err());
    }

    /// Trousseau qui refuse d'enregistrer à partir de la `fail_at`-ième écriture.
    struct Failing {
        inner: Memory,
        writes: Cell<usize>,
        fail_at: usize,
    }

    impl Store for Failing {
        fn get(&self, key: &str) -> Result<Option<String>, String> {
            self.inner.get(key)
        }
        fn set(&self, key: &str, value: &str) -> Result<(), String> {
            self.writes.set(self.writes.get() + 1);
            if self.writes.get() >= self.fail_at {
                return Err("trousseau indisponible".into());
            }
            self.inner.set(key, value)
        }
        fn del(&self, key: &str) -> Result<(), String> {
            self.inner.del(key)
        }
    }

    #[test]
    fn interrupted_rewrite_keeps_the_previous_secret_whole() {
        // Relecture C1 : jeton découpé, renouvellement interrompu au 2e morceau.
        let store = Failing { inner: Memory::default(), writes: Cell::new(0), fail_at: usize::MAX };
        write_in(&store, "oauth/a@b", "ancien-jeton-long", 4).unwrap();
        for fail_at in 1..=6 {
            let store = Failing { inner: Memory(RefCell::new(store.inner.0.borrow().clone())), writes: Cell::new(0), fail_at };
            assert!(write_in(&store, "oauth/a@b", "nouveau-jeton-plus-long", 4).is_err());
            assert_eq!(read_in(&store, "oauth/a@b").unwrap().as_deref(), Some("ancien-jeton-long"), "échec à l'écriture {fail_at}");
        }
    }

    #[test]
    fn secrets_of_the_previous_format_are_read_and_replaced() {
        // Format des versions précédentes : « vortex-pieces:2 », morceaux `clé#1`, `clé#2`.
        let m = Memory::default();
        m.set("k", "vortex-pieces:2").unwrap();
        m.set("k#1", "abcd").unwrap();
        m.set("k#2", "ef").unwrap();
        assert_eq!(read_in(&m, "k").unwrap().as_deref(), Some("abcdef"));
        write_in(&m, "k", "0123456789", 4).unwrap();
        assert_eq!(read_in(&m, "k").unwrap().as_deref(), Some("0123456789"));
        assert!(m.get("k#1").unwrap().is_none() && m.get("k#2").unwrap().is_none(), "anciens morceaux retirés");
        remove_in(&m, "k").unwrap();
        assert!(m.0.borrow().is_empty());
    }

    fn sealed(entries: &str, password: &str) -> String {
        crate::crypto::seal_with(password, entries.as_bytes(), 1000).unwrap()
    }

    #[test]
    fn damaged_vault_falls_back_to_its_copy() {
        // Relecture C3 : clé USB retirée pendant l'écriture du coffre.
        let dir = std::env::temp_dir().join(format!("vx-vault-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("secrets.vault");
        let password = "mot de passe maître";
        let value = |entries: Entries| entries.get("a/imap").cloned();
        std::fs::write(&file, sealed(r#"{"a/imap":"v2"}"#, password)).unwrap();
        std::fs::write(vault_backup(&file), sealed(r#"{"a/imap":"v1"}"#, password)).unwrap();

        // Coffre intact : lu tel quel ; un autre mot de passe est refusé.
        assert_eq!(open_vault(&file, password).map(value), Ok(Some("v2".into())));
        assert_eq!(open_vault(&file, "autre mot de passe"), Err(VaultError::WrongPassword));
        // Mot de passe changé : l'ancien, que porte encore la copie, est refusé,
        // et le coffre n'est pas remplacé par sa copie.
        std::fs::write(&file, sealed(r#"{"a/imap":"v3"}"#, "nouveau mot de passe")).unwrap();
        assert_eq!(open_vault(&file, password), Err(VaultError::WrongPassword));
        assert_eq!(open_vault(&file, "nouveau mot de passe").map(value), Ok(Some("v3".into())));
        std::fs::write(&file, sealed(r#"{"a/imap":"v2"}"#, password)).unwrap();

        // Coffre rempli de zéros : la copie de secours est lue, puis remise en place.
        std::fs::write(&file, vec![0u8; 512]).unwrap();
        assert_eq!(open_vault(&file, password).map(value), Ok(Some("v1".into())));
        assert_eq!(read_vault(&file, password).map(value), Ok(Some("v1".into())));

        // Coffre et copie abîmés : « abîmé », pas « mot de passe incorrect ».
        std::fs::write(&file, b"").unwrap();
        std::fs::write(vault_backup(&file), b"{}").unwrap();
        assert_eq!(open_vault(&file, password), Err(VaultError::Damaged));
        // Coffre abîmé, copie intacte mais autre mot de passe : « incorrect ».
        std::fs::write(vault_backup(&file), sealed("{}", password)).unwrap();
        assert_eq!(open_vault(&file, "autre mot de passe"), Err(VaultError::WrongPassword));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
