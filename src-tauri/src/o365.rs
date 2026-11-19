//! Agenda et contacts d'Outlook.com / O365 par l'API REST du service (il ne
//! propose ni `CalDAV` ni `CardDAV`). Adresses techniques : `endpoints.rs`.
//!
//! - Connexion : même ID d'application que pour le courrier, mais un jeton
//!   distinct (autorisations Calendars.ReadWrite et Contacts.ReadWrite), rangé
//!   dans le trousseau sous `o365pim/<adresse>`.
//! - Agenda : chaque occurrence est lue telle que le service l'affiche
//!   (`calendarView`) ; une modification ne touche que cette occurrence.
//! - Contacts : dossier « Contacts » et ses sous-dossiers.

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cache::{Cache, PimCollection, PimItem};
use crate::config::AccountConfig;
use crate::json::{self, Json};
use crate::oauth::{
    base64url, form, known_client_id, open_browser, random_bytes, sha256, signed_in_as, token_request, token_request_ex,
    wait_for_code_from, LOGIN_HOST, TENANT,
};
use crate::pimfmt::{Contact, Event, When};
use crate::router::lock_cache;

pub(crate) use crate::endpoints::PIM_API as API;
use crate::endpoints::{PIM_PREFER_TEXT, PIM_PREFER_UTC, PIM_SCOPE as SCOPE, PIM_SIGN_IN_SCOPE as SIGN_IN_SCOPE};
const MAX_RESPONSE: usize = 32 * 1024 * 1024;
/// Taille cumulée des pages d'une liste : un serveur ne peut pas remplir la
/// mémoire page après page (200 pages de 32 Mo).
const MAX_LIST_BYTES: usize = 256 * 1024 * 1024;

// ================================================================ jetons

fn key(user: &str) -> String {
    format!("o365pim/{}", user.trim().to_lowercase())
}

/// Clé des versions précédentes, reprise à la première lecture.
fn old_key(user: &str) -> String {
    format!("graph/{}", user.trim().to_lowercase())
}

fn label(user: &str) -> String {
    tr!("Vortex Mail — {user} (agenda et contacts Outlook.com)", user = user.trim().to_lowercase())
}

/// Entrées du trousseau d'un compte à sauvegarder : (clé, libellé).
pub(crate) fn backup_entries(user: &str) -> Vec<(String, String)> {
    vec![(key(user), label(user))]
}

fn load(user: &str) -> Result<Option<String>, String> {
    if let Some(v) = crate::secrets::read(&key(user), &label(user))? {
        return Ok(Some(v));
    }
    let Some(v) = crate::secrets::read(&old_key(user), &label(user))? else { return Ok(None) };
    if crate::secrets::write(&key(user), &label(user), &v).is_ok() {
        let _ = crate::secrets::remove(&old_key(user), &label(user));
    }
    Ok(Some(v))
}

fn save(user: &str, refresh: &str) -> Result<(), String> {
    crate::secrets::write(&key(user), &label(user), refresh)
}

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

/// Comme `access_refused`, quand seul le jeton refusé est connu.
pub(crate) fn token_refused(token: &str) {
    let user = ACCESS
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref()?.iter().find(|(_, (t, _))| t == token).map(|(k, _)| k.trim_start_matches("o365pim/").to_string()));
    if let Some(user) = user {
        access_refused(&user);
    }
}

pub(crate) fn is_signed_in(user: &str) -> bool {
    matches!(load(user), Ok(Some(_)))
}

pub(crate) fn forget(user: &str) -> Result<(), String> {
    crate::auth::forgotten("o365", crate::auth::PIM, user);
    crate::auth::sign_out(&key(user), || {
        if let Ok(mut guard) = ACCESS.lock() {
            if let Some(map) = guard.as_mut() {
                map.remove(&key(user));
            }
        }
        let _ = crate::secrets::remove(&old_key(user), &label(user));
        crate::secrets::remove(&key(user), &label(user))
    })
}

/// Jeton d'accès valide (renouvelé si nécessaire).
pub(crate) fn access_token(user: &str) -> Result<String, String> {
    if let Some(token) = cache_get(user) {
        return Ok(token);
    }
    let signed_out = crate::auth::sign_out_count(&key(user));
    let not_connected = || {
        crate::auth::sign_in_required("o365", crate::auth::PIM, user);
        tr!("Agenda et contacts Outlook.com non connectés : ouvrez Paramètres, modifiez le compte \
             et cliquez sur « Connecter l'agenda et les contacts ».")
        .to_string()
    };
    let refresh = load(user)?.ok_or_else(not_connected)?;
    let client_id = known_client_id(user).ok_or_else(not_connected)?;
    let response =
        token_request_ex(&[("client_id", &client_id), ("grant_type", "refresh_token"), ("refresh_token", &refresh), ("scope", SCOPE)])
            .map_err(|(rejected, e)| {
                if rejected {
                    crate::auth::sign_in_required("o365", crate::auth::PIM, user);
                }
                tr!("{e}. Reconnectez l'agenda et les contacts dans Paramètres.", e = e)
            })?;
    let token = response.access_token.clone().unwrap_or_default();
    // Rien n'est gardé si l'agenda a été déconnecté pendant la demande (A8).
    crate::auth::unless_signed_out(&key(user), signed_out, || {
        if let Some(new_refresh) = response.refresh_token.as_deref().filter(|r| *r != refresh) {
            save(user, new_refresh)?;
        }
        cache_put(user, &token, response.expires_in.unwrap_or(3600));
        Ok(())
    })?;
    Ok(token)
}

/// Jeton de l'agenda et des contacts tiré du jeton de renouvellement du courrier, quand l'agenda
/// et les contacts ont été autorisés dans la même page de connexion.
pub(crate) fn adopt(user: &str, client_id: &str, mail_refresh: &str) -> Result<(), String> {
    let response =
        token_request(&[("client_id", client_id), ("grant_type", "refresh_token"), ("refresh_token", mail_refresh), ("scope", SCOPE)])?;
    let refresh = response.refresh_token.unwrap_or_else(|| mail_refresh.to_string());
    save(user, &refresh)?;
    cache_put(user, &response.access_token.unwrap_or_default(), response.expires_in.unwrap_or(3600));
    Ok(())
}

/// Autorise l'agenda et les contacts dans le navigateur (même application
/// application que le courrier : son ID est déjà dans le trousseau).
pub(crate) fn sign_in(user: &str) -> Result<(), String> {
    let client_id = known_client_id(user).ok_or(tr!("Connectez d'abord le courrier de ce compte avec « Se connecter à Outlook.com »."))?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| tr!("port local indisponible : {e}", e = e))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://localhost:{port}");
    let verifier = base64url(&random_bytes::<32>()?);
    let challenge = base64url(&sha256(verifier.as_bytes()));
    let state = base64url(&random_bytes::<16>()?);
    let url = format!(
        "https://{LOGIN_HOST}/{TENANT}/oauth2/v2.0/authorize?{}",
        form(&[
            ("client_id", &client_id),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            ("response_mode", "query"),
            ("scope", SIGN_IN_SCOPE),
            ("login_hint", user.trim()),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("prompt", "select_account"),
        ])
    );
    open_browser(&url)?;
    let code = wait_for_code_from(&listener, &state, "Outlook.com")?;
    let response = token_request(&[
        ("client_id", &client_id),
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect),
        ("code_verifier", &verifier),
        ("scope", SIGN_IN_SCOPE),
    ])?;
    let who = signed_in_as(response.id_token.as_deref(), &client_id)?;
    if who != user.trim().to_lowercase() {
        return Err(tr!(
            "Le navigateur a connecté le compte « {who} » et non « {user} ». \
             Recommencez en choisissant le bon compte sur la page de connexion.",
            who = who,
            user = user.trim()
        ));
    }
    let refresh =
        response.refresh_token.ok_or(tr!("Le service n'a pas fourni de jeton de renouvellement (autorisation offline_access ?)"))?;
    save(user, &refresh)?;
    cache_put(user, &response.access_token.unwrap_or_default(), response.expires_in.unwrap_or(3600));
    Ok(())
}

// ================================================================ requêtes

pub(crate) fn error_reason(status: u16, body: &[u8]) -> String {
    let detail = json::parse(&String::from_utf8_lossy(body)).ok().map_or_default(|v| v.get("error").get("message").text().to_string());
    let base = match status {
        401 => tr!("autorisation refusée par Outlook.com (reconnectez l'agenda et les contacts)").to_string(),
        403 => tr!("accès refusé par Outlook.com").to_string(),
        404 => tr!("élément introuvable sur Outlook.com").to_string(),
        412 => tr!("modifié ailleurs entre-temps, resynchronisez puis recommencez").to_string(),
        429 => tr!("trop de requêtes, réessayez dans un instant").to_string(),
        s => tr!("réponse HTTP {s} d'Outlook.com", s = s),
    };
    if detail.is_empty() {
        base
    } else {
        format!("{base} ({detail})")
    }
}

/// Requête à l'API ; la réponse est lue en JSON (`Null` si vide).
fn call(method: &str, url: &str, token: &str, body: Option<&Json>) -> Result<Json, String> {
    call_ex(method, url, token, body, "").map_err(|(_, e)| e)
}

/// Comme `call` ; l'erreur porte aussi le statut HTTP (0 : pas de réponse).
/// `version` (`@odata.etag` connu, peut être vide) part en `If-Match` : une
/// écriture ne remplace pas une modification faite ailleurs entre-temps
/// (RFC 9110 §13.1.1 ; relecture du 12 novembre, n° 38).
fn call_ex(method: &str, url: &str, token: &str, body: Option<&Json>, version: &str) -> Result<Json, (u16, String)> {
    call_sized(method, url, token, body, version).map(|(json, _)| json)
}

/// Comme `call_ex`, avec la taille de la réponse reçue (pour borner une liste
/// sans réécrire chaque page en texte, audit P7).
fn call_sized(method: &str, url: &str, token: &str, body: Option<&Json>, version: &str) -> Result<(Json, usize), (u16, String)> {
    // Le jeton ne part que vers l'API du service, même si une réponse
    // (`@odata.nextLink`) désigne une autre adresse (relecture du 10 novembre 2026).
    if !url.starts_with(&format!("{API}/")) {
        return Err((0, tr!("adresse hors de l'API d'Outlook.com refusée : {url}", url = url)));
    }
    let mut headers = vec![
        ("Authorization", format!("Bearer {token}")),
        ("Accept", "application/json".to_string()),
        // Heures en UTC et descriptions en texte brut.
        ("Prefer", PIM_PREFER_UTC.to_string()),
        ("Prefer", PIM_PREFER_TEXT.to_string()),
    ];
    if !version.is_empty() {
        headers.push(("If-Match", version.to_string()));
    }
    let payload = body.map(Json::to_text);
    if payload.is_some() {
        headers.push(("Content-Type", "application/json; charset=utf-8".to_string()));
    }
    let (response, _) =
        crate::http::send(method, url, &headers, payload.as_deref().map(str::as_bytes), MAX_RESPONSE).map_err(|e| (0, e))?;
    if !(200..300).contains(&response.status) {
        if response.status == 401 {
            token_refused(token);
        }
        return Err((response.status, error_reason(response.status, &response.body)));
    }
    let size = response.body.len();
    if response.body.iter().all(u8::is_ascii_whitespace) {
        return Ok((Json::Null, size));
    }
    json::parse(&String::from_utf8_lossy(&response.body)).map(|json| (json, size)).map_err(|e| (response.status, e))
}

/// Taille maximale d'un envoi par l'API (message MIME encodé en base64) :
/// l'API refuse les requêtes de plus de 4 Mo.
const MAX_SEND_BASE64: usize = 4 * 1000 * 1000;

/// Envoie un message MIME déjà construit par l'API (`/me/sendMail`). Le
/// service le range lui-même dans « Éléments envoyés ».
///
/// # Errors
///
/// Message trop volumineux, ou refus de l'API.
pub(crate) fn send_mime(token: &str, mime: &[u8]) -> Result<(), String> {
    let body = mail_core::compose::base64(mime);
    if body.len() > MAX_SEND_BASE64 {
        return Err(tr!(
            "Message trop volumineux pour l'envoi par l'API d'Outlook.com ({size} Mo, pièces jointes comprises ; 3 Mo au plus).",
            size = mime.len().div_ceil(1024 * 1024)
        ));
    }
    let url = format!("{API}/me/sendMail");
    let headers = [("Authorization", format!("Bearer {token}")), ("Content-Type", "text/plain".to_string())];
    let (response, _) = crate::http::send("POST", &url, &headers, Some(body.as_bytes()), MAX_RESPONSE)?;
    if (200..300).contains(&response.status) {
        Ok(())
    } else {
        Err(error_reason(response.status, &response.body))
    }
}

/// Tous les éléments d'une liste, pages suivantes comprises.
fn list_all(url: &str, token: &str) -> Result<Vec<Json>, String> {
    let mut out = Vec::new();
    let mut next = Some(url.to_string());
    let (mut pages, mut total) = (0, 0usize);
    while let Some(url) = next.take() {
        let (page, size) = call_sized("GET", &url, token, None, "").map_err(|(_, e)| e)?;
        total = total.saturating_add(size);
        if total > MAX_LIST_BYTES {
            return Err(tr!("liste trop volumineuse renvoyée par Outlook.com").into());
        }
        out.extend(page.get("value").items().iter().cloned());
        next = page.get("@odata.nextLink").as_str().map(str::to_string);
        pages += 1;
        if pages > 200 {
            break;
        }
    }
    Ok(out)
}

/// L'API encode les identifiants en base64 (`+`, `/`, `=`) : ils vont tels
/// quels dans le chemin, sauf `/` qui doit être protégé.
fn id_path(id: &str) -> String {
    id.replace('/', "%2F")
}

// ================================================================ conversions

/// `2026-11-04T10:00:00.0000000` → (`20261104`, `T100000`).
fn split_date_time(value: &str) -> Option<(String, String)> {
    let (date, time) = value.split_once('T').unwrap_or((value, "00:00:00"));
    let date: String = date.chars().filter(char::is_ascii_digit).collect();
    let time: String = time.chars().take_while(|c| *c != '.').filter(char::is_ascii_digit).collect();
    (date.len() == 8).then(|| (date, format!("T{:0<6}", time.chars().take(6).collect::<String>())))
}

/// Heure de l'API (demandée en UTC) → valeur iCalendar.
fn when_from(v: &Json, all_day: bool) -> When {
    let raw = v.get("dateTime").text();
    let zone = v.get("timeZone").text();
    let Some((date, time)) = split_date_time(raw) else { return When::default() };
    if all_day {
        return When { value: date, tzid: String::new() };
    }
    if zone.is_empty() || zone.eq_ignore_ascii_case("UTC") {
        When { value: format!("{date}{time}Z"), tzid: String::new() }
    } else {
        When { value: format!("{date}{time}"), tzid: zone.to_string() }
    }
}

/// Valeur iCalendar → heure de l'API `{dateTime, timeZone}`. Une journée
/// entière est écrite à minuit dans le fuseau `day_zone` (celui du système),
/// comme le demande l'API, et non en UTC (relecture du 12 novembre, n° 45).
fn when_to(w: &When, all_day: bool, day_zone: &str) -> Json {
    let v = w.value.trim();
    let digits: String = v.chars().filter(char::is_ascii_digit).collect();
    let (year, month, day) = (digits.get(0..4).unwrap_or("1970"), digits.get(4..6).unwrap_or("01"), digits.get(6..8).unwrap_or("01"));
    let (hour, minute, second) =
        if all_day || digits.len() < 14 { ("00", "00", "00") } else { (&digits[8..10], &digits[10..12], &digits[12..14]) };
    let zone = if all_day {
        if day_zone.is_empty() {
            "UTC"
        } else {
            day_zone
        }
    } else if v.ends_with('Z') || w.tzid.is_empty() {
        "UTC"
    } else {
        w.tzid.as_str()
    };
    Json::obj([("dateTime", format!("{year}-{month}-{day}T{hour}:{minute}:{second}").into()), ("timeZone", zone.into())])
}

/// Couleurs d'agenda prédéfinies (`color`) quand `hexColor` est vide.
fn calendar_color(c: &Json) -> String {
    let hex = c.get("hexColor").text();
    if hex.len() == 7 && hex.starts_with('#') {
        return hex.to_string();
    }
    match c.get("color").text() {
        "lightBlue" => "#4f8edc",
        "lightGreen" => "#57a64a",
        "lightOrange" => "#e8853a",
        "lightGray" => "#8a8a8a",
        "lightYellow" => "#d9b51c",
        "lightTeal" => "#2aa198",
        "lightPink" => "#d16ba5",
        "lightBrown" => "#a0785a",
        "lightRed" => "#d9534f",
        _ => "",
    }
    .to_string()
}

/// Réponse d'un invité dans l'API (`status.response`) en `PARTSTAT` iCalendar.
fn graph_partstat(response: &str) -> &'static str {
    match response {
        "accepted" | "organizer" => "ACCEPTED",
        "tentativelyAccepted" => "TENTATIVE",
        "declined" => "DECLINED",
        _ => "NEEDS-ACTION",
    }
}

/// Répond à une invitation de l'agenda (lot H1b) : `action` vaut `accept`,
/// `tentativelyAccept` ou `decline` ; `send` : réponse envoyée à l'organisateur.
/// Un refus retire le rendez-vous de l'agenda Outlook.com.
pub(crate) fn respond(token: &str, event_url: &str, action: &str, send: bool) -> Result<(), String> {
    call("POST", &format!("{event_url}/{action}"), token, Some(&Json::obj([("sendResponse", send.into())]))).map(|_| ())
}

pub(crate) fn event_from(v: &Json, account: &str, calendar: &str) -> Option<Event> {
    let id = v.get("id").as_str()?;
    if v.get("isCancelled").as_bool() {
        return None;
    }
    let all_day = v.get("isAllDay").as_bool();
    let url = format!("{API}/me/events/{}", id_path(id));
    Some(Event {
        id: format!("{account}|{url}"),
        source: account.to_string(),
        calendar: calendar.to_string(),
        uid: v.get("iCalUId").as_str().unwrap_or(id).to_string(),
        summary: v.get("subject").text().to_string(),
        location: v.get("location").get("displayName").text().to_string(),
        description: v.get("body").get("content").text().trim().to_string(),
        start: when_from(v.get("start"), all_day),
        end: when_from(v.get("end"), all_day),
        all_day,
        organizer: {
            let who = v.get("organizer").get("emailAddress");
            if who.get("name").text().trim().is_empty() { who.get("address").text() } else { who.get("name").text() }.to_string()
        },
        organizer_email: v.get("organizer").get("emailAddress").get("address").text().to_string(),
        attendees: v
            .get("attendees")
            .items()
            .iter()
            .take(500)
            .map(|a| crate::pimfmt::Attendee {
                email: a.get("emailAddress").get("address").text().to_string(),
                name: a.get("emailAddress").get("name").text().to_string(),
                partstat: graph_partstat(a.get("status").get("response").text()).to_string(),
            })
            .collect(),
        reminder: if v.get("isReminderOn").as_bool() { v.get("reminderMinutesBeforeStart").as_i64() } else { None },
        ..Default::default()
    })
}

/// Champs téléphoniques de l'API dans l'ordre : mobile, bureau, domicile.
fn phone_slots(v: &Json) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(m) = v.get("mobilePhone").as_str().filter(|m| !m.is_empty()) {
        out.push(("mobilePhone", m.to_string()));
    }
    for field in ["businessPhones", "homePhones"] {
        for p in v.get(field).items() {
            if let Some(p) = p.as_str().filter(|p| !p.is_empty()) {
                out.push((field, p.to_string()));
            }
        }
    }
    out
}

pub(crate) fn contact_from(v: &Json, account: &str) -> Option<Contact> {
    let id = v.get("id").as_str()?;
    let url = format!("{API}/me/contacts/{}", id_path(id));
    let emails: Vec<String> = v
        .get("emailAddresses")
        .items()
        .iter()
        .filter_map(|e| e.get("address").as_str().filter(|a| !a.is_empty()).map(str::to_string))
        .collect();
    let mut name = v.get("displayName").text().trim().to_string();
    if name.is_empty() {
        name = format!("{} {}", v.get("givenName").text(), v.get("surname").text()).trim().to_string();
    }
    if name.is_empty() {
        name = emails.first().cloned().unwrap_or_default();
    }
    Some(Contact {
        id: format!("{account}|{url}"),
        source: account.to_string(),
        uid: id.to_string(),
        name,
        given_name: v.get("givenName").text().trim().to_string(),
        family_name: v.get("surname").text().trim().to_string(),
        emails,
        phones: phone_slots(v).into_iter().map(|(_, p)| p).collect(),
        organization: v.get("companyName").text().to_string(),
        note: v.get("personalNotes").text().to_string(),
    })
}

/// Contact → champs de l'API. Les numéros gardent leur type (mobile, bureau,
/// domicile) dans l'ordre d'origine ; les nouveaux vont en domicile.
pub(crate) fn contact_body(c: &Contact, before: Option<&Json>) -> Json {
    let slots: Vec<&str> = before.map_or_default(|b| phone_slots(b).into_iter().map(|(f, _)| f).collect());
    let mut mobile = String::new();
    let mut business: Vec<Json> = Vec::new();
    let mut home: Vec<Json> = Vec::new();
    for (i, phone) in c.phones.iter().filter(|p| !p.trim().is_empty()).enumerate() {
        let slot = slots.get(i).copied().unwrap_or(if i == 0 && slots.is_empty() { "mobilePhone" } else { "homePhones" });
        match slot {
            "mobilePhone" if mobile.is_empty() => mobile = phone.trim().to_string(),
            "businessPhones" => business.push(phone.trim().into()),
            _ => home.push(phone.trim().into()),
        }
    }
    let emails: Vec<Json> = c
        .emails
        .iter()
        .filter(|e| !e.trim().is_empty())
        .map(|e| Json::obj([("address", e.trim().into()), ("name", c.name.trim().into())]))
        .collect();
    // Prénom et nom tels quels : rien n'est déduit du nom affiché (relecture du 12 novembre, n° 43).
    Json::obj([
        ("displayName", c.name.trim().into()),
        ("givenName", c.given_name.trim().into()),
        ("surname", c.family_name.trim().into()),
        ("companyName", c.organization.trim().into()),
        ("personalNotes", c.note.clone().into()),
        ("emailAddresses", emails.into()),
        ("mobilePhone", if mobile.is_empty() { Json::Null } else { mobile.into() }),
        ("businessPhones", business.into()),
        ("homePhones", home.into()),
    ])
}

const WEEKDAYS: [(&str, &str); 7] =
    [("MO", "monday"), ("TU", "tuesday"), ("WE", "wednesday"), ("TH", "thursday"), ("FR", "friday"), ("SA", "saturday"), ("SU", "sunday")];

/// Jour de la semaine (0 = lundi) d'une date civile.
fn weekday(y: i64, m: i64, d: i64) -> usize {
    // Algorithme de Sakamoto (0 = dimanche), ramené au lundi.
    const T: [i64; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    let sunday_based = (y + y / 4 - y / 100 + y / 400 + T[usize::try_from(m - 1).unwrap_or_default()] + d).rem_euclid(7);
    usize::try_from((sunday_based + 6) % 7).unwrap_or_default()
}

/// Règle iCalendar → répétition de l'API (`patternedRecurrence`).
///
/// `None` : pas de répétition, ou règle que le service ne sait pas représenter
/// à l'identique. L'appelant refuse alors l'enregistrement avec un message
/// clair, au lieu de créer une autre répétition sans prévenir (relecture A5).
pub(crate) fn recurrence_from_rule(rule: &str, start: &When) -> Option<Json> {
    if rule.trim().is_empty() {
        return None;
    }
    let mut parts: HashMap<String, String> = HashMap::new();
    for part in rule.split(';').filter(|p| !p.trim().is_empty()) {
        let (key, value) = part.split_once('=')?;
        let key = key.trim().to_ascii_uppercase();
        // Seules les parties que le service sait représenter (pas de BYSETPOS,
        // BYWEEKNO…) ; une partie donnée deux fois est refusée aussi.
        if !matches!(key.as_str(), "FREQ" | "INTERVAL" | "COUNT" | "UNTIL" | "BYDAY" | "BYMONTHDAY" | "BYMONTH" | "WKST") {
            return None;
        }
        if parts.insert(key, value.trim().to_ascii_uppercase()).is_some() {
            return None;
        }
    }
    let digits: String = start.value.chars().filter(char::is_ascii_digit).collect();
    let (y, m, d): (i64, i64, i64) = (digits.get(0..4)?.parse().ok()?, digits.get(4..6)?.parse().ok()?, digits.get(6..8)?.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let interval: i64 = match parts.get("INTERVAL") {
        Some(i) => i.parse().ok().filter(|n| *n >= 1)?,
        None => 1,
    };
    let start_day = WEEKDAYS[weekday(y, m, d)].1;
    let by_day: Vec<(Option<i64>, &str)> = match parts.get("BYDAY") {
        Some(list) => list.split(',').map(by_day_entry).collect::<Option<_>>()?,
        None => Vec::new(),
    };
    let month_day = single_number(&parts, "BYMONTHDAY", 31).ok()?;
    let month = single_number(&parts, "BYMONTH", 12).ok()?;
    let plain_days = by_day.iter().all(|(index, _)| index.is_none());
    let day_names = |list: &[(Option<i64>, &str)]| -> Json { list.iter().map(|(_, name)| Json::from(*name)).collect::<Vec<_>>().into() };
    let first_day = match parts.get("WKST") {
        Some(code) => WEEKDAYS.iter().find(|(c, _)| c == code)?.1,
        None => "monday",
    };

    let freq = parts.get("FREQ").map(String::as_str)?;
    let pattern = match freq {
        "DAILY" if by_day.is_empty() && month_day.is_none() && month.is_none() => {
            Json::obj([("type", "daily".into()), ("interval", interval.into())])
        }
        // « Tous les jours ouvrés » (DAILY avec BYDAY) : chaque semaine, ces jours-là.
        "DAILY" | "WEEKLY" if month_day.is_none() && month.is_none() && plain_days && (freq == "WEEKLY" || interval == 1) => {
            let days = if by_day.is_empty() { vec![Json::from(start_day)].into() } else { day_names(&by_day) };
            Json::obj([
                ("type", "weekly".into()),
                ("interval", interval.into()),
                ("daysOfWeek", days),
                ("firstDayOfWeek", first_day.into()),
            ])
        }
        "MONTHLY" if month.is_none() => match (by_day.as_slice(), month_day) {
            ([], day) => {
                Json::obj([("type", "absoluteMonthly".into()), ("interval", interval.into()), ("dayOfMonth", day.unwrap_or(d).into())])
            }
            // « 2MO » : 2e lundi ; « 1MO,1WE » : 1er lundi et 1er mercredi.
            // Des rangs différents (« 1MO,3MO ») ne se représentent pas.
            (list, None) => Json::obj([
                ("type", "relativeMonthly".into()),
                ("interval", interval.into()),
                ("daysOfWeek", day_names(list)),
                ("index", same_index(list)?.into()),
            ]),
            _ => return None,
        },
        "YEARLY" => match (by_day.as_slice(), month_day) {
            ([], day) => Json::obj([
                ("type", "absoluteYearly".into()),
                ("interval", interval.into()),
                ("dayOfMonth", day.unwrap_or(d).into()),
                ("month", month.unwrap_or(m).into()),
            ]),
            // « 4e jeudi de novembre » : il faut le mois (sinon c'est chaque jeudi de l'année).
            (list, None) if month.is_some() => Json::obj([
                ("type", "relativeYearly".into()),
                ("interval", interval.into()),
                ("daysOfWeek", day_names(list)),
                ("index", same_index(list)?.into()),
                ("month", month.unwrap_or(m).into()),
            ]),
            _ => return None,
        },
        _ => return None,
    };
    let range = recurrence_range(&parts, &format!("{y:04}-{m:02}-{d:02}"), start)?;
    Some(Json::obj([("pattern", pattern), ("range", range)]))
}

/// Fin de la répétition : nombre d'occurrences, date de fin, ou aucune.
/// `None` : les deux à la fois, ou valeur illisible.
///
/// L'API lit `endDate` comme une date du fuseau du rendez-vous, alors qu'un
/// `UNTIL` avec heure est en UTC : il est converti dans le fuseau du début
/// (à l'ouest de l'UTC, sa date UTC est celle du lendemain ; relecture du
/// 12 novembre, n° 37).
fn recurrence_range(parts: &HashMap<String, String>, start_date: &str, start: &When) -> Option<Json> {
    let range = match (parts.get("COUNT"), parts.get("UNTIL")) {
        (Some(_), Some(_)) => return None,
        (Some(count), None) => {
            let count: i64 = count.parse().ok().filter(|n| *n >= 1)?;
            Json::obj([("type", "numbered".into()), ("startDate", start_date.into()), ("numberOfOccurrences", count.into())])
        }
        (None, Some(until)) => {
            let u: String = until.chars().filter(char::is_ascii_digit).take(8).collect();
            if u.len() != 8 {
                return None;
            }
            let end_date = if until.len() > 8 {
                let zone = if start.value.ends_with('Z') { "" } else { start.tzid.as_str() };
                let second = crate::tz::instant(&When { value: until.clone(), tzid: String::new() })?;
                crate::tz::civil_date(second, zone)?
            } else {
                format!("{}-{}-{}", &u[0..4], &u[4..6], &u[6..8])
            };
            Json::obj([("type", "endDate".into()), ("startDate", start_date.into()), ("endDate", end_date.into())])
        }
        (None, None) => Json::obj([("type", "noEnd".into()), ("startDate", start_date.into())]),
    };
    Some(range)
}

/// Un jour de BYDAY : son rang éventuel (1 à 4, ou -1 pour le dernier) et
/// son nom pour l'API. `None` : jour inconnu ou rang que le service n'a pas.
fn by_day_entry(entry: &str) -> Option<(Option<i64>, &'static str)> {
    let entry = entry.trim();
    // Découpe sûre : la valeur vient d'une invitation reçue et peut contenir
    // n'importe quel caractère.
    if !entry.is_ascii() || entry.len() < 2 {
        return None;
    }
    let (rank, code) = entry.split_at(entry.len() - 2);
    let name = WEEKDAYS.iter().find(|(c, _)| *c == code)?.1;
    if rank.is_empty() {
        return Some((None, name));
    }
    let rank: i64 = rank.strip_prefix('+').unwrap_or(rank).parse().ok()?;
    matches!(rank, 1..=4 | -1).then_some((Some(rank), name))
}

/// Rang commun à tous les jours (« first »…« last ») ; `None` s'ils n'en ont
/// pas ou s'ils en ont plusieurs.
fn same_index(list: &[(Option<i64>, &str)]) -> Option<&'static str> {
    let first = list.first()?.0?;
    if list.iter().any(|(rank, _)| *rank != Some(first)) {
        return None;
    }
    match first {
        1 => Some("first"),
        2 => Some("second"),
        3 => Some("third"),
        4 => Some("fourth"),
        -1 => Some("last"),
        _ => None,
    }
}

/// Valeur entière unique d'une partie de la règle, entre 1 et `max` :
/// `Ok(None)` si la partie est absente, `Err` si elle ne convient pas
/// (plusieurs valeurs, valeur négative ou hors limites).
fn single_number(parts: &HashMap<String, String>, key: &str, max: i64) -> Result<Option<i64>, ()> {
    match parts.get(key) {
        None => Ok(None),
        Some(v) => v.parse::<i64>().ok().filter(|n| (1..=max).contains(n)).map(Some).ok_or(()),
    }
}

/// Champs d'un rendez-vous pour l'API. `before` : version en cache (JSON de
/// l'API). Pour une modification, seuls les champs changés sont envoyés : la
/// description, lue en texte brut, ne remplace pas le texte mis en forme
/// (ni le lien d'une réunion en ligne) quand seul l'horaire change, et le
/// lieu garde son adresse (relecture du 12 novembre, n° 38). `day_zone` :
/// fuseau des journées entières.
pub(crate) fn event_body(e: &Event, recurrence: Option<Json>, before: Option<&Json>, day_zone: &str) -> Json {
    let old = before.and_then(|b| event_from(b, "", ""));
    let changed = |same: bool| old.is_none() || !same;
    let mut end = e.end.clone();
    if end.value.is_empty() {
        end = e.start.clone();
    }
    let mut body = Json::obj([]);
    if changed(old.as_ref().is_some_and(|o| o.summary.trim() == e.summary.trim())) {
        body.set("subject", e.summary.trim().into());
    }
    if changed(old.as_ref().is_some_and(|o| o.description.trim() == e.description.trim())) {
        body.set("body", Json::obj([("contentType", "text".into()), ("content", e.description.clone().into())]));
    }
    if changed(old.as_ref().is_some_and(|o| o.location.trim() == e.location.trim())) {
        body.set("location", Json::obj([("displayName", e.location.trim().into())]));
    }
    let same_time = |o: &Event| {
        let same = |a: &When, b: &When| crate::tz::same_instant(a, b);
        o.all_day == e.all_day && same(&o.start, &e.start) && (same(&o.end, &end) || (o.end.value.is_empty() && e.end.value.is_empty()))
    };
    if changed(old.as_ref().is_some_and(same_time)) {
        body.set("isAllDay", e.all_day.into());
        body.set("start", when_to(&e.start, e.all_day, day_zone));
        body.set("end", when_to(&end, e.all_day, day_zone));
    }
    if changed(old.as_ref().is_some_and(|o| o.reminder == e.reminder)) {
        body.set("isReminderOn", e.reminder.is_some().into());
        if let Some(minutes) = e.reminder {
            body.set("reminderMinutesBeforeStart", minutes.max(0).into());
        }
    }
    if let Some(r) = recurrence {
        body.set("recurrence", r);
    }
    body
}

// ================================================================ synchronisation

fn iso(stamp: &str) -> String {
    // `20251103T000000Z` → `2025-11-03T00:00:00Z`
    format!("{}-{}-{}T{}:{}:{}Z", &stamp[0..4], &stamp[4..6], &stamp[6..8], &stamp[9..11], &stamp[11..13], &stamp[13..15])
}

fn sync_calendars(cache: &Arc<Mutex<Cache>>, c: &AccountConfig, token: &str, from: &str, to: &str) -> Result<u32, String> {
    let calendars = list_all(&format!("{API}/me/calendars?$top=100&$select=id,name,color,hexColor"), token)?;
    let stored: Vec<PimCollection> = calendars
        .iter()
        .filter_map(|cal| {
            Some(PimCollection {
                account: c.id.clone(),
                url: format!("{API}/me/calendars/{}", id_path(cal.get("id").as_str()?)),
                kind: "calendar".into(),
                name: cal.get("name").text().to_string(),
                color: calendar_color(cal),
            })
        })
        .collect();
    lock_cache(cache).pim_set_collections(&c.id, "calendar", &stored)?;
    let mut count = 0;
    for col in &stored {
        let url = format!(
            "{}/calendarView?startDateTime={}&endDateTime={}&$top=500&$select=id,iCalUId,subject,body,location,start,end,isAllDay,isCancelled,type,organizer,attendees,isReminderOn,reminderMinutesBeforeStart",
            col.url,
            iso(from),
            iso(to)
        );
        let rows: Vec<PimItem> = list_all(&url, token)?
            .iter()
            .filter_map(|v| {
                let e = event_from(v, &c.id, &col.url)?;
                count += 1;
                let url = e.id.split_once('|').map_or_default(|(_, u)| u.to_string());
                Some(PimItem {
                    id: e.id.clone(),
                    url,
                    etag: v.get("@odata.etag").text().to_string(),
                    raw: v.to_text(),
                    data: serde_json::to_string(&vec![e]).unwrap_or_else(|_| "[]".into()),
                })
            })
            .collect();
        lock_cache(cache).pim_replace_items(&c.id, "calendar", &col.url, &rows)?;
    }
    Ok(count)
}

const CONTACT_FIELDS: &str =
    "$top=500&$select=id,displayName,givenName,surname,emailAddresses,mobilePhone,businessPhones,homePhones,companyName,personalNotes";

fn sync_contacts(cache: &Arc<Mutex<Cache>>, c: &AccountConfig, token: &str) -> Result<u32, String> {
    let mut books = vec![PimCollection {
        account: c.id.clone(),
        url: format!("{API}/me/contacts"),
        kind: "contacts".into(),
        name: tr!("Contacts").into(),
        color: String::new(),
    }];
    for folder in list_all(&format!("{API}/me/contactFolders?$top=100"), token)? {
        let Some(id) = folder.get("id").as_str() else { continue };
        books.push(PimCollection {
            account: c.id.clone(),
            url: format!("{API}/me/contactFolders/{}/contacts", id_path(id)),
            kind: "contacts".into(),
            name: folder.get("displayName").text().to_string(),
            color: String::new(),
        });
    }
    lock_cache(cache).pim_set_collections(&c.id, "contacts", &books)?;
    let mut count = 0;
    for book in &books {
        let rows: Vec<PimItem> = list_all(&format!("{}?{CONTACT_FIELDS}", book.url), token)?
            .iter()
            .filter_map(|v| {
                let contact = contact_from(v, &c.id)?;
                count += 1;
                let url = contact.id.split_once('|').map_or_default(|(_, u)| u.to_string());
                Some(PimItem {
                    id: contact.id.clone(),
                    url,
                    etag: v.get("@odata.etag").text().to_string(),
                    raw: v.to_text(),
                    data: serde_json::to_string(&contact).unwrap_or_else(|_| "null".into()),
                })
            })
            .collect();
        lock_cache(cache).pim_replace_items(&c.id, "contacts", &book.url, &rows)?;
    }
    Ok(count)
}

/// Relit agendas et carnets du compte ; `(contacts, rendez-vous)`.
pub(crate) fn sync(cache: &Arc<Mutex<Cache>>, c: &AccountConfig, token: &str, from: &str, to: &str) -> Result<(u32, u32), String> {
    let mut errors = Vec::new();
    let contacts = sync_contacts(cache, c, token).unwrap_or_else(|e| {
        errors.push(tr!("contacts : {e}", e = e));
        0
    });
    let events = sync_calendars(cache, c, token, from, to).unwrap_or_else(|e| {
        errors.push(tr!("agenda : {e}", e = e));
        0
    });
    if errors.is_empty() {
        Ok((contacts, events))
    } else {
        Err(errors.join("\n"))
    }
}

// ================================================================ écriture

/// Crée (`existing` absent) ou modifie un élément ; renvoie (adresse, version, JSON brut).
pub(crate) fn put(
    token: &str,
    kind: &str,
    collection: &str,
    existing: Option<&PimItem>,
    body: &Json,
) -> Result<(String, String, String), String> {
    let saved = if let Some(item) = existing {
        call_ex("PATCH", &item.url, token, Some(body), &item.etag).map_err(|(_, e)| e)?
    } else {
        let target = if kind == "calendar" { format!("{collection}/events") } else { collection.to_string() };
        call("POST", &target, token, Some(body))?
    };
    let id = saved.get("id").as_str().ok_or(tr!("réponse d'Outlook.com sans identifiant"))?;
    let base = if kind == "calendar" { "events" } else { "contacts" };
    Ok((format!("{API}/me/{base}/{}", id_path(id)), saved.get("@odata.etag").text().to_string(), saved.to_text()))
}

/// Supprime un élément ; `version` (`@odata.etag` connu) part en `If-Match`.
pub(crate) fn delete(token: &str, url: &str, version: &str) -> Result<(), String> {
    match call_ex("DELETE", url, token, None, version) {
        // 404 : déjà supprimé sur le serveur.
        Ok(_) | Err((404, _)) => Ok(()),
        Err((_, e)) => Err(e),
    }
}

/// Supprime une occurrence d'une série que Vortex Mail vient de créer et qui
/// n'a pas encore été relue (seule la série est en cache) : l'API n'a pas
/// d'équivalent d'`EXDATE`, l'occurrence est retrouvée parmi celles de la
/// série (`instances`) et supprimée (relecture du 12 novembre, n° 45).
/// `wanted` : début d'origine de l'occurrence (valeur de sa date exclue).
pub(crate) fn delete_occurrence(token: &str, series_url: &str, wanted: &When, day_zone: &str) -> Result<(), String> {
    let second = crate::tz::instant(wanted).ok_or(tr!("occurrence introuvable sur Outlook.com : resynchronisez puis recommencez"))?;
    // Journée entière : comparée par sa date dans le fuseau des journées entières.
    let v = wanted.value.as_str();
    let day = match (wanted.is_date(), v.get(0..4), v.get(4..6), v.get(6..8)) {
        (true, Some(y), Some(m), Some(d)) => Some(format!("{y}-{m}-{d}")),
        _ => None,
    };
    let stamp = crate::pim::utc_stamp;
    let url = format!(
        "{series_url}/instances?startDateTime={}&endDateTime={}&$select=id,originalStart,start",
        iso(&stamp(second - 86_400)),
        iso(&stamp(second + 86_400))
    );
    for instance in list_all(&url, token)? {
        let original = instance.get("originalStart").text();
        let start = if original.is_empty() { instance.get("start").get("dateTime").text() } else { original };
        let Some((date, time)) = split_date_time(start) else { continue };
        let at = crate::tz::instant(&When { value: format!("{date}{time}Z"), tzid: String::new() });
        let found = match &day {
            Some(day) => at.and_then(|at| crate::tz::civil_date(at, day_zone)).as_ref() == Some(day),
            None => at == Some(second),
        };
        if found {
            let Some(id) = instance.get("id").as_str() else { continue };
            return delete(token, &format!("{API}/me/events/{}", id_path(id)), "");
        }
    }
    Err(tr!("occurrence introuvable sur Outlook.com : resynchronisez puis recommencez").into())
}

/// Série enregistrée par Vortex Mail (réponse de l'API à sa création) : elle
/// porte sa répétition, contrairement aux occurrences lues par `calendarView`.
pub(crate) fn is_series_master(raw: &str) -> bool {
    json::parse(raw).is_ok_and(|v| v.get("type").text() == "seriesMaster" || !v.get("recurrence").is_null())
}

/// L'élément brut est une occurrence d'une série (modifiable seule).
pub(crate) fn is_series_instance(raw: &str) -> bool {
    json::parse(raw).is_ok_and(|v| matches!(v.get("type").text(), "occurrence" | "exception"))
}

pub(crate) fn parse_raw(raw: &str) -> Option<Json> {
    json::parse(raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_from_api() {
        let v = json::parse(
            r#"{"id":"AAMk/x=","iCalUId":"040000","subject":"Dentiste","isAllDay":false,"isCancelled":false,
                "type":"occurrence","body":{"contentType":"text","content":" Apporter la carte \r\n"},
                "location":{"displayName":"Aix"},
                "start":{"dateTime":"2026-11-04T10:00:00.0000000","timeZone":"UTC"},
                "end":{"dateTime":"2026-11-04T10:30:00.0000000","timeZone":"UTC"},
                "isReminderOn":true,"reminderMinutesBeforeStart":30,
                "organizer":{"emailAddress":{"name":"F","address":"f@example.org"}}}"#,
        )
        .unwrap();
        let e = event_from(&v, "acct", "cal").unwrap();
        assert_eq!(e.reminder, Some(30));
        assert_eq!(e.id, format!("acct|{API}/me/events/AAMk%2Fx="));
        assert_eq!(e.start, When { value: "20261104T100000Z".into(), tzid: String::new() });
        assert_eq!(e.end.value, "20261104T103000Z");
        assert_eq!(e.description, "Apporter la carte");
        assert_eq!(e.location, "Aix");
        // Nom affiché de l'organisateur, son adresse à part (lot H1b).
        assert_eq!((e.organizer.as_str(), e.organizer_email.as_str()), ("F", "f@example.org"));
        assert!(is_series_instance(&v.to_text()));

        let all_day = json::parse(
            r#"{"id":"B","subject":"Férié","isAllDay":true,
                "start":{"dateTime":"2026-11-11T00:00:00.0000000","timeZone":"UTC"},
                "end":{"dateTime":"2026-11-12T00:00:00.0000000","timeZone":"UTC"}}"#,
        )
        .unwrap();
        let e = event_from(&all_day, "acct", "cal").unwrap();
        assert!(e.all_day);
        assert_eq!((e.start.value.as_str(), e.end.value.as_str()), ("20261111", "20261112"));

        let cancelled = json::parse(r#"{"id":"C","isCancelled":true}"#).unwrap();
        assert!(event_from(&cancelled, "acct", "cal").is_none());
    }

    #[test]
    fn event_to_api() {
        let e = Event {
            summary: "Réunion".into(),
            start: When { value: "20261104T110000".into(), tzid: "Europe/Paris".into() },
            end: When { value: "20261104T120000".into(), tzid: "Europe/Paris".into() },
            ..Default::default()
        };
        let body = event_body(&e, None, None, "Europe/Paris");
        assert_eq!(body.get("start").get("dateTime").text(), "2026-11-04T11:00:00");
        assert_eq!(body.get("start").get("timeZone").text(), "Europe/Paris");
        assert!(body.get("recurrence").is_null());
        assert!(!body.get("isReminderOn").as_bool());
        let reminded = event_body(&Event { reminder: Some(15), ..e }, None, None, "Europe/Paris");
        assert!(reminded.get("isReminderOn").as_bool());
        assert_eq!(reminded.get("reminderMinutesBeforeStart").as_i64(), Some(15));

        let day = Event {
            all_day: true,
            start: When { value: "20261111".into(), tzid: String::new() },
            end: When { value: "20261112".into(), tzid: String::new() },
            ..Default::default()
        };
        // Journée entière : minuit dans le fuseau du système, pas en UTC (n° 45).
        let body = event_body(&day, None, None, "Europe/Paris");
        assert_eq!(body.get("end").get("dateTime").text(), "2026-11-12T00:00:00");
        assert_eq!(body.get("end").get("timeZone").text(), "Europe/Paris");
        assert!(body.get("isAllDay").as_bool());
        assert_eq!(event_body(&day, None, None, "").get("end").get("timeZone").text(), "UTC");
    }

    #[test]
    fn modification_partielle() {
        // Relecture du 12 novembre, n° 38 : seuls les champs changés partent.
        let before = json::parse(
            r#"{"id":"E1","subject":"Revue","isAllDay":false,"type":"singleInstance",
                "body":{"contentType":"text","content":"Lien de connexion"},
                "location":{"displayName":"Salle 4","address":{"street":"1 rue"}},
                "start":{"dateTime":"2026-11-04T10:00:00.0000000","timeZone":"UTC"},
                "end":{"dateTime":"2026-11-04T11:00:00.0000000","timeZone":"UTC"},
                "isReminderOn":true,"reminderMinutesBeforeStart":15}"#,
        )
        .unwrap();
        let mut e = event_from(&before, "acct", "cal").unwrap();
        // Même horaire écrit à l'heure de Paris : rien ne change.
        e.start = When { value: "20261104T110000".into(), tzid: "Europe/Paris".into() };
        e.end = When { value: "20261104T120000".into(), tzid: "Europe/Paris".into() };
        let body = event_body(&e, None, Some(&before), "Europe/Paris");
        assert_eq!(body.to_text(), "{}");
        // Décalé d'une heure : horaire seul, ni texte ni lieu.
        e.start.value = "20261104T120000".into();
        e.end.value = "20261104T130000".into();
        let body = event_body(&e, None, Some(&before), "Europe/Paris");
        assert_eq!(body.get("start").get("dateTime").text(), "2026-11-04T12:00:00");
        assert!(body.get("body").is_null() && body.get("location").is_null() && body.get("subject").is_null());
        e.description = "Autre texte".into();
        assert_eq!(event_body(&e, None, Some(&before), "").get("body").get("content").text(), "Autre texte");
        assert!(is_series_master(r#"{"type":"seriesMaster"}"#));
        assert!(!is_series_master(&before.to_text()));
    }

    #[test]
    fn fin_de_serie_dans_le_fuseau_du_rendez_vous() {
        // Relecture du 12 novembre, n° 37 : « jusqu'au 6 novembre » à New York.
        let ny = When { value: "20261102T090000".into(), tzid: "America/New_York".into() };
        let r = recurrence_from_rule("FREQ=DAILY;UNTIL=20261107T045959Z", &ny).unwrap();
        assert_eq!(r.get("range").get("endDate").text(), "2026-11-06");
        // Invitation venue de Tokyo (8 h, dernière occurrence le 6).
        let tokyo = When { value: "20261102T080000".into(), tzid: "Asia/Tokyo".into() };
        let r = recurrence_from_rule("FREQ=DAILY;UNTIL=20261105T230000Z", &tokyo).unwrap();
        assert_eq!(r.get("range").get("endDate").text(), "2026-11-06");
        // Date seule : telle quelle.
        let r = recurrence_from_rule("FREQ=DAILY;UNTIL=20261106", &tokyo).unwrap();
        assert_eq!(r.get("range").get("endDate").text(), "2026-11-06");
    }

    #[test]
    fn recurrences() {
        // Mercredi 4 novembre 2026.
        let start = When { value: "20261104T110000".into(), tzid: "Europe/Paris".into() };
        let weekly = recurrence_from_rule("FREQ=WEEKLY", &start).unwrap();
        assert_eq!(weekly.get("pattern").get("type").text(), "weekly");
        assert_eq!(weekly.get("pattern").get("daysOfWeek").items()[0].text(), "wednesday");
        assert_eq!(weekly.get("range").get("type").text(), "noEnd");
        assert_eq!(weekly.get("range").get("startDate").text(), "2026-11-04");

        let r = recurrence_from_rule("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,FR;COUNT=6", &start).unwrap();
        assert_eq!(r.get("pattern").get("interval").as_i64(), Some(2));
        assert_eq!(r.get("pattern").get("daysOfWeek").items().len(), 2);
        assert_eq!(r.get("range").get("numberOfOccurrences").as_i64(), Some(6));

        let r = recurrence_from_rule("FREQ=MONTHLY;BYDAY=-1FR;UNTIL=20271231T000000Z", &start).unwrap();
        assert_eq!(r.get("pattern").get("type").text(), "relativeMonthly");
        assert_eq!(r.get("pattern").get("index").text(), "last");
        assert_eq!(r.get("range").get("endDate").text(), "2027-12-31");

        let r = recurrence_from_rule("FREQ=MONTHLY", &start).unwrap();
        assert_eq!(r.get("pattern").get("dayOfMonth").as_i64(), Some(4));
        let r = recurrence_from_rule("FREQ=YEARLY", &start).unwrap();
        assert_eq!((r.get("pattern").get("month").as_i64(), r.get("pattern").get("dayOfMonth").as_i64()), (Some(11), Some(4)));
        assert!(recurrence_from_rule("", &start).is_none());
        assert!(recurrence_from_rule("FREQ=HOURLY", &start).is_none());
        assert_eq!(weekday(2026, 11, 4), 2);
        assert_eq!(weekday(2024, 2, 29), 3);
    }

    #[test]
    fn complex_rules_are_refused_not_simplified() {
        // Relecture A5 : jamais une autre répétition que celle demandée.
        let start = When { value: "20261104T110000".into(), tzid: "Europe/Paris".into() };
        let rule = |r: &str| recurrence_from_rule(r, &start);
        for refused in [
            "FREQ=MONTHLY;BYDAY=1MO,3MO",          // deux rangs différents
            "FREQ=MONTHLY;BYDAY=MO",               // chaque lundi du mois
            "FREQ=MONTHLY;BYDAY=5MO",              // pas de 5e dans l'API
            "FREQ=MONTHLY;BYMONTHDAY=1,15",        // plusieurs jours du mois
            "FREQ=MONTHLY;BYMONTHDAY=-1",          // dernier jour du mois
            "FREQ=MONTHLY;BYDAY=1MO;BYMONTHDAY=7", // les deux à la fois
            "FREQ=MONTHLY;BYMONTH=3",
            "FREQ=YEARLY;BYDAY=MO", // chaque lundi de l'année
            "FREQ=YEARLY;BYMONTH=3,9",
            "FREQ=DAILY;INTERVAL=2;BYDAY=MO,WE",
            "FREQ=WEEKLY;BYDAY=1MO",
            "FREQ=WEEKLY;BYDAY=MO;BYSETPOS=1",
            "FREQ=WEEKLY;BYWEEKNO=20",
            "FREQ=WEEKLY;COUNT=3;UNTIL=20270101T000000Z",
            "FREQ=WEEKLY;COUNT=0",
            "FREQ=WEEKLY;INTERVAL=0",
            "FREQ=WEEKLY;FREQ=DAILY",
            "FREQ=WEEKLY;WKST=XX",
            "FREQ=WEEKLY;BYDAY",
        ] {
            assert!(rule(refused).is_none(), "{refused} aurait dû être refusée");
        }
    }

    #[test]
    fn hostile_values_do_not_panic() {
        let start = When { value: "20261104T110000".into(), tzid: String::new() };
        for hostile in
            ["FREQ=MONTHLY;BYDAY=1MÖ", "FREQ=WEEKLY;BYDAY=é", "FREQ=WEEKLY;BYDAY=ééMO", "FREQ=YEARLY;BYMONTH=99999999999999999999"]
        {
            assert!(recurrence_from_rule(hostile, &start).is_none());
        }
        // Date de début invalide : refusée au lieu de faire paniquer `weekday`.
        for value in ["20261304T110000", "20260004T110000", "20261100", "2026"] {
            let start = When { value: value.into(), tzid: String::new() };
            assert!(recurrence_from_rule("FREQ=WEEKLY", &start).is_none(), "{value}");
        }
    }

    #[test]
    fn supported_rules_are_kept_as_is() {
        let start = When { value: "20261104T110000".into(), tzid: "Europe/Paris".into() };
        let rule = |r: &str| recurrence_from_rule(r, &start).unwrap();

        let r = rule("FREQ=MONTHLY;BYDAY=1MO,1WE");
        assert_eq!(r.get("pattern").get("type").text(), "relativeMonthly");
        assert_eq!(r.get("pattern").get("index").text(), "first");
        assert_eq!(r.get("pattern").get("daysOfWeek").items().len(), 2);

        let r = rule("FREQ=MONTHLY;BYMONTHDAY=15");
        assert_eq!(r.get("pattern").get("dayOfMonth").as_i64(), Some(15));

        let r = rule("FREQ=YEARLY;BYMONTH=11;BYDAY=4TH");
        assert_eq!(r.get("pattern").get("type").text(), "relativeYearly");
        assert_eq!(r.get("pattern").get("index").text(), "fourth");
        assert_eq!(r.get("pattern").get("month").as_i64(), Some(11));
        assert_eq!(r.get("pattern").get("daysOfWeek").items()[0].text(), "thursday");

        let r = rule("FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=8");
        assert_eq!(r.get("pattern").get("type").text(), "absoluteYearly");
        assert_eq!((r.get("pattern").get("month").as_i64(), r.get("pattern").get("dayOfMonth").as_i64()), (Some(3), Some(8)));

        // Jours ouvrés : chaque semaine, du lundi au vendredi.
        let r = rule("FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR");
        assert_eq!(r.get("pattern").get("type").text(), "weekly");
        assert_eq!(r.get("pattern").get("daysOfWeek").items().len(), 5);

        let r = rule("freq=weekly;interval=2;wkst=su;byday=+1mo".replace("+1", "").as_str());
        assert_eq!(r.get("pattern").get("firstDayOfWeek").text(), "sunday");
        assert_eq!(r.get("pattern").get("interval").as_i64(), Some(2));

        let r = rule("FREQ=MONTHLY;BYDAY=+2TU;COUNT=10");
        assert_eq!(r.get("pattern").get("index").text(), "second");
    }

    #[test]
    fn contacts_both_ways() {
        let v = json::parse(
            r#"{"id":"K1","displayName":"","givenName":"Arthur","surname":"Conilh",
                "emailAddresses":[{"name":"A","address":"a@x.fr"},{"address":""}],
                "mobilePhone":"+33 6 00","businessPhones":["01 02"],"homePhones":[],
                "companyName":"ACME","personalNotes":"ami"}"#,
        )
        .unwrap();
        let c = contact_from(&v, "acct").unwrap();
        assert_eq!(c.name, "Arthur Conilh");
        assert_eq!(c.emails, vec!["a@x.fr"]);
        assert_eq!(c.phones, vec!["+33 6 00", "01 02"]);

        // Les numéros gardent leur type ; un numéro ajouté va en domicile.
        let edited = Contact { phones: vec!["+33 6 11".into(), "01 03".into(), "04 05".into()], ..c };
        let body = contact_body(&edited, Some(&v));
        assert_eq!(body.get("mobilePhone").text(), "+33 6 11");
        assert_eq!(body.get("businessPhones").items()[0].text(), "01 03");
        assert_eq!(body.get("homePhones").items()[0].text(), "04 05");
        assert_eq!(body.get("emailAddresses").items()[0].get("address").text(), "a@x.fr");

        // Nouveau contact : premier numéro en mobile.
        let fresh = Contact { name: "Zoé".into(), phones: vec!["06".into()], ..Default::default() };
        let body = contact_body(&fresh, None);
        assert_eq!(body.get("mobilePhone").text(), "06");
        assert_eq!(body.get("homePhones").items().len(), 0);
    }

    #[test]
    fn api_errors() {
        let body = br#"{"error":{"code":"ErrorAccessDenied","message":"Access is denied."}}"#;
        assert_eq!(error_reason(403, body), "accès refusé par Outlook.com (Access is denied.)");
        assert_eq!(error_reason(500, b""), "réponse HTTP 500 d'Outlook.com");
        assert_eq!(iso("20251103T000000Z"), "2025-11-03T00:00:00Z");
        assert_eq!(split_date_time("2026-11-04T09:05:00.0000000"), Some(("20261104".into(), "T090500".into())));
    }
}
