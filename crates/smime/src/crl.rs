// SPDX-FileCopyrightText: Vortex Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Certificats révoqués (lot G2c de Vortex Mail ; pas dans MailCraft).
//!
//! Listes de certificats révoqués (CRL, RFC 5280 §5) que l'hôte a téléchargées aux adresses
//! que donnent les certificats ([`crate::cert_crl_urls`]). Pas d'entrée-sortie ici : l'hôte
//! remet les listes en DER ([`Revocations::new`]), la vérification d'un signataire les consulte
//! ([`crate::check_revoked`]).
//!
//! Une liste ne compte que si elle est signée par une autorité de confiance du certificat
//! (celle qui l'a délivré), qu'elle est en cours de validité (`thisUpdate` passé, `nextUpdate`
//! à venir) et qu'elle n'a pas d'extension critique (listes partielles, indirectes, deltas) :
//! sinon la révocation est dite non vérifiée, jamais « bonne ». Seul le certificat du signataire
//! est contrôlé, pas les autorités intermédiaires.

use der::Decode;
use spki::AlgorithmIdentifierOwned;

use crate::algorithms;
use crate::ber::{self, Tlv};
use crate::cert::Cert;
use crate::signature::Timestamp;

/// Listes gardées au plus.
pub const MAX_CRLS: usize = 256;
/// Taille d'une liste au plus (octets).
pub const MAX_CRL_BYTES: usize = 32 * 1024 * 1024;
/// Certificats révoqués lus au plus dans une liste.
pub const MAX_REVOKED: usize = 2_000_000;

const SEQUENCE: u8 = 0x30;
const INTEGER: u8 = 0x02;
const BIT_STRING: u8 = 0x03;
const BOOLEAN: u8 = 0x01;
const UTC_TIME: u8 = 0x17;
const GENERALIZED_TIME: u8 = 0x18;
const CONTEXT_0_CONSTRUCTED: u8 = 0xa0;

/// Ce que les listes disent d'un certificat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revocation {
    /// Une liste valable de son autorité ne le cite pas.
    Good,
    /// Une liste valable de son autorité le cite.
    Revoked,
    /// Aucune liste valable de son autorité ici.
    Unknown,
}

/// Une liste lue, pas encore contrôlée (sa signature l'est à la consultation).
struct List {
    /// Émetteur (`Name`, en DER).
    issuer: Vec<u8>,
    this_update: Timestamp,
    next_update: Option<Timestamp>,
    /// Numéros de série révoqués (contenu des `INTEGER`), triés.
    revoked: Vec<Vec<u8>>,
    /// `tbsCertList` tel que reçu, et ce qui le signe.
    tbs: Vec<u8>,
    algorithm: AlgorithmIdentifierOwned,
    signature: Vec<u8>,
}

/// Les listes de certificats révoqués que l'hôte a sous la main.
#[derive(Default)]
pub struct Revocations {
    lists: Vec<List>,
}

impl std::fmt::Debug for Revocations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Revocations").field("lists", &self.lists.len()).finish()
    }
}

/// Une date (`UTCTime` ou `GeneralizedTime`) en secondes depuis 1970.
fn time(t: &Tlv<'_>) -> Option<Timestamp> {
    if t.tag != UTC_TIME && t.tag != GENERALIZED_TIME {
        return None;
    }
    let time = x509_cert::time::Time::from_der(t.whole).ok()?;
    Timestamp::try_from(time.to_unix_duration().as_secs()).ok()
}

/// Des extensions (`SEQUENCE OF Extension`) dont l'une est critique ?
fn has_critical(extensions: &Tlv<'_>) -> bool {
    let Some(list) = ber::children(extensions.value) else { return true };
    list.iter().any(|ext| ber::children(ext.value).is_none_or(|fields| fields.iter().any(|f| f.tag == BOOLEAN && f.value.iter().any(|b| *b != 0))))
}

/// Une CRL en DER : `None` si elle est illisible, trop grande ou ne peut pas servir (extension
/// critique de la liste ou d'une entrée).
fn list(der: &[u8]) -> Option<List> {
    if der.len() > MAX_CRL_BYTES {
        return None;
    }
    let (outer, _) = ber::read(der).filter(|(t, _)| t.tag == SEQUENCE)?;
    let parts = ber::children(outer.value)?;
    let [tbs, algorithm, signature] = parts.as_slice() else { return None };
    if tbs.tag != SEQUENCE || signature.tag != BIT_STRING {
        return None;
    }
    let algorithm = AlgorithmIdentifierOwned::from_der(algorithm.whole).ok()?;
    let (unused, bits) = signature.value.split_first()?;
    if *unused != 0 {
        return None;
    }
    let fields = ber::children(tbs.value)?;
    let mut fields = fields.iter().peekable();
    // version INTEGER facultative, puis signature, issuer, thisUpdate.
    if fields.peek().is_some_and(|f| f.tag == INTEGER) {
        fields.next();
    }
    let _signature = fields.next()?;
    let issuer = fields.next().filter(|t| t.tag == SEQUENCE)?;
    let this_update = time(fields.next()?)?;
    let mut next_update = None;
    if let Some(t) = fields.peek().filter(|t| t.tag == UTC_TIME || t.tag == GENERALIZED_TIME) {
        next_update = Some(time(t)?);
        fields.next();
    }
    let mut revoked = Vec::new();
    if let Some(entries) = fields.peek().filter(|t| t.tag == SEQUENCE) {
        for entry in ber::children(entries.value)?.into_iter().take(MAX_REVOKED) {
            let entry_fields = ber::children(entry.value)?;
            let serial = entry_fields.first().filter(|t| t.tag == INTEGER)?;
            // Extension critique d'une entrée (émetteur d'une liste indirecte…) : liste inutilisable.
            if entry_fields.iter().skip(2).any(|e| e.tag == SEQUENCE && has_critical(e)) {
                return None;
            }
            revoked.push(serial.value.to_vec());
        }
        fields.next();
    }
    // crlExtensions [0] EXPLICIT : pas d'extension critique (liste partielle, delta…).
    if let Some(extensions) = fields.next().filter(|t| t.tag == CONTEXT_0_CONSTRUCTED) {
        let inner = ber::children(extensions.value)?;
        if inner.first().is_none_or(has_critical) {
            return None;
        }
    }
    revoked.sort_unstable();
    Some(List { issuer: issuer.whole.to_vec(), this_update, next_update, revoked, tbs: tbs.whole.to_vec(), algorithm, signature: bits.to_vec() })
}

/// Prochaine mise à jour annoncée par une CRL en DER (secondes depuis 1970), pour que l'hôte la
/// télécharge de nouveau à temps ; `None` si elle est illisible ou n'en annonce pas.
pub fn crl_next_update(der: &[u8]) -> Option<Timestamp> {
    list(der)?.next_update
}

impl Revocations {
    /// Les listes en DER ; une liste illisible ou inutilisable est laissée de côté, comme tout
    /// ce qui passe [`MAX_CRLS`].
    pub fn new<I, B>(ders: I) -> Revocations
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        Revocations { lists: ders.into_iter().take(MAX_CRLS).filter_map(|d| list(d.as_ref())).collect() }
    }

    /// Nombre de listes utilisables.
    pub fn len(&self) -> usize {
        self.lists.len()
    }

    /// Aucune liste ?
    pub fn is_empty(&self) -> bool {
        self.lists.is_empty()
    }

    /// Ce que disent les listes du certificat `cert`, à `now`, en ne croyant que celles que
    /// signe l'une des autorités `issuers` (de confiance, et qui l'ont délivré).
    pub(crate) fn status(&self, cert: &Cert, issuers: &[&Cert], now: Timestamp) -> Revocation {
        let mut good = false;
        for list in self.lists.iter().filter(|l| l.issuer == cert.issuer()) {
            if list.this_update > now.saturating_add(300) || list.next_update.is_some_and(|next| next < now) {
                continue;
            }
            let signed = issuers.iter().any(|issuer| {
                issuer.subject() == list.issuer.as_slice()
                    && algorithms::verify(issuer.public_key(), &list.algorithm, None, &list.tbs, &list.signature).is_ok()
            });
            if !signed {
                continue;
            }
            if list.revoked.binary_search_by(|s| s.as_slice().cmp(cert.serial())).is_ok() {
                return Revocation::Revoked;
            }
            good = true;
        }
        if good { Revocation::Good } else { Revocation::Unknown }
    }
}
