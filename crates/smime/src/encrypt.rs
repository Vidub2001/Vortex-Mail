// SPDX-FileCopyrightText: Vortex Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Chiffrement des messages envoyés (lot D3b de Vortex Mail ; pas dans MailCraft).
//!
//! `EnvelopedData` (RFC 5652 §6, RFC 8551 §3.3) : contenu chiffré en AES-256 CBC (RFC 3565)
//! avec une clé tirée au hasard, que tous les logiciels de messagerie savent lire. La clé est
//! transportée pour chaque destinataire :
//! - certificat RSA de 2048 à 8192 bits : en RSAES-PKCS1-v1_5 (`KeyTransRecipientInfo`) ;
//! - certificat P-256 ou P-384 (lot G2b) : enveloppée en AES-256 Key Wrap avec une clé convenue
//!   par ECDH entre une clé éphémère et celle du destinataire, dérivée en ANSI X9.63 sur SHA-256
//!   ou SHA-384 (`KeyAgreeRecipientInfo`, RFC 5753).
//!
//! Seuls les certificats faits pour le courrier, qui ne sont pas des autorités, servent de
//! destinataires ([`can_encrypt_to`]).
//!
//! Écrit en DER à la main (comme la lecture de `decrypt.rs`) : peu de champs, tous de longueur
//! connue, et les `RecipientInfo` rangés comme DER l'exige pour un SET OF.

use std::fmt;

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeEncrypt, KeyIvInit};
use der::Encode;

use crate::ber::tlv;
use crate::cert::Cert;

/// `rsaEncryption` (1.2.840.113549.1.1.1) en DER.
const OID_RSA: &[u8] = &[0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
/// `id-data` (1.2.840.113549.1.7.1).
const OID_DATA: &[u8] = &[0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01];
/// `id-envelopedData` (1.2.840.113549.1.7.3).
const OID_ENVELOPED_DATA: &[u8] = &[0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x03];
/// `id-aes256-CBC` (2.16.840.1.101.3.4.1.42).
const OID_AES256_CBC: &[u8] = &[0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x2a];
/// `NULL`.
const NULL: &[u8] = &[0x05, 0x00];
/// `INTEGER 0` (versions).
const VERSION_0: &[u8] = &[0x02, 0x01, 0x00];
/// `INTEGER 2` (version d'un `EnvelopedData` avec un `KeyAgreeRecipientInfo`, RFC 5652 §6.1).
const VERSION_2: &[u8] = &[0x02, 0x01, 0x02];
/// `INTEGER 3` (version d'un `KeyAgreeRecipientInfo`).
const VERSION_3: &[u8] = &[0x02, 0x01, 0x03];
/// `id-ecPublicKey` (1.2.840.10045.2.1).
const OID_EC_PUBLIC_KEY: &[u8] = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
/// `dhSinglePass-stdDH-sha256kdf-scheme` (1.3.132.1.11.1).
const OID_ECDH_SHA256_KDF: &[u8] = &[0x06, 0x06, 0x2b, 0x81, 0x04, 0x01, 0x0b, 0x01];
/// `dhSinglePass-stdDH-sha384kdf-scheme` (1.3.132.1.11.2).
const OID_ECDH_SHA384_KDF: &[u8] = &[0x06, 0x06, 0x2b, 0x81, 0x04, 0x01, 0x0b, 0x02];
/// `id-aes256-wrap` (2.16.840.1.101.3.4.1.45).
const OID_AES256_WRAP: &[u8] = &[0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x2d];

const RSA_KEY: const_oid::ObjectIdentifier = const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const EC_KEY: const_oid::ObjectIdentifier = const_oid::ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");

/// La clé publique d'un destinataire.
enum PublicKey {
    Rsa(rsa::PublicKey),
    P256(p256::PublicKey),
    P384(p384::PublicKey),
}

/// Destinataires d'un message chiffré, au plus.
pub const MAX_ENCRYPT_RECIPIENTS: usize = 100;

/// Pourquoi un message n'a pas pu être chiffré.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EncryptError {
    /// Un certificat ne peut pas servir à chiffrer (ni RSA de 2048 bits ou plus, ni P-256 ou
    /// P-384, illisible) ;
    /// son rang dans la liste donnée.
    UnsupportedCertificate(usize),
    /// Aucun destinataire, ou trop.
    Recipients,
    /// Pas d'aléa du système, ou échec du chiffrement.
    Failed,
}

impl fmt::Display for EncryptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncryptError::UnsupportedCertificate(i) => write!(f, "certificate {i} can't be encrypted to"),
            EncryptError::Recipients => f.write_str("no recipients, or too many"),
            EncryptError::Failed => f.write_str("the message could not be encrypted"),
        }
    }
}

impl std::error::Error for EncryptError {}

/// La clé publique d'un certificat, s'il peut servir à chiffrer du courrier.
fn public_key(cert: &Cert) -> Option<PublicKey> {
    use rsa::pkcs8::DecodePublicKey as _;
    use rsa::traits::PublicKeyParts as _;
    if cert.is_ca() || !cert.for_email() {
        return None;
    }
    let der = cert.public_key().to_der().ok()?;
    match cert.public_key().algorithm.oid {
        RSA_KEY => {
            let key = rsa::PublicKey::from_public_key_der(&der).ok()?;
            let bits = key.size().saturating_mul(8);
            (crate::algorithms::MIN_RSA_BITS..=crate::algorithms::MAX_RSA_BITS).contains(&bits).then_some(PublicKey::Rsa(key))
        }
        EC_KEY => p256::PublicKey::from_public_key_der(&der)
            .map(PublicKey::P256)
            .or_else(|_| p384::PublicKey::from_public_key_der(&der).map(PublicKey::P384))
            .ok(),
        _ => None,
    }
}

/// Le certificat `der` peut-il servir à chiffrer un message (RSA de 2048 à 8192 bits, ou
/// P-256 ou P-384 ; fait pour le courrier, pas une autorité) ? Sa validité dans le temps n'est
/// pas regardée ici.
pub fn can_encrypt_to(der: &[u8]) -> bool {
    Cert::from_der(der).is_some_and(|cert| public_key(&cert).is_some())
}

/// Clé éphémère de l'expéditeur pour la courbe `C`, tirée de l'aléa du système.
fn ephemeral<C>() -> Result<p256::elliptic_curve::SecretKey<C>, EncryptError>
where
    C: p256::elliptic_curve::CurveArithmetic,
{
    // Tirage rejeté (zéro, au-delà de l'ordre) : probabilité négligeable, on retire.
    for _ in 0..16 {
        let mut bytes = p256::elliptic_curve::FieldBytes::<C>::default();
        getrandom::fill(bytes.as_mut_slice()).map_err(|_| EncryptError::Failed)?;
        if let Ok(secret) = p256::elliptic_curve::SecretKey::<C>::from_bytes(&bytes) {
            return Ok(secret);
        }
    }
    Err(EncryptError::Failed)
}

/// `KeyAgreeRecipientInfo` (RFC 5753) : la clé de contenu enveloppée pour le destinataire
/// `cert`, avec la clé partagée `shared` convenue avec la clé éphémère `originator` (SEC1).
fn key_agreement(cert: &Cert, content_key: &[u8], shared: &[u8], originator: &[u8], sha384: bool) -> Result<Vec<u8>, EncryptError> {
    let wrap = tlv(0x30, OID_AES256_WRAP);
    let info = crate::decrypt::shared_info(&wrap, None, content_key.len()).ok_or(EncryptError::Failed)?;
    let kek = if sha384 { crate::decrypt::x963::<sha2::Sha384>(shared, &info, 32) } else { crate::decrypt::x963::<sha2::Sha256>(shared, &info, 32) };
    let wrapped = crate::decrypt::aes_wrap::<aes::Aes256>(&kek, content_key).ok_or(EncryptError::Failed)?;
    // originator [0] EXPLICIT : originatorKey [1] IMPLICIT { id-ecPublicKey, BIT STRING }.
    let point = [&[0u8][..], originator].concat();
    let originator_key = tlv(0xa1, &[tlv(0x30, OID_EC_PUBLIC_KEY).as_slice(), tlv(0x03, &point).as_slice()].concat());
    let originator = tlv(0xa0, &originator_key);
    let kdf = if sha384 { OID_ECDH_SHA384_KDF } else { OID_ECDH_SHA256_KDF };
    let algorithm = tlv(0x30, &[kdf, wrap.as_slice()].concat());
    let serial = tlv(0x02, cert.serial());
    let rid = tlv(0x30, &[cert.issuer(), serial.as_slice()].concat());
    let key = tlv(0x30, &[rid.as_slice(), tlv(0x04, &wrapped).as_slice()].concat());
    let keys = tlv(0x30, &key);
    // kari [1] IMPLICIT KeyAgreeRecipientInfo, version 3.
    Ok(tlv(0xa1, &[VERSION_3, originator.as_slice(), algorithm.as_slice(), keys.as_slice()].concat()))
}

/// `content` (une entité MIME) chiffré pour les certificats `recipients` (DER) : un
/// `ContentInfo` d'`EnvelopedData`, en DER. L'expéditeur s'ajoute lui-même aux destinataires
/// pour pouvoir relire sa copie.
///
/// # Errors
///
/// [`EncryptError::UnsupportedCertificate`] pour le premier certificat qui ne convient pas,
/// [`EncryptError::Recipients`] sans destinataire ou au-delà de [`MAX_ENCRYPT_RECIPIENTS`],
/// [`EncryptError::Failed`] sans aléa du système.
pub fn encrypt(content: &[u8], recipients: &[Vec<u8>]) -> Result<Vec<u8>, EncryptError> {
    if recipients.is_empty() || recipients.len() > MAX_ENCRYPT_RECIPIENTS {
        return Err(EncryptError::Recipients);
    }
    let mut keys: Vec<(Cert, PublicKey)> = Vec::with_capacity(recipients.len());
    for (i, der) in recipients.iter().enumerate() {
        // Un certificat donné deux fois ne compte qu'une.
        if keys.iter().any(|(cert, _)| cert.der() == der.as_slice()) {
            continue;
        }
        let cert = Cert::from_der(der).ok_or(EncryptError::UnsupportedCertificate(i))?;
        let key = public_key(&cert).ok_or(EncryptError::UnsupportedCertificate(i))?;
        keys.push((cert, key));
    }

    let mut content_key = [0u8; 32];
    let mut iv = [0u8; 16];
    getrandom::fill(&mut content_key).map_err(|_| EncryptError::Failed)?;
    getrandom::fill(&mut iv).map_err(|_| EncryptError::Failed)?;
    let encrypted =
        cbc::Encryptor::<aes::Aes256>::new_from_slices(&content_key, &iv).map_err(|_| EncryptError::Failed)?.encrypt_padded_vec::<Pkcs7>(content);

    let mut rng = rand::rng();
    let mut infos: Vec<Vec<u8>> = Vec::with_capacity(keys.len());
    let mut agreement = false;
    for (cert, key) in &keys {
        match key {
            PublicKey::Rsa(key) => {
                let wrapped = key.encrypt(&mut rng, rsa::Pkcs1v15Encrypt, &content_key).map_err(|_| EncryptError::Failed)?;
                let serial = tlv(0x02, cert.serial());
                let issuer_and_serial = tlv(0x30, &[cert.issuer(), serial.as_slice()].concat());
                let algorithm = tlv(0x30, &[OID_RSA, NULL].concat());
                let wrapped = tlv(0x04, &wrapped);
                // KeyTransRecipientInfo, version 0 (désigné par émetteur et numéro de série).
                infos.push(tlv(0x30, &[VERSION_0, issuer_and_serial.as_slice(), algorithm.as_slice(), wrapped.as_slice()].concat()));
            }
            PublicKey::P256(public) => {
                let secret = ephemeral::<p256::NistP256>()?;
                let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), public.as_affine());
                let point = secret.public_key().to_sec1_bytes();
                infos.push(key_agreement(cert, &content_key, shared.raw_secret_bytes(), &point, false)?);
                agreement = true;
            }
            PublicKey::P384(public) => {
                let secret = ephemeral::<p384::NistP384>()?;
                let shared = p384::ecdh::diffie_hellman(secret.to_nonzero_scalar(), public.as_affine());
                let point = secret.public_key().to_sec1_bytes();
                infos.push(key_agreement(cert, &content_key, shared.raw_secret_bytes(), &point, true)?);
                agreement = true;
            }
        }
    }
    // DER : les éléments d'un SET OF rangés par ordre de leurs octets (X.690 §11.6).
    infos.sort();
    let recipient_infos = tlv(0x31, &infos.concat());
    let iv = tlv(0x04, &iv);
    let content_algorithm = tlv(0x30, &[OID_AES256_CBC, iv.as_slice()].concat());
    // encryptedContent [0] IMPLICIT OCTET STRING.
    let encrypted = tlv(0x80, &encrypted);
    let encrypted_content_info = tlv(0x30, &[OID_DATA, content_algorithm.as_slice(), encrypted.as_slice()].concat());
    // EnvelopedData version 0 quand tous les destinataires sont en version 0 (RSA), 2 avec un
    // KeyAgreeRecipientInfo (RFC 5652 §6.1).
    let version = if agreement { VERSION_2 } else { VERSION_0 };
    let enveloped = tlv(0x30, &[version, recipient_infos.as_slice(), encrypted_content_info.as_slice()].concat());
    let enveloped = tlv(0xa0, &enveloped);
    Ok(tlv(0x30, &[OID_ENVELOPED_DATA, enveloped.as_slice()].concat()))
}
