//! Contacts et agenda : synchronisation CardDAV/CalDAV par compte, plus un
//! carnet et un agenda « Sur cet ordinateur ». Tout est gardé dans le cache
//! SQLite : affichage immédiat, et lecture hors connexion.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::cache::{Cache, CollectedAddress, PimCollection, PimItem};
use crate::config::{AccountConfig, AuthMethod};
use crate::dav::{self, Auth, Kind, Scope};
use crate::pimfmt::{self, Contact, Event};
use crate::router::{lock_cache, AccountError};

pub(crate) const LOCAL: &str = "local";
pub(crate) const LOCAL_BOOK: &str = "local:contacts";
pub(crate) const LOCAL_CALENDAR: &str = "local:calendar";

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncReport {
    pub contacts: u32,
    pub events: u32,
    pub errors: Vec<AccountError>,
}

/// Adresse proposée en saisissant un destinataire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Suggestion {
    pub name: String,
    pub email: String,
    /// Fiche du carnet (sinon : adresse collectée, que l'on peut écarter).
    pub contact: bool,
}

// ------------------------------------------------------------------ outils

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Instant UTC au format iCalendar (`AAAAMMJJTHHMMSSZ`).
pub(crate) fn utc_stamp(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // Jours depuis 1970 → date civile (algorithme de H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// Identifiant unique (UID) pour un nouveau contact ou rendez-vous.
pub(crate) fn new_uid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // Aléa du système : ni l'heure ni le processus ne partent chez le serveur.
    if let Ok(random) = crate::oauth::random_bytes::<16>() {
        let hex: String = random.iter().fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        });
        return format!("{hex}@vortexmail");
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{nanos:x}-{:x}-{:x}@vortexmail", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

/// Identification CardDAV/CalDAV du compte, s'il en a une utilisable ici.
/// Erreur : le compte est relié, mais le jeton n'a pas pu être obtenu.
fn auth_for(c: &AccountConfig) -> Result<Option<Auth>, String> {
    let off = |kind| start_hint(c, kind).0 == "-";
    if (off(Kind::Calendar) && off(Kind::Contacts)) || is_o365(c) {
        return Ok(None); // Outlook.com : API du service (voir o365.rs)
    }
    if crate::google::is_google_host(&c.imap.host) {
        // Gmail : jeton OAuth Google, seulement si l'utilisateur a connecté l'agenda.
        if !crate::google::is_signed_in(google_user(c)) {
            return Ok(None);
        }
        let scope = Scope::new(&[crate::google::CALDAV_START, crate::google::CARDDAV_START]);
        return crate::google::access_token(google_user(c)).map(|t| Some(Auth::Bearer(t, scope)));
    }
    if c.imap.password.is_empty() {
        return Ok(None);
    }
    // Le mot de passe ne va qu'aux serveurs du compte : domaine de l'adresse,
    // adresses saisies dans les réglages ou fournisseur connu.
    let hints = [start_hint(c, Kind::Calendar).0, start_hint(c, Kind::Contacts).0];
    let mut trusted: Vec<&str> = hints.iter().map(String::as_str).filter(|h| *h != "-").collect();
    trusted.push(c.email.as_str());
    let scope = Scope::new(&trusted);
    Ok(Some(Auth::Basic { user: c.imap.username.trim().to_string(), password: c.imap.password.clone(), scope }))
}

/// Adresse Google du compte (identifiant IMAP, sinon adresse d'envoi).
pub(crate) fn google_user(c: &AccountConfig) -> &str {
    let user = c.imap.username.trim();
    if user.contains('@') {
        user
    } else {
        c.email.trim()
    }
}

/// Serveur des contacts et de l'agenda d'un compte.
enum Remote {
    Dav(Auth),
    /// Outlook.com / O365 : API du service (jeton d'accès).
    O365(String),
}

/// Compte Outlook.com (connexion `OAuth2`) : agenda et contacts par l'API du service.
fn is_o365(c: &AccountConfig) -> bool {
    c.imap.auth == AuthMethod::OAuth2 && !crate::google::is_google_host(&c.imap.host)
}

fn remote_for(c: &AccountConfig) -> Result<Option<Remote>, String> {
    if is_o365(c) {
        let user = google_user(c);
        if !crate::o365::is_signed_in(user) {
            return Ok(None);
        }
        return crate::o365::access_token(user).map(|t| Some(Remote::O365(t)));
    }
    Ok(auth_for(c)?.map(Remote::Dav))
}

fn required_remote(c: &AccountConfig) -> Result<Remote, String> {
    remote_for(c)?.ok_or_else(|| tr!("compte sans contacts ni agenda synchronisés").to_string())
}

fn delete_remote(remote: &Remote, item: &PimItem) -> Result<(), String> {
    match remote {
        Remote::Dav(auth) => dav::delete(&item.url, auth, &item.etag),
        Remote::O365(token) => crate::o365::delete(token, &item.url, &item.etag),
    }
}

/// Point de départ de la découverte, et s'il a été donné explicitement
/// (les erreurs sont alors signalées au lieu d'être ignorées). `-` : désactivé.
fn start_hint(c: &AccountConfig, kind: Kind) -> (String, bool) {
    let dav = c.dav_url.trim();
    let card = c.card_url.trim();
    let given = match kind {
        Kind::Contacts if !card.is_empty() => card,
        Kind::Calendar | Kind::Contacts => dav,
    };
    if !given.is_empty() {
        return (given.replace("{email}", c.email.trim()), true);
    }
    if crate::google::is_google_host(&c.imap.host) {
        let start = match kind {
            Kind::Calendar => crate::google::CALDAV_START,
            Kind::Contacts => crate::google::CARDDAV_START,
        };
        return (start.to_string(), true);
    }
    if let Some(url) = known_provider(c, kind) {
        return (url, true);
    }
    (c.email.clone(), false)
}

/// Mailo et Netcourrier : le même service (mêmes serveurs d'agenda et de contacts).
fn is_mailo(domain: &str, imap_host: &str) -> bool {
    const DOMAINS: [&str; 3] = ["mailo.com", "netcourrier.com", "net-c.com"];
    DOMAINS.contains(&domain)
        || domain.starts_with("net-c.")
        || DOMAINS.iter().any(|d| imap_host == *d || imap_host.ends_with(&format!(".{d}")))
}

/// Fournisseurs dont les serveurs `CalDAV` / `CardDAV` ne se trouvent pas par
/// `.well-known` (hôtes distincts pour l'agenda et les contacts).
fn known_provider(c: &AccountConfig, kind: Kind) -> Option<String> {
    let email = c.email.trim().to_lowercase();
    let domain = email.rsplit('@').next().unwrap_or("");
    let host = c.imap.host.trim().to_ascii_lowercase();
    if is_mailo(domain, &host) {
        return Some(match kind {
            Kind::Calendar => format!("https://caldav.mailo.com/calendar/{email}/1"),
            Kind::Contacts => format!("https://carddav.mailo.com/adbook/{email}/1"),
        });
    }
    None
}

/// Retire du cache les carnets, agendas et fiches d'un compte (déconnexion).
pub(crate) fn forget_account(cache: &Arc<Mutex<Cache>>, account: &str) -> Result<(), String> {
    let prefix = format!("{account}|");
    discovered().retain(|key| !key.starts_with(&prefix));
    let mut c = lock_cache(cache);
    for kind in [Kind::Contacts, Kind::Calendar] {
        c.pim_set_collections(account, kind.as_str(), &[])?;
    }
    Ok(())
}

/// Le carnet et l'agenda « Sur cet ordinateur » existent toujours.
pub(crate) fn ensure_local(cache: &Arc<Mutex<Cache>>) {
    let mut c = lock_cache(cache);
    let existing = c.pim_collections().unwrap_or_default();
    for (kind, url, name) in [("contacts", LOCAL_BOOK, "Sur cet ordinateur"), ("calendar", LOCAL_CALENDAR, "Sur cet ordinateur")] {
        if !existing.iter().any(|x| x.url == url) {
            let col =
                PimCollection { account: LOCAL.into(), url: url.into(), kind: kind.into(), name: name.into(), color: "#6b7280".into() };
            if let Err(e) = c.pim_set_collections(LOCAL, kind, &[col]) {
                crate::diag::report(&e);
            }
        }
    }
}

// ------------------------------------------------------------------ synchronisation

/// Relit carnets et agendas de tous les comptes compatibles.
pub(crate) fn sync_all(cache: &Arc<Mutex<Cache>>, configs: &[AccountConfig]) -> SyncReport {
    ensure_local(cache);
    let mut report = SyncReport::default();
    for c in configs {
        match sync_account(cache, c) {
            Ok((contacts, events)) => {
                report.contacts += contacts;
                report.events += events;
            }
            Err(message) => report.errors.push(AccountError { account_id: c.id.clone(), message }),
        }
    }
    report
}

fn sync_account(cache: &Arc<Mutex<Cache>>, c: &AccountConfig) -> Result<(u32, u32), String> {
    if is_o365(c) {
        let Some(Remote::O365(token)) = remote_for(c).map_err(|e| tr!("contacts et agenda : {e}", e = e))? else {
            return Ok((0, 0));
        };
        let from = utc_stamp(now() - 365 * 86_400);
        let to = utc_stamp(now() + 2 * 365 * 86_400);
        return crate::o365::sync(cache, c, &token, &from, &to);
    }
    let Some(auth) = auth_for(c).map_err(|e| tr!("contacts et agenda : {e}", e = e))? else { return Ok((0, 0)) };
    let mut counts = (0u32, 0u32);
    let mut errors = Vec::new();
    // L'agenda est relu même si les contacts échouent (et inversement).
    for kind in [Kind::Contacts, Kind::Calendar] {
        if let Err(e) = sync_kind(cache, c, &auth, kind, &mut counts) {
            errors.push(if kind == Kind::Contacts { tr!("contacts : {e}", e = e) } else { tr!("agenda : {e}", e = e) });
        }
    }
    if errors.is_empty() {
        Ok(counts)
    } else {
        Err(errors.join("\n"))
    }
}

/// Carnets et agendas déjà trouvés depuis le démarrage, par compte, type et
/// adresse de départ : la découverte (DNS, `.well-known`, trois `PROPFIND`)
/// n'est refaite qu'au premier passage, après une erreur, ou si l'adresse de
/// départ change (audit P6).
static DISCOVERED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn discovered() -> std::sync::MutexGuard<'static, BTreeSet<String>> {
    // Ensemble remplacé élément par élément, sans état intermédiaire : sûr après une panique.
    DISCOVERED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn sync_kind(cache: &Arc<Mutex<Cache>>, c: &AccountConfig, auth: &Auth, kind: Kind, counts: &mut (u32, u32)) -> Result<(), String> {
    let (hint, explicit) = start_hint(c, kind);
    if hint == "-" {
        return Ok(());
    }
    let key = format!("{}|{}|{hint}", c.id, kind.as_str());
    if discovered().contains(&key) {
        let known: Vec<dav::Collection> = lock_cache(cache)
            .pim_collections()?
            .into_iter()
            .filter(|col| col.account == c.id && col.kind == kind.as_str())
            .map(|col| dav::Collection { url: col.url, name: col.name, color: col.color, kind })
            .collect();
        if !known.is_empty() {
            if let Ok(found) = sync_collections(cache, c, auth, kind, &known) {
                add_counts(counts, found);
                return Ok(());
            }
        }
        // Erreur (carnet supprimé, adresse changée…) : nouvelle découverte.
        discovered().remove(&key);
    }
    let collections = match dav::discover(&hint, auth, kind) {
        Ok(list) => list,
        // Recherche automatique sans résultat : le compte n'a simplement pas de service.
        Err(_) if !explicit => return Ok(()),
        Err(e) => return Err(e),
    };
    let stored: Vec<PimCollection> = collections
        .iter()
        .map(|col| PimCollection {
            account: c.id.clone(),
            url: col.url.clone(),
            kind: kind.as_str().into(),
            name: col.name.clone(),
            color: col.color.chars().take(7).collect(),
        })
        .collect();
    lock_cache(cache).pim_set_collections(&c.id, kind.as_str(), &stored)?;
    let found = sync_collections(cache, c, auth, kind, &collections)?;
    discovered().insert(key);
    add_counts(counts, found);
    Ok(())
}

fn add_counts(counts: &mut (u32, u32), found: (u32, u32)) {
    counts.0 = counts.0.saturating_add(found.0);
    counts.1 = counts.1.saturating_add(found.1);
}

/// Relit chaque carnet ou agenda : seuls les éléments nouveaux ou changés
/// sont téléchargés et analysés, les autres sont repris du cache (audit P6).
/// Rend le nombre de contacts et de rendez-vous.
fn sync_collections(
    cache: &Arc<Mutex<Cache>>,
    c: &AccountConfig,
    auth: &Auth,
    kind: Kind,
    collections: &[dav::Collection],
) -> Result<(u32, u32), String> {
    let from = utc_stamp(now() - 365 * 86_400);
    let to = utc_stamp(now() + 2 * 365 * 86_400);
    let mut counts = (0u32, 0u32);
    let mut cached: HashMap<String, PimItem> = HashMap::new();
    for (item_kind, collection, item) in lock_cache(cache).pim_items_of(&c.id)? {
        if item_kind == kind.as_str() {
            cached.insert(format!("{collection}\n{}", item.url), item);
        }
    }
    for col in collections {
        let prefix = format!("{}\n", col.url);
        let known: HashMap<String, String> =
            cached.iter().filter_map(|(key, item)| key.strip_prefix(&prefix).map(|url| (url.to_owned(), item.etag.clone()))).collect();
        let fetched = match kind {
            Kind::Contacts => dav::fetch_contacts(&col.url, auth, &known)?,
            Kind::Calendar => dav::fetch_events(&col.url, auth, &from, &to, &known)?,
        };
        let mut rows: Vec<PimItem> = fetched
            .changed
            .into_iter()
            .filter_map(|item| {
                let id = format!("{}|{}", c.id, item.url);
                let data = match kind {
                    Kind::Contacts => {
                        let mut contact = pimfmt::parse_vcard(&item.data)?;
                        contact.id.clone_from(&id);
                        contact.source.clone_from(&c.id);
                        json(&contact)
                    }
                    Kind::Calendar => {
                        let mut events = pimfmt::parse_ical(&item.data);
                        if events.is_empty() {
                            return None;
                        }
                        for e in &mut events {
                            e.id.clone_from(&id);
                            e.source.clone_from(&c.id);
                            e.calendar.clone_from(&col.url);
                        }
                        json(&events)
                    }
                };
                Some(PimItem { id, url: item.url, etag: item.etag, raw: item.data, data })
            })
            .collect();
        rows.extend(fetched.unchanged.iter().filter_map(|url| cached.remove(&format!("{prefix}{url}"))));
        for row in &rows {
            match kind {
                Kind::Contacts => counts.0 = counts.0.saturating_add(1),
                Kind::Calendar => counts.1 = counts.1.saturating_add(event_count(&row.data)),
            }
        }
        lock_cache(cache).pim_replace_items(&c.id, kind.as_str(), &col.url, &rows)?;
    }
    Ok(counts)
}

/// Nombre de rendez-vous d'un élément d'agenda (liste JSON d'`Event`).
fn event_count(data: &str) -> u32 {
    serde_json::from_str::<Vec<serde::de::IgnoredAny>>(data).map_or(0, |events| u32::try_from(events.len()).unwrap_or(u32::MAX))
}

// ------------------------------------------------------------------ lecture

pub(crate) fn collections(cache: &Arc<Mutex<Cache>>) -> Result<Vec<PimCollection>, String> {
    ensure_local(cache);
    let mut list = lock_cache(cache).pim_collections()?;
    // Nom des collections « Sur cet ordinateur » : dans la langue de l'interface
    // (le nom rangé dans le cache est celui de leur création).
    for col in &mut list {
        if col.url == LOCAL_BOOK || col.url == LOCAL_CALENDAR {
            col.name = tr!("Sur cet ordinateur").to_string();
        }
    }
    Ok(list)
}

pub(crate) fn contacts(cache: &Arc<Mutex<Cache>>) -> Result<Vec<Contact>, String> {
    let mut list: Vec<Contact> = lock_cache(cache).pim_data("contacts")?.iter().filter_map(|d| serde_json::from_str(d).ok()).collect();
    list.sort_by_key(|c| c.name.to_lowercase());
    Ok(list)
}

pub(crate) fn events(cache: &Arc<Mutex<Cache>>) -> Result<Vec<Event>, String> {
    Ok(lock_cache(cache).pim_data("calendar")?.iter().filter_map(|d| serde_json::from_str::<Vec<Event>>(d).ok()).flatten().collect())
}

/// Nom (et sa forme en minuscules) et adresses de chaque fiche, dans l'ordre
/// du carnet.
type ContactIndex = Arc<Vec<(String, String, Vec<String>)>>;

/// Fiches gardées pour la complétion des adresses (lot J10) : relire et
/// analyser tous les contacts à chaque frappe coûte cher avec un gros carnet.
/// Clé : le cache (son adresse) et sa version des carnets (`pim_revision`).
static CONTACT_INDEX: Mutex<Option<(usize, u64, ContactIndex)>> = Mutex::new(None);

fn contact_index(cache: &Arc<Mutex<Cache>>) -> Result<ContactIndex, String> {
    let owner = Arc::as_ptr(cache).addr();
    let revision = lock_cache(cache).pim_revision();
    let mut memo = CONTACT_INDEX.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((o, r, index)) = memo.as_ref() {
        if *o == owner && *r == revision {
            return Ok(Arc::clone(index));
        }
    }
    let index: ContactIndex = Arc::new(contacts(cache)?.into_iter().map(|c| (c.name.to_lowercase(), c.name, c.emails)).collect());
    // Version lue avant les fiches : une écriture entre les deux fera seulement relire.
    *memo = Some((owner, revision, Arc::clone(&index)));
    Ok(index)
}

/// Adresses collectées prêtes pour la complétion (lot J13), de la plus utile
/// à la moins utile : (nom, adresse, nom et adresse en minuscules).
type AddressIndex = Arc<Vec<(String, String, String)>>;

/// Comme [`CONTACT_INDEX`] : gardé tant que les adresses collectées ne
/// changent pas (`addresses_revision`).
static ADDRESS_INDEX: Mutex<Option<(usize, u64, AddressIndex)>> = Mutex::new(None);

/// Rang d'une adresse collectée : ses messages envoyés, comptés à moitié tous
/// les six mois depuis le dernier envoi (on écrit d'abord aux gens d'aujourd'hui).
fn rank(address: &CollectedAddress, now: i64) -> f64 {
    let age_days = u32::try_from((now - address.last).max(0) / 86_400).unwrap_or(u32::MAX);
    f64::from(address.sent.max(1)) * 0.5_f64.powf(f64::from(age_days) / 182.0)
}

fn address_index(cache: &Arc<Mutex<Cache>>) -> Result<AddressIndex, String> {
    let owner = Arc::as_ptr(cache).addr();
    let revision = lock_cache(cache).addresses_revision();
    let mut memo = ADDRESS_INDEX.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((o, r, index)) = memo.as_ref() {
        if *o == owner && *r == revision {
            return Ok(Arc::clone(index));
        }
    }
    // Version lue avant la table : une écriture entre les deux fera seulement relire.
    let mut collected = lock_cache(cache).collected_addresses()?;
    let at = now();
    collected.sort_by(|a, b| rank(b, at).total_cmp(&rank(a, at)).then_with(|| a.email.cmp(&b.email)));
    let index: AddressIndex = Arc::new(
        collected
            .into_iter()
            .map(|a| {
                let haystack = format!("{} {}", a.name.to_lowercase(), a.email.to_lowercase());
                (a.name, a.email, haystack)
            })
            .collect(),
    );
    *memo = Some((owner, revision, Arc::clone(&index)));
    Ok(index)
}

/// Nom et adresse d'un destinataire saisi (`Nom <adresse>`, `"Dupont, Marie"
/// <m@x.fr>` ou l'adresse seule).
fn mailbox_parts(entry: &str) -> Option<(String, String)> {
    let entry = entry.trim();
    let (name, email) = match entry.strip_suffix('>').and_then(|e| e.rsplit_once('<')) {
        Some((name, email)) => (unquote(name.trim()), email.trim()),
        None => (String::new(), entry),
    };
    (email.contains('@') && !email.contains(char::is_whitespace)).then(|| (name, email.to_string()))
}

/// Nom entre guillemets (RFC 5322 §3.2.4) : une paire de guillemets retirée,
/// `\"` et `\\` rendus tels quels.
fn unquote(name: &str) -> String {
    let Some(inner) = name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) else { return name.to_string() };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' { chars.next().unwrap_or(c) } else { c });
    }
    out
}

/// Note les destinataires d'un message envoyé depuis `account` (`To`, `Cc`,
/// `Bcc`) pour la complétion (lot J13) ; ils seront retirés avec le compte.
///
/// # Errors
///
/// Cache inutilisable.
pub(crate) fn note_sent(cache: &Arc<Mutex<Cache>>, account: &str, draft: &mail_core::Draft) -> Result<(), String> {
    let recipients: Vec<(String, String)> =
        draft.to.iter().chain(&draft.cc).chain(&draft.bcc).map(String::as_str).filter_map(mailbox_parts).collect();
    lock_cache(cache).note_sent_to(account, &recipients, now())
}

/// Écarte une adresse collectée de la complétion (lot J13).
///
/// # Errors
///
/// Cache inutilisable.
pub(crate) fn forget_address(cache: &Arc<Mutex<Cache>>, email: &str) -> Result<(), String> {
    lock_cache(cache).forget_address(email)
}

/// Adresses contenant la saisie : contacts d'abord, puis adresses collectées.
pub(crate) fn suggest(cache: &Arc<Mutex<Cache>>, query: &str) -> Result<Vec<Suggestion>, String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let mut out: Vec<Suggestion> = Vec::new();
    let mut push = |name: &str, email: &str, contact: bool| {
        if !email.contains('@') || out.iter().any(|s| s.email.eq_ignore_ascii_case(email)) {
            return;
        }
        out.push(Suggestion { name: name.to_string(), email: email.to_string(), contact });
    };
    let index = contact_index(cache)?;
    for (lower, name, emails) in &*index {
        let name_match = lower.contains(&q);
        for e in emails {
            if name_match || e.to_lowercase().contains(&q) {
                push(name, e, true);
            }
        }
    }
    // Les premières suffisent : la liste est rangée de la plus utile à la moins utile.
    for (name, email, _) in address_index(cache)?.iter().filter(|(_, _, haystack)| haystack.contains(&q)).take(16) {
        push(name, email, false);
    }
    out.truncate(8);
    Ok(out)
}

// ------------------------------------------------------------------ écriture

fn config_for<'a>(configs: &'a [AccountConfig], account: &str) -> Result<&'a AccountConfig, String> {
    configs.iter().find(|c| c.id == account).ok_or_else(|| tr!("compte {account} introuvable", account = account))
}

fn collection_account(cache: &Arc<Mutex<Cache>>, collection: &str) -> Result<String, String> {
    lock_cache(cache)
        .pim_collections()?
        .into_iter()
        .find(|c| c.url == collection)
        .map(|c| c.account)
        .ok_or_else(|| tr!("carnet ou agenda introuvable : resynchronisez").to_string())
}

/// Occurrences supprimées (dates exclues ajoutées) d'une série Outlook.com que
/// Vortex Mail a créée et qui n'a pas encore été relue : l'API ne connaît pas
/// les dates exclues, chaque occurrence est supprimée par elle-même
/// (relecture du 12 novembre, n° 45). Sans effet ailleurs.
fn delete_o365_occurrences(cache: &Arc<Mutex<Cache>>, configs: &[AccountConfig], event: &Event) -> Result<(), String> {
    if event.id.is_empty() || event.exdates.is_empty() {
        return Ok(());
    }
    let Some((account, _, item)) = lock_cache(cache).pim_get(&event.id)? else { return Ok(()) };
    if account == LOCAL || !crate::o365::is_series_master(&item.raw) {
        return Ok(());
    }
    let Some(Remote::O365(token)) = remote_for(config_for(configs, &account)?)? else { return Ok(()) };
    let before: Vec<String> =
        serde_json::from_str::<Vec<Event>>(&item.data).ok().and_then(|list| list.into_iter().next()).map_or_default(|e| e.exdates);
    let series = item.url;
    for value in event.exdates.iter().filter(|x| !before.contains(x)) {
        let floating = !value.ends_with('Z') && value.len() > 8;
        let wanted = pimfmt::When { value: value.clone(), tzid: if floating { event.start.tzid.clone() } else { String::new() } };
        crate::o365::delete_occurrence(&token, &series, &wanted, &crate::tz::system_zone())?;
    }
    Ok(())
}

/// Contenu à écrire : texte (vCard, iCalendar) ou champs de l'API Outlook.com.
#[derive(Clone, Copy)]
struct Payload<'a> {
    extension: &'a str,
    content_type: &'a str,
    /// Texte CardDAV/CalDAV, à partir de la version précédente éventuelle.
    text: &'a dyn Fn(Option<&str>) -> String,
    /// Champs de l'API Outlook.com, à partir de la version précédente éventuelle (JSON brut).
    o365: &'a dyn Fn(Option<&str>) -> Result<crate::json::Json, String>,
}

/// Écrit un élément dans `collection` (création ou remplacement), sur le
/// serveur s'il y a lieu ; retire l'ancienne version si elle change de collection.
#[expect(clippy::too_many_arguments, reason = "point d'écriture commun aux contacts et aux rendez-vous, sur tous les types de serveurs")]
fn store(
    cache: &Arc<Mutex<Cache>>,
    configs: &[AccountConfig],
    kind: &str,
    id: &str,
    collection: &str,
    uid: &str,
    payload: Payload<'_>,
    data: impl Fn(&str, &str) -> String,
) -> Result<String, String> {
    let account = collection_account(cache, collection)?;
    let existing = if id.is_empty() { None } else { lock_cache(cache).pim_get(id)? };
    let same_place = existing.as_ref().is_some_and(|(a, col, _)| *a == account && col == collection);
    // La version précédente ne sert de base que si elle reste au même endroit
    // et dans le même format (une fiche Outlook.com n'est pas une vCard).
    let remote = if account == LOCAL { None } else { Some(required_remote(config_for(configs, &account)?)?) };
    let old_is_o365 = existing.as_ref().is_some_and(|(_, _, item)| item.url.starts_with(crate::o365::API));
    let raw_before =
        existing.as_ref().filter(|_| old_is_o365 == matches!(remote, Some(Remote::O365(_)))).map(|(_, _, item)| item.raw.clone());

    let (new_id, url, etag, body) = match &remote {
        None => (format!("{LOCAL}|{uid}"), String::new(), String::new(), (payload.text)(raw_before.as_deref())),
        Some(Remote::Dav(auth)) => {
            let body = (payload.text)(raw_before.as_deref());
            let (url, etag) = if let (Some((_, _, item)), true) = (&existing, same_place) {
                (item.url.clone(), item.etag.clone())
            } else {
                let name: String = uid.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
                (format!("{}/{name}.{}", collection.trim_end_matches('/'), payload.extension), String::new())
            };
            // Nouvel élément au nom choisi par Vortex Mail (UID qu'il a généré) :
            // un nouvel essai après un délai dépassé ne crée pas de doublon.
            let own = existing.is_none() && uid.ends_with("@vortexmail");
            let new_etag = if own {
                dav::put_own(&url, auth, payload.content_type, &body)?
            } else {
                dav::put(&url, auth, payload.content_type, &body, &etag, !same_place)?
            };
            // Sans version en réponse, le serveur a modifié l'élément en
            // l'enregistrant (RFC 4791 §5.3.4) : il est relu, sinon la version
            // vide ferait partir l'enregistrement suivant sans contrôle
            // (relecture du 12 novembre, n° 30). Relecture impossible : la
            // version enregistrée est gardée, sans contrôle au prochain envoi.
            let (new_etag, body) = if new_etag.is_empty() { dav::reread(&url, auth).unwrap_or((new_etag, body)) } else { (new_etag, body) };
            (format!("{account}|{url}"), url, new_etag, body)
        }
        Some(Remote::O365(token)) => {
            let fields = (payload.o365)(if same_place { raw_before.as_deref() } else { None })?;
            let target = existing.as_ref().filter(|_| same_place).map(|(_, _, item)| item);
            let (url, etag, raw) = crate::o365::put(token, kind, collection, target, &fields)?;
            (format!("{account}|{url}"), url, etag, raw)
        }
    };

    // La nouvelle version est enregistrée d'abord : elle existe déjà sur le
    // serveur, et un nouvel essai ne doit pas la recréer (relecture A9).
    let data = data(&new_id, &body);
    let item = PimItem { id: new_id.clone(), url, etag, raw: body, data };
    lock_cache(cache).pim_put(kind, &account, collection, &item)?;

    // Ancienne version ailleurs (changement de carnet ou d'agenda) : supprimée.
    // Si le serveur refuse (version changée entre-temps, réseau), elle reste
    // affichée à son ancienne place, où l'on peut la supprimer à la main ;
    // le déplacement lui-même a réussi et n'est pas refait.
    if let (Some((old_account, _, old)), false) = (&existing, same_place) {
        let removed = if old_account == LOCAL {
            Ok(())
        } else {
            match config_for(configs, old_account).ok().map(remote_for).transpose() {
                Ok(Some(Some(old_remote))) => delete_remote(&old_remote, old),
                Ok(_) => Ok(()),
                Err(e) => Err(e),
            }
        };
        match removed {
            Ok(()) => lock_cache(cache).pim_delete(&old.id)?,
            Err(e) => crate::diag::report(&e),
        }
    }
    Ok(new_id)
}

/// Enregistre un contact dans le carnet `collection`. `imported` : la vCard
/// d'origine d'un contact importé (lot E8b), qui sert de base à l'écriture
/// pour garder ce que Vortex Mail ne modifie pas (photo, champs propres).
pub(crate) fn save_contact(
    cache: &Arc<Mutex<Cache>>,
    configs: &[AccountConfig],
    mut contact: Contact,
    collection: &str,
    imported: Option<&str>,
) -> Result<Contact, String> {
    if contact.uid.is_empty() {
        contact.uid = new_uid();
    }
    let account = collection_account(cache, collection)?;
    let template = contact.clone();
    let id = store(
        cache,
        configs,
        "contacts",
        &contact.id,
        collection,
        &contact.uid,
        Payload {
            extension: "vcf",
            content_type: "text/vcard",
            text: &|raw| pimfmt::write_vcard(&template, raw.or(imported)),
            o365: &|raw| Ok(crate::o365::contact_body(&template, raw.and_then(crate::o365::parse_raw).as_ref())),
        },
        |id, _| json(&Contact { id: id.to_string(), source: account.clone(), ..template.clone() }),
    )?;
    contact.id = id;
    contact.source = account;
    Ok(contact)
}

pub(crate) fn save_event(
    cache: &Arc<Mutex<Cache>>,
    configs: &[AccountConfig],
    mut event: Event,
    collection: &str,
    raw: Option<&str>,
) -> Result<Event, String> {
    if event.uid.is_empty() {
        event.uid = new_uid();
    }
    let account = collection_account(cache, collection)?;
    let stamp = utc_stamp(now());
    delete_o365_occurrences(cache, configs, &event)?;
    let template = event.clone();
    // Importé d'une invitation : le calendrier reçu sert de base, y compris
    // pour remplacer une version précédente (mise à jour de l'organisateur,
    // relecture du 12 novembre, n° 7). Une occurrence seule s'écrit sur le
    // calendrier en place, pour garder la série.
    let imported = raw;
    let invitation_first = template.recurrence_id.is_empty();
    let id = store(
        cache,
        configs,
        "calendar",
        &event.id,
        collection,
        &event.uid,
        Payload {
            extension: "ics",
            content_type: "text/calendar",
            text: &|before| {
                let base = if invitation_first { imported.or(before) } else { before.or(imported) };
                pimfmt::write_ical(&template, base, &stamp)
            },
            o365: &|before| {
                let instance = before.is_some_and(crate::o365::is_series_instance);
                if instance && !template.rrule.is_empty() {
                    return Err(tr!("Ce rendez-vous fait partie d'une série Outlook.com : la modification ne \
                         porte que sur cette occurrence, sans répétition. Pour changer la \
                         répétition de la série, utilisez Outlook.com.")
                    .into());
                }
                let recurrence = if instance { None } else { crate::o365::recurrence_from_rule(&template.rrule, &template.start) };
                if !template.rrule.is_empty() && recurrence.is_none() && !instance {
                    return Err(tr!("Cette répétition n'est pas prise en charge par Outlook.com.").into());
                }
                let previous = before.and_then(crate::o365::parse_raw);
                Ok(crate::o365::event_body(&template, recurrence, previous.as_ref(), &crate::tz::system_zone()))
            },
        },
        |id, body| {
            // Calendrier iCalendar écrit : la série et ses occurrences modifiées
            // (relecture F2), comme après une synchronisation.
            let mut events = if body.starts_with("BEGIN:VCALENDAR") { pimfmt::parse_ical(body) } else { Vec::new() };
            if events.is_empty() {
                events = vec![Event { recurrence_id: String::new(), ..template.clone() }];
            }
            for e in &mut events {
                e.id = id.to_string();
                e.source.clone_from(&account);
                e.calendar = collection.to_string();
            }
            json(&events)
        },
    )?;
    event.id = id;
    event.source = account;
    event.calendar = collection.to_string();
    Ok(event)
}

// ------------------------------------------------------------------ import et export (lot E8b)

/// Éléments en cache d'un agenda ou d'un carnet.
fn collection_items(cache: &Arc<Mutex<Cache>>, collection: &str) -> Result<Vec<PimItem>, String> {
    let account = collection_account(cache, collection)?;
    Ok(lock_cache(cache).pim_items_of(&account)?.into_iter().filter(|(_, c, _)| c == collection).map(|(_, _, item)| item).collect())
}

/// Lignes d'un bloc, repliées, fins de ligne `\r\n`.
fn block_text(block: &[String]) -> String {
    block.iter().map(|l| pimfmt::fold(l)).collect()
}

/// `UID` d'un bloc (`VEVENT`, `VCARD`), vide s'il n'en a pas.
fn block_uid(block: &[String]) -> String {
    block.iter().filter_map(|l| pimfmt::parse_line(l)).find(|l| l.name == "UID").map_or_default(|l| l.value.trim().to_string())
}

/// Calendrier iCalendar fait de fuseaux et de rendez-vous déjà repliés.
fn calendar_text(timezones: &str, events: &str) -> String {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Vortex Mail//FR\r\n{timezones}{events}END:VCALENDAR\r\n")
}

/// Un agenda en un seul fichier iCalendar, ou un carnet en un seul fichier
/// vCard ; renvoie le texte et le nombre de rendez-vous ou de contacts. Les
/// éléments d'Outlook.com (gardés en JSON) sont écrits par Vortex Mail.
///
/// # Errors
/// Agenda ou carnet inconnu, cache illisible.
pub(crate) fn export_collection(cache: &Arc<Mutex<Cache>>, collection: &str) -> Result<(String, u32), String> {
    let kind = lock_cache(cache).pim_collections()?.into_iter().find(|c| c.url == collection).map(|c| c.kind);
    let items = collection_items(cache, collection)?;
    let count = u32::try_from(items.len()).unwrap_or(u32::MAX);
    if kind.as_deref() == Some("contacts") {
        let mut out = String::new();
        for item in &items {
            if item.raw.trim_start().to_ascii_uppercase().starts_with("BEGIN:VCARD") {
                out.extend(pimfmt::component_blocks(&item.raw, "VCARD").iter().map(|b| block_text(b)));
            } else if let Ok(c) = serde_json::from_str::<Contact>(&item.data) {
                out.push_str(&pimfmt::write_vcard(&c, None));
            }
        }
        return Ok((out, count));
    }
    let stamp = utc_stamp(now());
    let (mut zones, mut seen_zones, mut events) = (String::new(), std::collections::HashSet::new(), String::new());
    for item in &items {
        let raw = if item.raw.trim_start().to_ascii_uppercase().starts_with("BEGIN:VCALENDAR") {
            item.raw.clone()
        } else {
            let parsed: Vec<Event> = serde_json::from_str(&item.data).unwrap_or_default();
            parsed.iter().map(|e| pimfmt::write_ical(e, None, &stamp)).collect()
        };
        for zone in pimfmt::component_blocks(&raw, "VTIMEZONE") {
            if seen_zones.insert(block_uid_or_tzid(&zone)) {
                zones.push_str(&block_text(&zone));
            }
        }
        events.extend(pimfmt::vevent_blocks(&raw).iter().map(|b| block_text(b)));
    }
    Ok((calendar_text(&zones, &events), count))
}

/// Nom d'un fuseau (`TZID`), pour ne l'écrire qu'une fois.
fn block_uid_or_tzid(block: &[String]) -> String {
    block.iter().filter_map(|l| pimfmt::parse_line(l)).find(|l| l.name == "TZID").map_or_default(|l| l.value)
}

/// Identifiants des éléments déjà dans la collection, par `UID` : un élément
/// importé une seconde fois remplace le premier au lieu de faire un doublon.
fn known_uids(cache: &Arc<Mutex<Cache>>, collection: &str) -> Result<std::collections::HashMap<String, String>, String> {
    let mut known = std::collections::HashMap::new();
    for item in collection_items(cache, collection)? {
        let uids: Vec<String> = if let Ok(events) = serde_json::from_str::<Vec<Event>>(&item.data) {
            events.into_iter().map(|e| e.uid).collect()
        } else {
            serde_json::from_str::<Contact>(&item.data).map_or_default(|c| vec![c.uid])
        };
        for uid in uids.into_iter().filter(|u| !u.is_empty()) {
            known.insert(uid, item.id.clone());
        }
    }
    Ok(known)
}

/// Importe les rendez-vous d'un fichier iCalendar dans l'agenda `collection` :
/// chaque `UID` (série et occurrences modifiées) devient un rendez-vous, écrit
/// sur le calendrier reçu comme pour une invitation ; un `UID` déjà présent
/// est mis à jour. Un rendez-vous refusé n'arrête pas les suivants.
///
/// # Errors
/// Fichier sans rendez-vous, agenda inconnu.
pub(crate) fn import_calendar(
    cache: &Arc<Mutex<Cache>>,
    configs: &[AccountConfig],
    collection: &str,
    text: &str,
) -> Result<crate::router::ImportReport, String> {
    let blocks = pimfmt::vevent_blocks(text);
    if blocks.is_empty() {
        return Err(tr!("Aucun rendez-vous dans ce fichier.").into());
    }
    let zones: String = pimfmt::component_blocks(text, "VTIMEZONE").iter().map(|b| block_text(b)).collect();
    // Série et occurrences modifiées ensemble, dans l'ordre du fichier.
    // Table UID → place du groupe : un passage, au lieu d'une recherche dans
    // tous les groupes pour chaque bloc (audit P7).
    let mut groups: Vec<(String, String)> = Vec::new();
    let mut by_uid: HashMap<String, usize> = HashMap::new();
    for block in &blocks {
        let uid = block_uid(block);
        if let Some(&at) = by_uid.get(&uid).filter(|_| !uid.is_empty()) {
            groups[at].1.push_str(&block_text(block));
        } else {
            if !uid.is_empty() {
                by_uid.insert(uid.clone(), groups.len());
            }
            groups.push((uid, block_text(block)));
        }
    }
    let known = known_uids(cache, collection)?;
    let mut report = crate::router::ImportReport::default();
    for (uid, events) in groups {
        let raw = calendar_text(&zones, &events);
        let parsed = pimfmt::parse_ical(&raw);
        let Some(mut event) = parsed.iter().find(|e| e.recurrence_id.is_empty()).or_else(|| parsed.first()).cloned() else {
            report.failed += 1;
            continue;
        };
        event.id = known.get(&uid).cloned().unwrap_or_default();
        match save_event(cache, configs, event, collection, Some(&raw)) {
            Ok(_) => report.imported += 1,
            Err(e) => {
                report.failed += 1;
                report.error = Some(e);
            }
        }
    }
    Ok(report)
}

/// Importe les contacts d'un fichier vCard dans le carnet `collection` ; un
/// `UID` déjà présent est mis à jour. Un contact refusé n'arrête pas les suivants.
///
/// # Errors
/// Fichier sans contact, carnet inconnu.
pub(crate) fn import_contacts(
    cache: &Arc<Mutex<Cache>>,
    configs: &[AccountConfig],
    collection: &str,
    text: &str,
) -> Result<crate::router::ImportReport, String> {
    let blocks = pimfmt::component_blocks(text, "VCARD");
    if blocks.is_empty() {
        return Err(tr!("Aucun contact dans ce fichier.").into());
    }
    let known = known_uids(cache, collection)?;
    let mut report = crate::router::ImportReport::default();
    for block in &blocks {
        let raw = block_text(block);
        let Some(mut contact) = pimfmt::parse_vcard(&raw) else {
            report.failed += 1;
            continue;
        };
        contact.id = known.get(&contact.uid).cloned().unwrap_or_default();
        match save_contact(cache, configs, contact, collection, Some(&raw)) {
            Ok(_) => report.imported += 1,
            Err(e) => {
                report.failed += 1;
                report.error = Some(e);
            }
        }
    }
    Ok(report)
}

/// Suite à donner à la réponse à une invitation de l'agenda (lot H1b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answered {
    /// Réponse transmise par le serveur ou par l'API, ou gardée sans envoi.
    Done,
    /// Serveur qui ne planifie pas, ou agenda « Sur cet ordinateur » : la
    /// réponse part par courrier (iMIP) depuis le compte `account`, à partir de
    /// l'invitation `raw` telle qu'elle était.
    ByMail { account: String, raw: String },
}

/// Rendez-vous d'un élément en cache, avec la réponse `answer` du compte
/// `email` portée sur sa ligne d'invité, prêts pour l'interface.
fn answered_data(item: &PimItem, email: &str, answer: crate::imip::PartStat) -> String {
    let mut events: Vec<Event> = serde_json::from_str(&item.data).unwrap_or_default();
    for e in &mut events {
        for a in e.attendees.iter_mut().filter(|a| a.email.eq_ignore_ascii_case(email)) {
            a.partstat = answer.ical().to_string();
        }
    }
    json(&events)
}

/// Répond à une invitation déposée dans l'agenda (lot H1b) :
/// - Outlook.com : par l'API (`accept`, `tentativelyAccept`, `decline`) ;
/// - `CalDAV` : la copie de l'invité est réécrite, sa ligne `ATTENDEE` portant la
///   réponse ; un serveur qui planifie (RFC 6638) prévient l'organisateur, sauf
///   si `send` est faux ; sinon la réponse part par courrier ([`Answered::ByMail`]) ;
/// - « Sur cet ordinateur » : la copie locale change, la réponse part par courrier.
///
/// `as_account` : compte qui répond, quand l'invitation vise plusieurs adresses
/// gérées par Vortex Mail. Hors du compte de l'agenda, seule la copie en cache
/// change et la réponse part par courrier, depuis ce compte : un serveur
/// n'accepte la réponse que de son propriétaire.
///
/// Un refus garde le rendez-vous sur le serveur (l'interface ne l'affiche
/// plus) : le supprimer enverrait un second refus.
///
/// # Errors
/// Rendez-vous introuvable, compte absent des invités, refus du serveur.
pub(crate) fn answer_event(
    cache: &Arc<Mutex<Cache>>,
    configs: &[AccountConfig],
    id: &str,
    answer: crate::imip::PartStat,
    send: bool,
    as_account: Option<&str>,
) -> Result<Answered, String> {
    let Some((account, collection, item)) = lock_cache(cache).pim_get(id)? else {
        return Err(tr!("Rendez-vous introuvable dans l'agenda.").into());
    };
    let invited = pimfmt::parse_ical(&item.raw);
    let is_invited = |c: &&AccountConfig| invited.iter().any(|e| e.attendees.iter().any(|a| a.email.eq_ignore_ascii_case(&c.email)));
    if let Some(chosen) = as_account.filter(|chosen| *chosen != account) {
        let me = configs
            .iter()
            .filter(is_invited)
            .find(|c| c.id == chosen)
            .ok_or_else(|| tr!("Ce compte n'est pas invité à ce rendez-vous.").to_string())?;
        let updated = crate::imip::updated(&item.raw, &me.email, answer);
        let data = answered_data(&item, &me.email, answer);
        let kept = PimItem { raw: updated, data, ..item.clone() };
        lock_cache(cache).pim_put("calendar", &account, &collection, &kept)?;
        return Ok(if send { Answered::ByMail { account: me.id.clone(), raw: item.raw } } else { Answered::Done });
    }
    if account == LOCAL {
        let me = configs.iter().find(is_invited).ok_or_else(|| tr!("Aucun de vos comptes n'est invité à ce rendez-vous.").to_string())?;
        let updated = crate::imip::updated(&item.raw, &me.email, answer);
        let data = answered_data(&item, &me.email, answer);
        let kept = PimItem { raw: updated, data, ..item.clone() };
        lock_cache(cache).pim_put("calendar", &account, &collection, &kept)?;
        return Ok(if send { Answered::ByMail { account: me.id.clone(), raw: item.raw } } else { Answered::Done });
    }
    let config = config_for(configs, &account)?;
    match required_remote(config)? {
        Remote::O365(token) => {
            crate::o365::respond(&token, &item.url, answer.graph_action(), send)?;
            if answer == crate::imip::PartStat::Declined {
                // L'API retire le rendez-vous refusé de l'agenda.
                lock_cache(cache).pim_delete(id)?;
            } else {
                let kept = PimItem { data: answered_data(&item, &config.email, answer), ..item.clone() };
                lock_cache(cache).pim_put("calendar", &account, &collection, &kept)?;
            }
            Ok(Answered::Done)
        }
        Remote::Dav(auth) => {
            let updated = crate::imip::updated(&item.raw, &config.email, answer);
            let scheduling = dav::auto_schedule(&collection, &auth);
            let etag = dav::put_answer(&item.url, &auth, &updated, &item.etag, send)?;
            let (etag, raw) = if etag.is_empty() { dav::reread(&item.url, &auth).unwrap_or((etag, updated)) } else { (etag, updated) };
            let kept = PimItem { etag, raw, data: answered_data(&item, &config.email, answer), ..item.clone() };
            lock_cache(cache).pim_put("calendar", &account, &collection, &kept)?;
            Ok(if send && !scheduling { Answered::ByMail { account: account.clone(), raw: item.raw } } else { Answered::Done })
        }
    }
}

pub(crate) fn delete_item(cache: &Arc<Mutex<Cache>>, configs: &[AccountConfig], id: &str) -> Result<(), String> {
    let Some((account, _, item)) = lock_cache(cache).pim_get(id)? else { return Ok(()) };
    if account != LOCAL {
        delete_remote(&required_remote(config_for(configs, &account)?)?, &item)?;
    }
    lock_cache(cache).pim_delete(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(1_793_714_400), "20261103T140000Z");
        assert_eq!(utc_stamp(951_782_400), "20000229T000000Z");
    }

    #[test]
    fn mailo_and_netcourrier() {
        assert!(is_mailo("mailo.com", "mail.mailo.com"));
        assert!(is_mailo("netcourrier.com", ""));
        assert!(is_mailo("net-c.fr", ""));
        assert!(is_mailo("mondomaine.fr", "mail.netcourrier.com"));
        assert!(!is_mailo("gmail.com", "imap.gmail.com"));
        assert!(!is_mailo("exemple.fr", "faux-mailo.com"));
    }

    #[test]
    fn a9_deplacement_interrompu_sans_doublon() {
        // A9 : un contact du carnet « pro » déplacé vers « Sur cet ordinateur » ;
        // la suppression de l'ancien échoue (serveur injoignable), puis on réessaie.
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        // Port local libéré juste avant : personne n'y répond, la suppression échoue.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let server = format!("https://127.0.0.1:{port}/dav/");
        let book = format!("{server}contacts/");
        let book = book.as_str();
        let collection = |account: &str, url: &str| crate::cache::PimCollection {
            account: account.into(),
            url: url.into(),
            kind: "contacts".into(),
            name: "Contacts".into(),
            color: String::new(),
        };
        {
            let mut c = lock_cache(&cache);
            c.pim_set_collections(LOCAL, "contacts", &[collection(LOCAL, LOCAL_BOOK)]).unwrap();
            c.pim_set_collections("pro", "contacts", &[collection("pro", book)]).unwrap();
            let url = format!("{book}marie.vcf");
            let raw = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:marie\r\nFN:Marie Dupont\r\nEND:VCARD\r\n".to_string();
            let item = PimItem { id: format!("pro|{url}"), url, etag: "\"1\"".into(), raw, data: String::new() };
            c.pim_put("contacts", "pro", book, &item).unwrap();
        }
        let configs: Vec<AccountConfig> = vec![serde_json::from_value(serde_json::json!({
            "id": "pro", "name": "Pro", "email": "moi@example.fr", "color": "#000",
            "davUrl": server,
            "imap": { "host": "127.0.0.1", "port": 993, "security": "auto", "auth": "normal",
                      "username": "moi", "password": "secret" },
            "smtp": { "host": "127.0.0.1", "port": 465, "security": "auto", "auth": "normal",
                      "username": "moi", "password": "" },
        }))
        .expect("compte de test")];
        let old_id = format!("pro|{book}marie.vcf");
        let contact = Contact { id: old_id.clone(), uid: "marie".into(), name: "Marie Dupont".into(), ..Contact::default() };

        for essai in 1..=2 {
            save_contact(&cache, &configs, contact.clone(), LOCAL_BOOK, None).unwrap();
            let c = lock_cache(&cache);
            let local = c.pim_items_of(LOCAL).unwrap();
            assert_eq!(local.len(), 1, "essai {essai} : la copie déplacée n'existe qu'une fois");
            assert!(c.pim_get(&old_id).unwrap().is_some(), "essai {essai} : l'ancienne reste à sa place");
        }
    }

    #[test]
    fn p6_rendez_vous_comptes_sans_les_relire() {
        assert_eq!(event_count(r#"[{"id":"a"},{"id":"b"}]"#), 2);
        assert_eq!(event_count("[]"), 0);
        assert_eq!(event_count("illisible"), 0);
    }

    #[test]
    fn uids_are_unique() {
        assert_ne!(new_uid(), new_uid());
    }

    #[test]
    fn j10_completion_suit_les_changements_du_carnet() {
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        ensure_local(&cache);
        let vcf = |name: &str| format!("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:marie@x\r\nFN:{name}\r\nEMAIL:marie@x.fr\r\nEND:VCARD\r\n");
        import_contacts(&cache, &[], LOCAL_BOOK, &vcf("Marie Curie")).unwrap();
        let found = suggest(&cache, "curie").unwrap();
        assert_eq!(found.first().map(|s| (s.name.as_str(), s.email.as_str(), s.contact)), Some(("Marie Curie", "marie@x.fr", true)));
        // Fiche renommée : la complétion le voit aussitôt, sans index périmé.
        import_contacts(&cache, &[], LOCAL_BOOK, &vcf("Marie Sklodowska")).unwrap();
        assert!(suggest(&cache, "curie").unwrap().iter().all(|s| !s.contact));
        assert_eq!(suggest(&cache, "sklodowska").unwrap().len(), 1);
        // Un autre cache (autre profil) ne reprend pas l'index du premier.
        let other = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        assert_eq!(suggest(&other, "sklodowska").unwrap(), Vec::<Suggestion>::new());
    }

    #[test]
    fn j13_completion_par_les_adresses_collectees() {
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let draft = mail_core::Draft {
            to: vec![r#""Dupont, Marie" <marie@x.fr>"#.into()],
            cc: vec!["paul@y.fr".into(), "pas une adresse".into()],
            bcc: vec!["Cachée <cache@z.fr>".into()],
            ..mail_core::Draft::default()
        };
        note_sent(&cache, "a", &draft).unwrap();
        let shown = |q: &str| suggest(&cache, q).unwrap().into_iter().map(|s| (s.name, s.email, s.contact)).collect::<Vec<_>>();
        assert_eq!(shown("dupont"), [("Dupont, Marie".to_string(), "marie@x.fr".to_string(), false)], "nom entre guillemets lu");
        assert_eq!(shown("y.fr"), [(String::new(), "paul@y.fr".to_string(), false)]);
        assert_eq!(shown("cach").len(), 1, "copie cachée comprise");
        // Écartée : plus proposée, sans index périmé.
        forget_address(&cache, "marie@x.fr").unwrap();
        assert_eq!(shown("dupont"), Vec::new());
    }

    #[test]
    fn j13_rang_frequence_et_anciennete() {
        let at = 1_800_000_000;
        let address =
            |sent: u32, days_ago: i64| CollectedAddress { email: "x@y.fr".into(), name: String::new(), sent, last: at - days_ago * 86_400 };
        assert!(rank(&address(2, 0), at) > rank(&address(10, 3 * 365), at), "écrit récemment avant écrit souvent il y a longtemps");
        assert!(rank(&address(5, 10), at) > rank(&address(2, 10), at), "à date égale, le plus souvent");
        assert!(rank(&address(0, 0), at) > 0.0, "noté à l'envoi, pas encore trouvé dans « Envoyés »");
        assert_eq!(mailbox_parts(r#"  "Le \"Chef\"" <chef@x.fr> "#), Some((r#"Le "Chef""#.to_string(), "chef@x.fr".to_string())));
        assert_eq!(mailbox_parts("sans arobase"), None);
    }

    #[test]
    fn e8b_import_et_export() {
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        ensure_local(&cache);
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Ailleurs//FR\r\n\
                   BEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\n\
                   TZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
                   BEGIN:VEVENT\r\nUID:serie@x\r\nDTSTART;TZID=Europe/Paris:20261120T100000\r\n\
                   DTEND;TZID=Europe/Paris:20261120T110000\r\nRRULE:FREQ=WEEKLY\r\nSUMMARY:Point\r\n\
                   X-AILLEURS:garde\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:serie@x\r\nRECURRENCE-ID;TZID=Europe/Paris:20261127T100000\r\n\
                   DTSTART;TZID=Europe/Paris:20261127T140000\r\nDTEND;TZID=Europe/Paris:20261127T150000\r\n\
                   SUMMARY:Point déplacé\r\nEND:VEVENT\r\n\
                   BEGIN:VEVENT\r\nUID:seul@x\r\nDTSTART;VALUE=DATE:20261201\r\nSUMMARY:Congé\r\nEND:VEVENT\r\n\
                   END:VCALENDAR\r\n";
        let report = import_calendar(&cache, &[], LOCAL_CALENDAR, ics).unwrap();
        assert_eq!((report.imported, report.failed), (2, 0), "une série avec son occurrence, un rendez-vous : {report:?}");
        // Importé une seconde fois : mis à jour, pas de doublon.
        import_calendar(&cache, &[], LOCAL_CALENDAR, ics).unwrap();
        let (text, count) = export_collection(&cache, LOCAL_CALENDAR).unwrap();
        assert_eq!(count, 2, "{text}");
        assert_eq!(text.matches("BEGIN:VTIMEZONE").count(), 1, "fuseau écrit une fois : {text}");
        assert_eq!(text.matches("BEGIN:VEVENT").count(), 3, "série, occurrence modifiée, rendez-vous : {text}");
        assert!(text.contains("X-AILLEURS:garde"), "propriétés inconnues gardées : {text}");
        assert!(text.starts_with("BEGIN:VCALENDAR\r\n") && text.ends_with("END:VCALENDAR\r\n"), "{text}");
        assert!(import_calendar(&cache, &[], LOCAL_CALENDAR, "BEGIN:VCARD\r\nEND:VCARD\r\n").is_err(), "pas d'agenda");

        let vcf = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:marie@x\r\nFN:Marie Curie\r\nN:Curie;Marie;;;\r\n\
                   EMAIL:marie@x.fr\r\nX-AILLEURS:garde\r\nEND:VCARD\r\n\
                   BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Paul\r\nEMAIL:paul@y.fr\r\nEND:VCARD\r\n";
        let report = import_contacts(&cache, &[], LOCAL_BOOK, vcf).unwrap();
        assert_eq!((report.imported, report.failed), (2, 0), "{report:?}");
        let (text, count) = export_collection(&cache, LOCAL_BOOK).unwrap();
        assert_eq!(count, 2, "{text}");
        assert_eq!(text.matches("BEGIN:VCARD").count(), 2, "{text}");
        assert!(text.contains("Marie Curie") && text.contains("paul@y.fr"), "{text}");
        assert!(text.contains("X-AILLEURS:garde"), "propriétés inconnues gardées : {text}");
    }

    #[test]
    fn h1b_reponse_dans_l_agenda_local() {
        let cache = Arc::new(Mutex::new(Cache::in_memory().unwrap()));
        let raw = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:r1\r\nDTSTART:20261120T100000Z\r\nSUMMARY:Point\r\n\
                   ORGANIZER;CN=Marie:mailto:marie@exemple.fr\r\n\
                   ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:moi@example.fr\r\n\
                   ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:perso@example.fr\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
            .to_string();
        let mut events = pimfmt::parse_ical(&raw);
        for e in &mut events {
            e.id = "local|r1".into();
        }
        let item = PimItem { id: "local|r1".into(), url: String::new(), etag: String::new(), raw, data: json(&events) };
        lock_cache(&cache).pim_put("calendar", LOCAL, LOCAL_CALENDAR, &item).unwrap();
        let account = |id: &str, email: &str| -> AccountConfig {
            serde_json::from_value(serde_json::json!({
                "id": id, "name": id, "email": email, "color": "#000",
                "imap": { "host": "127.0.0.1", "port": 993, "security": "auto", "auth": "normal", "username": "moi", "password": "" },
                "smtp": { "host": "127.0.0.1", "port": 465, "security": "auto", "auth": "normal", "username": "moi", "password": "" },
            }))
            .expect("compte de test")
        };
        let configs = vec![account("pro", "Moi@Example.fr"), account("perso", "perso@example.fr"), account("autre", "autre@example.fr")];

        let done = answer_event(&cache, &configs, "local|r1", crate::imip::PartStat::Accepted, false, None).unwrap();
        assert_eq!(done, Answered::Done, "sans envoi : rien d'autre à faire");
        let (_, _, kept) = lock_cache(&cache).pim_get("local|r1").unwrap().unwrap();
        assert!(kept.raw.contains("PARTSTAT=ACCEPTED:mailto:moi@example.fr"), "{}", kept.raw);
        let shown: Vec<Event> = serde_json::from_str(&kept.data).unwrap();
        assert_eq!(shown[0].attendees[0].partstat, "ACCEPTED", "réponse visible dans l'agenda");

        let mail = answer_event(&cache, &configs, "local|r1", crate::imip::PartStat::Declined, true, None).unwrap();
        assert!(matches!(mail, Answered::ByMail { ref account, .. } if account == "pro"), "réponse par courrier : {mail:?}");
        assert!(answer_event(&cache, &configs, "local|absent", crate::imip::PartStat::Accepted, false, None).is_err(), "introuvable");

        // Invitation adressée à deux comptes : réponse au nom du second.
        let perso = answer_event(&cache, &configs, "local|r1", crate::imip::PartStat::Tentative, true, Some("perso")).unwrap();
        assert!(matches!(perso, Answered::ByMail { ref account, .. } if account == "perso"), "envoyée par le compte choisi : {perso:?}");
        let (_, _, kept) = lock_cache(&cache).pim_get("local|r1").unwrap().unwrap();
        assert!(kept.raw.contains("PARTSTAT=TENTATIVE:mailto:perso@example.fr"), "{}", kept.raw);
        assert!(kept.raw.contains("PARTSTAT=DECLINED:mailto:moi@example.fr"), "l'autre réponse ne change pas : {}", kept.raw);
        assert!(
            answer_event(&cache, &configs, "local|r1", crate::imip::PartStat::Accepted, false, Some("autre")).is_err(),
            "compte absent des invités"
        );
    }
}
