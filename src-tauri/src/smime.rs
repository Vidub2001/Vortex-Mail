//! S/MIME (RFC 8551), avec la crate `vortex-smime` : vérification des
//! signatures des messages reçus (lot D1, reprise de `mailcraft-smime`) et
//! signature des messages envoyés (lot D2).
//!
//! - Autorités de confiance **pour le courrier** : celles que le système range
//!   à part pour cet usage (paquet `email-ca-bundle.pem` de p11-kit sous Linux,
//!   magasin « Autorités de certification racines de confiance » sous Windows,
//!   limité à la messagerie sécurisée), jamais celles des sites web ; plus les
//!   certificats que l'utilisateur dépose dans le dossier `smime/autorites` de
//!   ses réglages (autorité d'entreprise, autorité de test). Lues une fois.
//! - Le résultat est calculé au téléchargement du message, à partir des octets
//!   reçus, et gardé dans le cache ; « Vérifier de nouveau » le recalcule.
//! - Un message signé « opaque » (`application/pkcs7-mime`) porte son contenu
//!   dans la signature : ce contenu est affiché à la place de la pièce
//!   `smime.p7m` (`shown_raw`).
//! - Signature (lot D2) : la clé et le certificat d'un compte sont importés
//!   d'un fichier PKCS #12. La clé privée va dans le trousseau du système (ou
//!   le coffre du mode portable), jamais dans un fichier ; les certificats,
//!   publics, et le choix « signer par défaut » dans `smime/comptes`. Un
//!   certificat n'est accepté que pour l'adresse du compte, valide et fait
//!   pour signer du courrier.
//! - Révocation (lot G2c) : les listes de certificats révoqués (CRL) des
//!   autorités sont téléchargées en arrière-plan aux adresses que donnent les
//!   certificats des signataires et des correspondants, gardées dans
//!   `smime/revocations` et relues à leur échéance ; elles sont consultées sans
//!   réseau à chaque vérification et avant d'écrire chiffré. Seule l'autorité
//!   apprend qu'on utilise ses certificats, jamais lesquels.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

use crate::config::AccountConfig;

use serde::{Deserialize, Serialize};
use vortex_smime::{Anchors, Context, Coverage, DigitalSignature, Revocation, Revocations, Signed, SignedKind, Verdict, Why};

/// Certificats pris au plus parmi les autorités (système et utilisateur).
const MAX_ANCHORS: usize = 4_000;
/// Taille d'un certificat d'autorité pris au plus.
const MAX_ANCHOR_BYTES: usize = 64 * 1024;
/// Taille d'un paquet de certificats lu au plus.
const MAX_BUNDLE_BYTES: u64 = 16 * 1024 * 1024;
/// Fichiers lus au plus dans le dossier des autorités de l'utilisateur.
const MAX_USER_FILES: usize = 200;

/// Où les distributions rangent les autorités de confiance pour le courrier.
#[cfg(all(unix, not(target_os = "macos")))]
const MAIL_BUNDLES: [&str; 3] = [
    // Arch, et les autres sur `trust extract-compat` de p11-kit.
    "/etc/ca-certificates/extracted/email-ca-bundle.pem",
    // Fedora, Red Hat, CentOS ; openSUSE.
    "/etc/pki/ca-trust/extracted/pem/email-ca-bundle.pem",
    "/var/lib/ca-certificates/email-ca-bundle.pem",
];

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Listes de certificats révoqués lues du disque, relues après un téléchargement (lot G2c).
static REVOCATIONS: RwLock<Option<Arc<Revocations>>> = RwLock::new(None);
/// Adresses de listes en cours de téléchargement, et dernier échec de chacune.
static FETCHING: Mutex<Vec<String>> = Mutex::new(Vec::new());
static FAILED: Mutex<Vec<(String, i64)>> = Mutex::new(Vec::new());
/// Listes téléchargées au plus par tour.
const MAX_CRL_URLS: usize = 64;
/// Après un échec, nouvel essai au plus tôt (secondes).
const CRL_RETRY: i64 = 3_600;
/// Liste sans échéance annoncée : relue au bout de (secondes).
const CRL_DEFAULT_AGE: u64 = 86_400;
static ANCHORS: OnceLock<Anchors> = OnceLock::new();

/// Dossier S/MIME des réglages (`<réglages>/smime`) ; le dossier des
/// autorités ajoutées par l'utilisateur y est créé, pour qu'il le trouve.
pub(crate) fn set_dir(dir: PathBuf) {
    let _ = std::fs::create_dir_all(dir.join("autorites"));
    let _ = DIR.set(dir);
    // Listes des autorités des correspondants : rafraîchies au démarrage (lot G2c), lues
    // hors du chemin du démarrage.
    std::thread::spawn(|| {
        let urls: Vec<String> = read_correspondents()
            .values()
            .filter_map(|c| mail_core::compose::base64_decode(&c.certificate).ok())
            .flat_map(|der| vortex_smime::cert_crl_urls(&der))
            .collect();
        fetch_revocations(urls, unix_now());
    });
}

/// Secondes depuis 1970.
fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Dossier des listes de certificats révoqués.
fn revocations_dir() -> Option<PathBuf> {
    DIR.get().map(|d| d.join("revocations"))
}

/// Fichier de la liste téléchargée à `url`.
fn revocation_file(url: &str) -> Option<PathBuf> {
    let name: String = url.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).take(150).collect();
    revocations_dir().map(|d| d.join(format!("{name}.crl")))
}

/// Les listes de certificats révoqués gardées sur cet ordinateur.
fn revocations() -> Arc<Revocations> {
    if let Some(known) = REVOCATIONS.read().unwrap_or_else(PoisonError::into_inner).as_ref() {
        return Arc::clone(known);
    }
    let max = u64::try_from(vortex_smime::MAX_CRL_BYTES).unwrap_or(u64::MAX);
    let lists: Vec<Vec<u8>> = revocations_dir()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "crl"))
        .take(vortex_smime::MAX_CRLS)
        .filter_map(|path| read_limited(&path, max))
        .collect();
    let loaded = Arc::new(Revocations::new(lists));
    *REVOCATIONS.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&loaded));
    loaded
}

/// La liste de `url` est-elle absente, ou à relire (échéance passée) ?
fn stale(url: &str, now: i64) -> bool {
    let Some(file) = revocation_file(url) else { return false };
    let max = u64::try_from(vortex_smime::MAX_CRL_BYTES).unwrap_or(u64::MAX);
    let Some(der) = read_limited(&file, max) else { return true };
    match vortex_smime::crl_next_update(&der) {
        Some(next) => next <= now,
        None => std::fs::metadata(&file)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_none_or(|age| age.as_secs() > CRL_DEFAULT_AGE),
    }
}

/// Télécharge en arrière-plan les listes de `urls` absentes ou à relire (lot G2c).
pub(crate) fn fetch_revocations(urls: Vec<String>, now: i64) {
    let wanted: Vec<String> = {
        let mut fetching = FETCHING.lock().unwrap_or_else(PoisonError::into_inner);
        let failed = FAILED.lock().unwrap_or_else(PoisonError::into_inner);
        let mut wanted = Vec::new();
        for url in urls {
            let recent_failure = failed.iter().any(|(u, at)| *u == url && now.saturating_sub(*at) < CRL_RETRY);
            if wanted.len() < MAX_CRL_URLS && !wanted.contains(&url) && !fetching.contains(&url) && !recent_failure && stale(&url, now) {
                fetching.push(url.clone());
                wanted.push(url);
            }
        }
        wanted
    };
    if wanted.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let mut changed = false;
        for url in &wanted {
            match download_revocations(url) {
                Ok(()) => changed = true,
                Err(e) => {
                    crate::diag::report(&format_args!("liste de révocation {url} : {e}"));
                    let mut failed = FAILED.lock().unwrap_or_else(PoisonError::into_inner);
                    failed.retain(|(u, _)| u != url);
                    failed.push((url.clone(), unix_now()));
                }
            }
        }
        FETCHING.lock().unwrap_or_else(PoisonError::into_inner).retain(|u| !wanted.contains(u));
        if changed {
            *REVOCATIONS.write().unwrap_or_else(PoisonError::into_inner) = None;
        }
    });
}

/// Télécharge la liste de `url` et la garde, si c'est bien une liste lisible. Souvent en
/// `http` : sans risque, la liste est signée par son autorité, vérifiée à la consultation.
fn download_revocations(url: &str) -> Result<(), String> {
    let file = revocation_file(url).ok_or_else(|| tr!("Dossier des réglages inconnu.").to_string())?;
    let response = crate::http::get(url, vortex_smime::MAX_CRL_BYTES)?;
    if response.status != 200 {
        return Err(format!("HTTP {}", response.status));
    }
    if Revocations::new([response.body.as_slice()]).is_empty() {
        return Err(tr!("liste illisible").into());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let temporary = file.with_extension("tmp");
    std::fs::write(&temporary, &response.body).map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, &file).map_err(|e| e.to_string())
}

/// Dossier des autorités ajoutées par l'utilisateur, s'il est connu.
fn user_dir() -> Option<PathBuf> {
    DIR.get().map(|d| d.join("autorites"))
}

fn anchors() -> &'static Anchors {
    ANCHORS.get_or_init(|| {
        let found: Vec<Vec<u8>> = system().into_iter().chain(user()).filter(|c| c.len() <= MAX_ANCHOR_BYTES).take(MAX_ANCHORS).collect();
        crate::diag::report(&tr!("{n} autorités de confiance pour le courrier (signatures S/MIME)", n = found.len()));
        Anchors::new(found)
    })
}

/// Certificats d'un texte PEM (`-----BEGIN CERTIFICATE-----`), en DER.
fn pem_certificates(text: &str) -> Vec<Vec<u8>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(BEGIN) {
        let after = &rest[start + BEGIN.len()..];
        let Some(end) = after.find(END) else { break };
        if let Ok(der) = mail_core::compose::base64_decode(&after[..end]) {
            out.push(der);
        }
        if out.len() >= MAX_ANCHORS {
            break;
        }
        rest = &after[end + END.len()..];
    }
    out
}

/// Contenu d'un fichier, s'il n'est pas plus gros que `max` octets.
fn read_limited(path: &Path, max: u64) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes).ok()?;
    (u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= max).then_some(bytes)
}

/// Certificats d'un fichier : PEM (un ou plusieurs) ou DER.
fn certificates_in(bytes: &[u8]) -> Vec<Vec<u8>> {
    match std::str::from_utf8(bytes) {
        Ok(text) if text.contains("-----BEGIN CERTIFICATE-----") => pem_certificates(text),
        _ => vec![bytes.to_vec()],
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system() -> Vec<Vec<u8>> {
    MAIL_BUNDLES.iter().find_map(|path| read_limited(Path::new(path), MAX_BUNDLE_BYTES)).map_or_default(|bytes| certificates_in(&bytes))
}

#[cfg(windows)]
fn system() -> Vec<Vec<u8>> {
    use schannel::cert_context::ValidUses;
    /// Messagerie sécurisée (id-kp-emailProtection, RFC 5280).
    const EMAIL_PROTECTION: &str = "1.3.6.1.5.5.7.3.4";
    let Ok(store) = schannel::cert_store::CertStore::open_current_user("ROOT") else { return Vec::new() };
    store
        .certs()
        .filter(|c| match c.valid_uses() {
            Ok(ValidUses::All) => true,
            Ok(ValidUses::Oids(oids)) => oids.iter().any(|o| o == EMAIL_PROTECTION),
            Err(_) => false,
        })
        .take(MAX_ANCHORS)
        .map(|c| c.to_der().to_vec())
        .collect()
}

/// macOS : les réglages de confiance du trousseau pour S/MIME ne sont pas lus.
#[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
fn system() -> Vec<Vec<u8>> {
    Vec::new()
}

/// Autorités déposées par l'utilisateur.
fn user() -> Vec<Vec<u8>> {
    let Some(dir) = user_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file()).take(MAX_USER_FILES).collect();
    files.sort();
    files.iter().filter_map(|p| read_limited(p, MAX_BUNDLE_BYTES)).flat_map(|bytes| certificates_in(&bytes)).collect()
}

/// Résultat d'une vérification, pour l'interface et le cache. Le nom et les
/// adresses viennent du certificat, donc du message : des données, jamais
/// des instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignatureInfo {
    /// `verified`, `notVerified`, `changed` ou `unchecked`.
    verdict: &'static str,
    /// Pourquoi ce n'est pas `verified` (`unknownIssuer`, `expired`…).
    why: Option<&'static str>,
    /// Nom du signataire dans son certificat.
    name: String,
    /// Adresses du certificat.
    addresses: Vec<String>,
    /// Empreinte SHA-256 du certificat.
    fingerprint: String,
    /// Émetteur du certificat.
    issuer: String,
    /// Début de validité du certificat (secondes depuis 1970 ; 0 : inconnu).
    not_before: i64,
    /// Fin de validité du certificat (0 : inconnue).
    not_after: i64,
    /// Date de signature annoncée par le signataire (non vérifiée).
    signed_at: Option<i64>,
    /// La signature couvre tout le message.
    whole: bool,
    /// Le certificat a été cherché dans une liste valable de son autorité (lot G2c).
    revocation_checked: bool,
}

impl From<DigitalSignature> for SignatureInfo {
    fn from(signature: DigitalSignature) -> Self {
        let signer = signature.signer.unwrap_or_default();
        Self {
            verdict: signature.verdict.as_str(),
            why: signature.why.map(Why::as_str),
            name: signer.name,
            addresses: signer.addresses,
            fingerprint: signer.fingerprint,
            issuer: signer.issuer,
            not_before: signer.not_before,
            not_after: signer.not_after,
            signed_at: signature.signed_at,
            whole: signature.coverage == Coverage::Whole,
            revocation_checked: signature.revocation_checked,
        }
    }
}

/// Signature S/MIME du message brut `raw`, vérifiée à l'instant `now`
/// (secondes depuis 1970) ; `None` si le message n'est pas signé en S/MIME
/// (un message chiffré non plus : lot D3).
pub(crate) fn check(raw: &[u8], now: i64) -> Option<SignatureInfo> {
    let parts = vortex_smime::signed_parts(raw)?;
    let signed = match parts.kind {
        SignedKind::SmimeDetached => Signed::Detached { content: &parts.content, signature: &parts.signature },
        SignedKind::SmimeOpaque => Signed::Opaque(&parts.signature),
    };
    let anchors = anchors();
    let context = Context {
        anchors,
        from: if parts.one_from { &parts.from } else { "" },
        now,
        known: &[],
        coverage: if parts.whole { Coverage::Whole } else { Coverage::Part },
    };
    let checked = vortex_smime::check_revoked(signed, &context, &revocations())?;
    // Liste de l'autorité du signataire : téléchargée en arrière-plan pour les prochaines fois.
    if let Some(certificate) = checked.certificate.as_deref() {
        fetch_revocations(vortex_smime::cert_crl_urls(certificate), now);
    }
    let mut signature = checked.signature;
    // Certificat du signataire, pour lui écrire chiffré (lot D3b) : seulement quand la
    // signature est la sienne et que son certificat est pour l'adresse `From`.
    if let Some(certificate) = checked.certificate.as_deref() {
        if parts.one_from && !matches!(signature.verdict, Verdict::Changed | Verdict::Unchecked) {
            remember(&parts.from, certificate, signature.verdict == Verdict::Verified, now);
        }
    }
    // Aucune autorité pour le courrier lisible ici : le dire, plutôt
    // qu'« émetteur inconnu » (Ubuntu et Debian n'en rangent aucune à part).
    if anchors.is_empty()
        && signature.verdict == Verdict::NotVerified
        && matches!(signature.why, Some(Why::UnknownIssuer | Why::SelfSigned))
    {
        signature.why = Some(Why::NoAuthorities);
    }
    Some(signature.into())
}

/// Colonne `signature` du cache : le résultat en JSON, ou
/// [`crate::cache::UNSIGNED`] pour un message non signé en S/MIME.
pub(crate) fn column(raw: &[u8], now: i64) -> String {
    check(raw, now).and_then(|s| serde_json::to_string(&s).ok()).unwrap_or_else(|| crate::cache::UNSIGNED.to_string())
}

/// Type MIME de la partie qui porte une signature S/MIME détachée
/// (`application/pkcs7-signature`, ou `x-pkcs7-signature` des anciens logiciels).
pub(crate) fn is_signature_part(content_type: &str) -> bool {
    let kind = content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    matches!(kind.as_str(), "application/pkcs7-signature" | "application/x-pkcs7-signature")
}

/// L'en-tête `Content-Type` d'un message annonce-t-il du S/MIME (signé ou
/// chiffré) ? Un message signé avec une clé PGP répond oui aussi : il est alors
/// téléchargé, puis trouvé non signé en S/MIME.
pub(crate) fn announced(header: &[u8]) -> bool {
    let text = String::from_utf8_lossy(header).to_ascii_lowercase();
    text.contains("multipart/signed") || text.contains("pkcs7-mime")
}

/// Message tel qu'il s'affiche : pour un message signé « opaque », ses
/// en-têtes, sans ceux qui décrivent la partie `pkcs7-mime`, suivis du
/// contenu que la signature porte ; `None` pour tout autre message.
pub(crate) fn opaque_content(raw: &[u8]) -> Option<Vec<u8>> {
    let parts = vortex_smime::signed_parts(raw)?;
    if parts.kind != SignedKind::SmimeOpaque || !parts.whole {
        return None;
    }
    let inner = vortex_smime::ber::signed_content(&parts.signature)?;
    Some(with_content(raw, &inner))
}

/// Le message `raw` avec `inner` pour contenu : ses en-têtes, sans ceux qui
/// décrivent sa partie S/MIME (`Content-*`), puis l'entité MIME `inner`.
fn with_content(raw: &[u8], inner: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(raw);
    let head_end = text.find("\r\n\r\n").map(|i| i + 2).or_else(|| text.find("\n\n").map(|i| i + 1)).unwrap_or(text.len());
    let mut out = Vec::with_capacity(head_end + inner.len());
    let mut skipping = false;
    for line in text[..head_end].split_inclusive('\n') {
        if line.starts_with([' ', '\t']) {
            if !skipping {
                out.extend_from_slice(line.as_bytes());
            }
            continue;
        }
        let name = line.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
        skipping = matches!(name.as_str(), "content-type" | "content-transfer-encoding" | "content-disposition" | "content-description");
        if !skipping {
            out.extend_from_slice(line.as_bytes());
        }
    }
    out.extend_from_slice(inner);
    out
}

// ------------------------------------------------------------------ messages chiffrés (lot D3a)

/// `Message::encryption` d'un message chiffré lu (en mémoire seulement).
pub(crate) const DECRYPTED: &str = "ok";

/// Le corps d'un message `application/pkcs7-mime` (le CMS), décodé.
fn pkcs7_body(raw: &[u8]) -> Option<Vec<u8>> {
    let content_type = mail_core::compose::header_value(raw, "Content-Type")?.to_ascii_lowercase();
    if !content_type.contains("pkcs7-mime") {
        return None;
    }
    let start = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|i| i + 2))?;
    let body = raw.get(start..)?;
    let encoding = mail_core::compose::header_value(raw, "Content-Transfer-Encoding").unwrap_or_default().to_ascii_lowercase();
    if encoding.contains("base64") {
        mail_core::compose::base64_decode(&String::from_utf8_lossy(body)).ok()
    } else {
        Some(body.to_vec())
    }
}

/// Le message `raw` est-il chiffré en S/MIME (`EnvelopedData`, `AuthEnvelopedData`) ?
pub(crate) fn is_encrypted(raw: &[u8]) -> bool {
    pkcs7_body(raw).is_some_and(|cms| vortex_smime::is_enveloped(&cms))
}

/// Pourquoi un message chiffré n'a pas pu être lu, pour l'utilisateur.
fn decrypt_reason(e: vortex_smime::DecryptError) -> String {
    use vortex_smime::DecryptError;
    match e {
        DecryptError::NotForMe => tr!("Ce message est chiffré pour un autre certificat que les vôtres.").into(),
        DecryptError::Unsupported => tr!("Ce message est chiffré avec un procédé non pris en charge.").into(),
        DecryptError::Unreadable => tr!("Message chiffré illisible.").into(),
        _ => tr!("Ce message chiffré n'a pas pu être déchiffré.").into(),
    }
}

/// Le message chiffré `raw` du compte `account_id`, déchiffré en mémoire : ses
/// en-têtes suivis du contenu, prêt à être lu comme un autre message. La clé
/// du compte est essayée d'abord, puis celles des autres comptes.
///
/// # Errors
///
/// Aucun certificat importé, message chiffré pour un autre certificat, procédé
/// non pris en charge, message abîmé.
pub(crate) fn decrypt(accounts: &[AccountConfig], account_id: &str, raw: &[u8]) -> Result<Vec<u8>, String> {
    let cms = pkcs7_body(raw).ok_or_else(|| tr!("Message chiffré illisible.").to_string())?;
    let mut tried = false;
    let mut failure: Option<vortex_smime::DecryptError> = None;
    let mine = accounts.iter().filter(|c| c.id == account_id);
    for config in mine.chain(accounts.iter().filter(|c| c.id != account_id)) {
        let Ok((identity, _)) = identity_parts(&config.id) else { continue };
        tried = true;
        match identity.decrypt(&cms) {
            Ok(inner) => return Ok(with_content(raw, &inner)),
            // « Pas pour moi » ne masque pas une autre raison donnée par un autre certificat.
            Err(e) if failure.is_none() || failure == Some(vortex_smime::DecryptError::NotForMe) => failure = Some(e),
            Err(_) => {}
        }
    }
    if !tried {
        return Err(tr!(
            "Ce message est chiffré : importez votre certificat (fichier .p12) dans Paramètres, sur la fiche du compte, pour le lire."
        )
        .into());
    }
    Err(decrypt_reason(failure.unwrap_or(vortex_smime::DecryptError::Failed)))
}

// ------------------------------------------------------------------ signature (lot D2)

/// Fichier des certificats d'un compte, dans `smime/comptes`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Stored {
    /// Certificats en base64 (DER), le signataire d'abord.
    certificates: Vec<String>,
    /// Bouton « Signer » enfoncé à l'ouverture d'une rédaction.
    sign_by_default: bool,
}

/// Certificat de signature d'un compte, pour les réglages.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdentityInfo {
    name: String,
    addresses: Vec<String>,
    issuer: String,
    fingerprint: String,
    not_before: i64,
    not_after: i64,
    sign_by_default: bool,
}

/// Compte qui a un certificat S/MIME, pour la fenêtre de rédaction.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SigningAccount {
    account_id: String,
    sign_by_default: bool,
    state: CertificateState,
}

/// Certificat d'un compte tel que la rédaction le trouve (audit S4) : un
/// fichier présent mais illisible n'est pas pris pour une absence de
/// certificat, sans quoi un brouillon à chiffrer repartirait en clair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CertificateState {
    /// Certificat lu : le compte peut signer et chiffrer.
    Ready,
    /// Fichier du certificat présent mais illisible.
    Unreadable,
}

/// Nom de fichier tiré de l'identifiant d'un compte : seulement des lettres,
/// des chiffres, `-` et `_` ; le reste en hexadécimal.
fn file_of(account_id: &str) -> Option<PathBuf> {
    let name: String = account_id
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' { char::from(b).to_string() } else { format!("%{b:02x}") })
        .collect();
    if name.is_empty() {
        return None;
    }
    DIR.get().map(|d| d.join("comptes").join(format!("{name}.json")))
}

fn read_stored(account_id: &str) -> Option<Stored> {
    let bytes = read_limited(&file_of(account_id)?, 1024 * 1024)?;
    serde_json::from_slice(&bytes).ok()
}

fn write_stored(account_id: &str, stored: &Stored) -> Result<(), String> {
    let file = file_of(account_id).ok_or_else(|| tr!("Dossier des réglages inconnu.").to_string())?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec_pretty(stored).map_err(|e| e.to_string())?;
    std::fs::write(&file, json).map_err(|e| e.to_string())
}

/// Entrée du trousseau qui garde la clé privée d'un compte.
fn key_entry(account_id: &str) -> String {
    format!("{account_id}/smime")
}

fn key_label(email: &str) -> String {
    tr!("Vortex Mail — {email} (clé de signature S/MIME)", email = email)
}

/// Message pour l'utilisateur d'une erreur de signature.
fn reason(e: vortex_smime::SignError) -> String {
    use vortex_smime::SignError;
    match e {
        SignError::Unreadable => tr!("Fichier illisible, ou mot de passe incorrect.").into(),
        SignError::NoKey => tr!("Ce fichier ne contient pas de clé privée avec son certificat.").into(),
        SignError::UnsupportedKey => tr!("Type de clé non pris en charge (RSA de 2048 à 8192 bits, ou ECDSA P-256 ou P-384).").into(),
        SignError::NotForEmail => tr!("Ce certificat ne permet pas de signer du courrier.").into(),
        SignError::Expired => tr!("Le certificat de signature est expiré, ou pas encore valide.").into(),
        _ => tr!("La signature du message a échoué.").into(),
    }
}

fn info(identity: &vortex_smime::Identity, sign_by_default: bool) -> IdentityInfo {
    let signer = identity.signer();
    IdentityInfo {
        name: signer.name,
        addresses: signer.addresses,
        issuer: signer.issuer,
        fingerprint: signer.fingerprint,
        not_before: signer.not_before,
        not_after: signer.not_after,
        sign_by_default,
    }
}

/// Identité d'un compte faite de sa clé (trousseau) et de ses certificats.
fn identity_parts(account_id: &str) -> Result<(vortex_smime::Identity, Stored), String> {
    let stored = read_stored(account_id).ok_or_else(|| tr!("Aucun certificat de signature pour ce compte.").to_string())?;
    let key = crate::secrets::read(&key_entry(account_id), "")?
        .ok_or_else(|| tr!("Clé de signature introuvable dans le trousseau : importez de nouveau le certificat.").to_string())?;
    let key = mail_core::compose::base64_decode(&key)
        .map_err(|_| tr!("Clé de signature illisible : importez de nouveau le certificat.").to_string())?;
    let certificates: Vec<Vec<u8>> = stored.certificates.iter().filter_map(|c| mail_core::compose::base64_decode(c).ok()).collect();
    let identity = vortex_smime::Identity::new(&key, &certificates).map_err(reason)?;
    Ok((identity, stored))
}

/// Importe la clé et le certificat d'un compte depuis un fichier PKCS #12
/// (`.p12`, `.pfx`), à la place des précédents.
///
/// # Errors
///
/// Fichier illisible ou mot de passe faux, certificat d'une autre adresse,
/// expiré ou qui ne signe pas de courrier, trousseau inaccessible.
pub(crate) fn import(config: &AccountConfig, data: &[u8], password: &str, now: i64) -> Result<IdentityInfo, String> {
    let (key, certificates) = vortex_smime::read_pkcs12(data, password).map_err(reason)?;
    let identity = vortex_smime::Identity::new(&key, &certificates).map_err(reason)?;
    identity.usable_at(now).map_err(reason)?;
    if !identity.is_for(&config.email) {
        return Err(tr!(
            "Ce certificat est pour {addresses}, pas pour {email} : vos correspondants verraient une signature non vérifiée.",
            addresses = identity.signer().addresses.join(", "),
            email = config.email
        ));
    }
    let previous = read_stored(&config.id).unwrap_or_default();
    crate::secrets::write(&key_entry(&config.id), &key_label(&config.email), &mail_core::compose::base64(&key))?;
    let stored = Stored {
        certificates: identity.certificates().iter().map(|c| mail_core::compose::base64(c.as_slice())).collect(),
        sign_by_default: previous.sign_by_default,
    };
    write_stored(&config.id, &stored)?;
    Ok(info(&identity, stored.sign_by_default))
}

/// Certificat de signature d'un compte, s'il en a un (sans lire la clé).
pub(crate) fn identity_info(account_id: &str) -> Option<IdentityInfo> {
    let stored = read_stored(account_id)?;
    let certificate = mail_core::compose::base64_decode(stored.certificates.first()?).ok()?;
    let signer = vortex_smime::certificate_signer(&certificate)?;
    Some(IdentityInfo {
        name: signer.name,
        addresses: signer.addresses,
        issuer: signer.issuer,
        fingerprint: signer.fingerprint,
        not_before: signer.not_before,
        not_after: signer.not_after,
        sign_by_default: stored.sign_by_default,
    })
}

/// « Signer tous les messages » d'un compte.
///
/// # Errors
///
/// Compte sans certificat, ou fichier impossible à écrire.
pub(crate) fn set_sign_by_default(account_id: &str, on: bool) -> Result<(), String> {
    let mut stored = read_stored(account_id).ok_or_else(|| tr!("Aucun certificat de signature pour ce compte.").to_string())?;
    stored.sign_by_default = on;
    write_stored(account_id, &stored)
}

/// Retire le certificat et la clé d'un compte (aussi à sa suppression).
///
/// # Errors
///
/// Trousseau ou fichier impossible à modifier.
pub(crate) fn forget(account_id: &str) -> Result<(), String> {
    crate::secrets::remove(&key_entry(account_id), "")?;
    match file_of(account_id).map(std::fs::remove_file) {
        Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

/// Comptes qui ont un certificat, lisible ou non, parmi `accounts`.
pub(crate) fn signers(accounts: &[AccountConfig]) -> Vec<SigningAccount> {
    accounts
        .iter()
        .filter_map(|c| {
            let present = file_of(&c.id).is_some_and(|file| file.exists());
            signer_of(&c.id, read_stored(&c.id).as_ref(), present)
        })
        .collect()
}

/// Ce que la rédaction apprend du certificat d'un compte : `stored`, le
/// fichier lu ; `present`, le fichier existe sur le disque.
fn signer_of(account_id: &str, stored: Option<&Stored>, present: bool) -> Option<SigningAccount> {
    let account_id = account_id.to_string();
    match stored {
        Some(stored) if !stored.certificates.is_empty() => {
            Some(SigningAccount { account_id, sign_by_default: stored.sign_by_default, state: CertificateState::Ready })
        }
        None if present => Some(SigningAccount { account_id, sign_by_default: false, state: CertificateState::Unreadable }),
        // Certificat retiré (liste vide), ou aucun fichier.
        Some(_) | None => None,
    }
}

/// Signature d'un message à envoyer par le compte `config`, à l'instant
/// `now` : rend la signature CMS détachée et le nom de son hachage.
///
/// # Errors
///
/// Pas de certificat ou de clé, certificat expiré ou d'une autre adresse,
/// échec de la signature.
pub(crate) fn sign(config: &AccountConfig, content: &[u8], now: i64) -> Result<(Vec<u8>, &'static str), String> {
    let (identity, _) = identity_parts(&config.id)?;
    if !identity.is_for(&config.email) {
        return Err(tr!("Le certificat de signature n'est pas celui de l'adresse {email}.", email = config.email));
    }
    let signature = identity.sign_detached(content, now).map_err(reason)?;
    Ok((signature, identity.micalg()))
}

// ------------------------------------------------------------------ correspondants (lot D3b)

/// Certificat d'un correspondant, gardé pour lui écrire chiffré.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Correspondent {
    /// Certificat en base64 (DER).
    certificate: String,
    /// Sa signature venait d'une autorité de confiance (verdict « vérifiée »).
    verified: bool,
    /// Dernière fois qu'il a été vu sur un message signé (secondes depuis 1970).
    seen: i64,
}

/// Certificat d'un correspondant, pour les réglages.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CorrespondentInfo {
    address: String,
    name: String,
    issuer: String,
    fingerprint: String,
    not_after: i64,
    verified: bool,
    seen: i64,
}

/// Correspondants par adresse (en minuscules), dans `smime/correspondants.json`.
type Correspondents = std::collections::BTreeMap<String, Correspondent>;

/// Une seule écriture à la fois du fichier des correspondants.
static CORRESPONDENTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn correspondents_file() -> Option<PathBuf> {
    DIR.get().map(|d| d.join("correspondants.json"))
}

fn read_correspondents() -> Correspondents {
    correspondents_file()
        .and_then(|f| read_limited(&f, MAX_BUNDLE_BYTES))
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_correspondents(all: &Correspondents) -> Result<(), String> {
    let file = correspondents_file().ok_or_else(|| tr!("Dossier des réglages inconnu.").to_string())?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec_pretty(all).map_err(|e| e.to_string())?;
    std::fs::write(&file, json).map_err(|e| e.to_string())
}

/// Adresse seule, en minuscules (`Nom <a@b.fr>` → `a@b.fr`).
fn address_key(address: &str) -> String {
    let inner = match (address.rfind('<'), address.rfind('>')) {
        (Some(start), Some(end)) if start < end => address.get(start + 1..end).unwrap_or(address),
        _ => address,
    };
    inner.trim().to_ascii_lowercase()
}

/// Garde le certificat `der` du correspondant `address`, vu sur un message
/// signé : un certificat d'une autorité de confiance n'est remplacé que par un
/// autre qui l'est aussi (ou quand il a expiré) ; sinon le plus durable gagne.
/// Seuls les certificats qui peuvent servir à chiffrer sont gardés.
fn remember(address: &str, der: &[u8], verified: bool, now: i64) {
    let key = address_key(address);
    if key.is_empty() || !vortex_smime::can_encrypt_to(der) {
        return;
    }
    let Some(new) = vortex_smime::certificate_signer(der) else { return };
    let _guard = CORRESPONDENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut all = read_correspondents();
    let encoded = mail_core::compose::base64(der);
    if let Some(old) = all.get(&key) {
        let old_signer = mail_core::compose::base64_decode(&old.certificate).ok().and_then(|d| vortex_smime::certificate_signer(&d));
        let old_expired = old_signer.as_ref().is_none_or(|s| s.not_after < now);
        let same = old.certificate == encoded;
        let better = old_expired || ((verified || !old.verified) && old_signer.as_ref().is_none_or(|s| new.not_after >= s.not_after));
        if same && (old.verified || !verified) && now - old.seen < 86_400 {
            return; // déjà gardé tel quel aujourd'hui : pas d'écriture
        }
        if !same && !better {
            return;
        }
    }
    let verified = verified || all.get(&key).is_some_and(|old| old.verified && old.certificate == encoded);
    all.insert(key, Correspondent { certificate: encoded, verified, seen: now });
    if let Err(e) = write_correspondents(&all) {
        crate::diag::report(&e);
    }
}

/// Certificats des correspondants, pour les réglages, par adresse.
pub(crate) fn correspondents() -> Vec<CorrespondentInfo> {
    read_correspondents()
        .into_iter()
        .filter_map(|(address, c)| {
            let der = mail_core::compose::base64_decode(&c.certificate).ok()?;
            let signer = vortex_smime::certificate_signer(&der)?;
            Some(CorrespondentInfo {
                address,
                name: signer.name,
                issuer: signer.issuer,
                fingerprint: signer.fingerprint,
                not_after: signer.not_after,
                verified: c.verified,
                seen: c.seen,
            })
        })
        .collect()
}

/// Oublie le certificat du correspondant `address`.
///
/// # Errors
///
/// Fichier impossible à écrire.
pub(crate) fn forget_correspondent(address: &str) -> Result<(), String> {
    let _guard = CORRESPONDENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut all = read_correspondents();
    if all.remove(&address_key(address)).is_some() {
        write_correspondents(&all)?;
    }
    Ok(())
}

/// Certificat valide à `now` du correspondant `address`, en DER.
fn correspondent_certificate(all: &Correspondents, address: &str, now: i64) -> Option<Vec<u8>> {
    let der = mail_core::compose::base64_decode(&all.get(&address_key(address))?.certificate).ok()?;
    let signer = vortex_smime::certificate_signer(&der)?;
    (signer.not_before <= now && now <= signer.not_after && vortex_smime::can_encrypt_to(&der)).then_some(der)
}

/// Adresses parmi `addresses` dont aucun certificat valide n'est connu.
pub(crate) fn without_certificate(addresses: &[String], now: i64) -> Vec<String> {
    let all = read_correspondents();
    addresses.iter().filter(|a| correspondent_certificate(&all, a, now).is_none()).cloned().collect()
}

/// Destinataires d'un message à chiffrer, examinés pour la rédaction (lot D4).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecipientCheck {
    /// Sans certificat valide : l'envoi chiffré est refusé.
    missing: Vec<String>,
    /// Certificat d'une autorité inconnue : à confirmer avec le correspondant.
    unverified: Vec<String>,
}

/// Les destinataires `addresses` : sans certificat, ou avec un certificat
/// qui ne vient pas d'une autorité de confiance.
pub(crate) fn check_recipients(addresses: &[String], now: i64) -> RecipientCheck {
    let all = read_correspondents();
    let mut check = RecipientCheck::default();
    for address in addresses {
        if correspondent_certificate(&all, address, now).is_none() {
            check.missing.push(address.clone());
        } else if all.get(&address_key(address)).is_some_and(|c| !c.verified) {
            check.unverified.push(address.clone());
        }
    }
    check
}

/// Le contenu d'un message du compte `config` chiffré pour `recipients` et
/// pour l'expéditeur lui-même (copie dans « Éléments envoyés »), puis relu
/// avec la clé de l'expéditeur avant d'être rendu.
///
/// # Errors
///
/// Pas de certificat pour l'expéditeur ou pour un destinataire (les adresses
/// sont nommées), certificat qui ne convient pas, échec du chiffrement.
pub(crate) fn encrypt(config: &AccountConfig, content: &[u8], recipients: &[String], now: i64) -> Result<Vec<u8>, String> {
    let (identity, _) = identity_parts(&config.id)?;
    let missing = without_certificate(recipients, now);
    if !missing.is_empty() {
        return Err(tr!(
            "Pas de certificat pour chiffrer vers : {addresses}. Il faut d'abord avoir reçu d'eux un message signé.",
            addresses = missing.join(", ")
        ));
    }
    let all = read_correspondents();
    // Certificat révoqué par son autorité (lot G2c) : pas de message chiffré pour lui.
    let lists = revocations();
    let revoked: Vec<String> = recipients
        .iter()
        .filter(|a| {
            correspondent_certificate(&all, a, now)
                .is_some_and(|der| vortex_smime::revocation_of(&der, anchors(), &lists, now) == Revocation::Revoked)
        })
        .cloned()
        .collect();
    if !revoked.is_empty() {
        return Err(tr!(
            "Certificat révoqué par son autorité : {addresses}. Demandez-leur un nouveau message signé avec leur nouveau certificat.",
            addresses = revoked.join(", ")
        ));
    }
    let mut certificates: Vec<Vec<u8>> = recipients.iter().filter_map(|a| correspondent_certificate(&all, a, now)).collect();
    let own = identity.certificates().first().cloned().ok_or_else(|| tr!("Aucun certificat de signature pour ce compte.").to_string())?;
    if !vortex_smime::can_encrypt_to(&own) {
        return Err(
            tr!("Votre certificat ne permet pas de chiffrer (clé RSA, P-256 ou P-384 nécessaire) : votre copie serait illisible.").into()
        );
    }
    certificates.push(own);
    let cms = vortex_smime::encrypt(content, &certificates).map_err(|e| match e {
        vortex_smime::EncryptError::UnsupportedCertificate(i) => {
            tr!("Le certificat de {address} ne permet pas de chiffrer.", address = recipients.get(i).map_or("?", String::as_str))
        }
        _ => tr!("Le chiffrement du message a échoué.").into(),
    })?;
    // Relu avec la clé de l'expéditeur : un message que lui-même ne pourrait pas relire ne part pas.
    match identity.decrypt(&cms) {
        Ok(plain) if plain == content => Ok(cms),
        _ => Err(tr!("Le chiffrement du message a échoué.").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s4_certificat_illisible_distingue_de_l_absence() {
        let stored = || Stored { certificates: vec!["MIIB".into()], sign_by_default: true };
        let ready = signer_of("a", Some(&stored()), true).expect("certificat lu");
        assert_eq!((ready.state, ready.sign_by_default), (CertificateState::Ready, true));
        let unreadable = signer_of("a", None, true).expect("fichier présent mais illisible");
        assert_eq!(unreadable.state, CertificateState::Unreadable);
        assert!(signer_of("a", None, false).is_none(), "pas de certificat");
        assert!(signer_of("a", Some(&Stored { certificates: Vec::new(), sign_by_default: false }), true).is_none(), "certificat retiré");
    }

    /// Messages signés par Alice (autorité de test reprise avec la crate), et une date
    /// où son certificat est valide.
    const OPAQUE: &[u8] = include_bytes!("../../crates/smime/testdata/opaque.eml");
    const DETACHED: &[u8] = include_bytes!("../../crates/smime/testdata/detached.eml");
    const NOW: i64 = 1_794_182_400;

    #[test]
    fn signature_detachee() {
        let s = check(DETACHED, NOW).expect("message signé");
        // L'autorité de test n'est pas parmi celles du système.
        assert_eq!(s.verdict, "notVerified");
        assert!(matches!(s.why, Some("unknownIssuer" | "noAuthorities")), "{:?}", s.why);
        assert_eq!(s.addresses, ["alice@smime.example"]);
        assert!(s.whole);
        assert_eq!(opaque_content(DETACHED), None, "contenu lisible tel quel");
        let changed: Vec<u8> = String::from_utf8_lossy(DETACHED).replace("Hello Bob", "Hello Eve").into_bytes();
        assert_eq!(check(&changed, NOW).expect("message signé").verdict, "changed");
    }

    #[test]
    fn d4_destinataires_examines() {
        // Sans dossier des réglages : aucun certificat connu, rien de « non vérifié ».
        let check = check_recipients(&["a@x.fr".to_string()], 0);
        assert_eq!((check.missing.as_slice(), check.unverified.len()), (&["a@x.fr".to_string()][..], 0));
    }

    #[test]
    fn d3b_adresses() {
        assert_eq!(address_key("Marie Dupont <Marie@Exemple.FR>"), "marie@exemple.fr");
        assert_eq!(address_key(" paul@x.fr "), "paul@x.fr");
        // Sans dossier des réglages : aucun certificat connu.
        assert_eq!(without_certificate(&["a@x.fr".to_string()], 0), ["a@x.fr"]);
    }

    #[test]
    fn d3_message_chiffre_reconnu() {
        let enveloped = include_bytes!("../../crates/smime/testdata/enveloped.eml");
        assert!(is_encrypted(enveloped));
        assert!(!is_encrypted(OPAQUE), "signé, pas chiffré");
        assert!(!is_encrypted(DETACHED));
        assert_eq!(pkcs7_body(DETACHED), None);
        // Sans certificat importé : on le dit.
        assert!(decrypt(&[], "a", enveloped).unwrap_err().contains(".p12"));
        let shown =
            with_content(b"From: a@x.fr\r\nContent-Type: application/pkcs7-mime\r\n\r\nCMS", b"Content-Type: text/plain\r\n\r\nClair");
        assert_eq!(shown, b"From: a@x.fr\r\nContent-Type: text/plain\r\n\r\nClair");
    }

    #[test]
    fn signature_opaque_affichee_par_son_contenu() {
        assert_eq!(check(OPAQUE, NOW).expect("message signé").addresses, ["alice@smime.example"]);
        let shown = String::from_utf8(opaque_content(OPAQUE).expect("contenu de la signature")).expect("ASCII");
        assert!(
            shown.starts_with("To: bob@smime.example\r\nFrom: alice@smime.example\r\nSubject: Opaque minutes\r\nMIME-Version: 1.0\r\n"),
            "en-têtes du message gardés : {shown}"
        );
        assert!(!shown.to_ascii_lowercase().contains("pkcs7"), "en-têtes de la partie signée retirés : {shown}");
        assert!(shown.contains("Content-Type: text/plain; charset=us-ascii\r\n\r\nHello Bob,"), "{shown}");
    }

    #[test]
    fn certificats_pem_et_der() {
        let pem = "texte\n-----BEGIN CERTIFICATE-----\nAAEC\n-----END CERTIFICATE-----\n-----BEGIN CERTIFICATE-----\nAwQF\n-----END CERTIFICATE-----";
        assert_eq!(pem_certificates(pem), vec![vec![0, 1, 2], vec![3, 4, 5]]);
        assert_eq!(certificates_in(&[0x30, 0x82]), vec![vec![0x30, 0x82]]);
        assert!(pem_certificates("-----BEGIN CERTIFICATE-----\nAAEC").is_empty(), "bloc sans fin");
    }

    #[test]
    fn messages_non_signes() {
        assert_eq!(check(b"From: a@x.fr\r\nContent-Type: text/plain\r\n\r\nBonjour", 0), None);
        assert_eq!(column(b"Subject: x\r\n\r\ny", 0), crate::cache::UNSIGNED);
        assert!(announced(b"Content-Type: Multipart/Signed; protocol=\"application/pkcs7-signature\""));
        assert!(announced(b"Content-Type: application/x-pkcs7-mime; smime-type=signed-data"));
        assert!(!announced(b"Content-Type: multipart/mixed; boundary=x"));
        assert!(is_signature_part("Application/PKCS7-Signature; name=smime.p7s"));
        assert!(is_signature_part("application/x-pkcs7-signature"));
        assert!(!is_signature_part("application/pdf"));
        assert_eq!(opaque_content(b"Content-Type: text/plain\r\n\r\nx"), None);
    }
}
