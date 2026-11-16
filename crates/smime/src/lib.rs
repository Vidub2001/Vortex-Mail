// SPDX-FileCopyrightText: MailCraft contributors, Vortex Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! S/MIME (RFC 8551) de Vortex Mail : vérification des signatures (lot D1), signature des
//! messages envoyés (lot D2, `sign.rs`), lecture des messages chiffrés (lot D3a, `decrypt.rs`)
//! et chiffrement des messages envoyés (lot D3b, `encrypt.rs`) ; ces trois fichiers sont écrits
//! pour Vortex Mail.
//!
//! Reprise de la crate `mailcraft-smime` de MailCraft (`github.com/UCS-Care-Group/mailcraft`,
//! MIT OR Apache-2.0, voir `NOTICE`), avec ses dépendances réduites à ce qui sert ici : le
//! lecteur BER (`ber.rs`, de `mailcraft-model`), le résultat d'une vérification
//! (`signature.rs`, de `mailcraft-model`, sans `serde`) et la lecture des deux moitiés d'un
//! message signé (`parts.rs`, d'après `mailcraft-mime`). Les fichiers repris gardent leur
//! licence et leur texte d'origine ; les écarts sont signalés en français.
//!
//! What follows is MailCraft's description of the crate.
//!
//! MailCraft's S/MIME (RFC 8551): Digital signatures checked (#30), written from the public
//! specifications (RFC 5652 for CMS, RFC 5280 for X.509, RFC 5754, RFC 4055 and RFC 5480 for
//! the algorithms). No I/O: the host hands in the certificates it trusts ([`Anchors`]), the
//! message's halves come from [`signed_parts`], and the result is [`DigitalSignature`].
//!
//! [`check`] reads the CMS SignedData of a detached or an opaque Digital signature, finds the
//! signer's certificate, compares the signer's digest with the message's, checks the signature
//! (RSA PKCS #1 v1.5 or PSS of 2048 bits or more, ECDSA on P-256 or P-384, over SHA-2), and then
//! whether the certificate rests on a trust anchor, is for signing mail and is for the From
//! address. MD5 and SHA-1 are no longer safe and are said to be. Revocation isn't checked.
//!
//! Input is hostile: BER is turned into DER with nesting bounded, every length is checked
//! against what is there, the certificates, signers and the path to an anchor are capped, and
//! nothing here panics on any bytes.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

mod algorithms;
pub mod ber;
mod cert;
mod chain;
mod crl;
mod decrypt;
mod encrypt;
mod parts;
mod sign;
pub mod signature;
mod verify;

#[cfg(test)]
mod tests;

pub use chain::{Anchors, MAX_ANCHORS};
pub use crl::{MAX_CRL_BYTES, MAX_CRLS, Revocation, Revocations, crl_next_update};
pub use decrypt::{DecryptError, MAX_CONTENT, MAX_RECIPIENTS, is_enveloped};
pub use encrypt::{EncryptError, MAX_ENCRYPT_RECIPIENTS, can_encrypt_to, encrypt};
pub use parts::{SignedKind, SignedParts, signed_parts};
pub use sign::{Identity, MAX_CHAIN, MAX_PKCS12_BYTES, SignError, read_pkcs12};
pub use signature::{Coverage, DigitalSignature, SignatureFormat, Signer, Timestamp, Verdict, Why};
pub use verify::{Checked, Context, MAX_CERTS, MAX_SIGNERS, MAX_VERIFICATIONS, Signed, check, check_revoked};

/// Le signataire tel qu'un certificat en DER le décrit (Vortex Mail, lot D2) : `None` quand ce
/// n'en est pas un.
pub fn certificate_signer(der: &[u8]) -> Option<Signer> {
    cert::Cert::from_der(der).map(|cert| verify::signer_of(&cert))
}

/// The fingerprint and addresses of a certificate in DER, as a Digital signature's details show
/// them: `None` when it isn't one.
pub fn certificate_summary(der: &[u8]) -> Option<(String, Vec<String>, Timestamp)> {
    let cert = cert::Cert::from_der(der)?;
    Some((cert.fingerprint(), cert.addresses(), cert.validity().1))
}

/// Les adresses (`http`, `https`) où l'autorité d'un certificat en DER publie sa liste de
/// certificats révoqués (Vortex Mail, lot G2c) : vide quand ce n'est pas un certificat ou qu'il
/// n'en donne pas.
pub fn cert_crl_urls(der: &[u8]) -> Vec<String> {
    cert::Cert::from_der(der).map(|cert| cert.crl_urls()).unwrap_or_default()
}

/// Ce que disent les listes `revocations` du certificat `der` d'un correspondant, à `now`, en ne
/// croyant que celles que signe l'une des autorités `anchors` (Vortex Mail, lot G2c : avant de
/// lui écrire chiffré).
pub fn revocation_of(der: &[u8], anchors: &Anchors, revocations: &Revocations, now: Timestamp) -> Revocation {
    let Some(cert) = cert::Cert::from_der(der) else { return Revocation::Unknown };
    let issuers: Vec<&cert::Cert> = anchors.named(cert.issuer()).collect();
    revocations.status(&cert, &issuers, now)
}
