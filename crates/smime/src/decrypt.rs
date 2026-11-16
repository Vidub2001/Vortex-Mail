// SPDX-FileCopyrightText: Vortex Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lecture des messages chiffrés (lots D3a et G2b de Vortex Mail ; pas dans MailCraft).
//!
//! `EnvelopedData` (RFC 5652 §6) et `AuthEnvelopedData` (RFC 5083). Clé de contenu :
//! - transportée en RSA (`KeyTransRecipientInfo`) : RSAES-PKCS1-v1_5, ou RSAES-OAEP
//!   (RFC 3560) sur SHA-1 ou SHA-256 ;
//! - ou convenue par ECDH sur P-256 ou P-384 (`KeyAgreeRecipientInfo`, RFC 5753, lot G2b) :
//!   clé éphémère de l'expéditeur, dérivation ANSI X9.63 sur SHA-1 ou SHA-2, clé enveloppée
//!   en AES Key Wrap (RFC 3394).
//!
//! Contenu chiffré en AES-128, AES-192 ou AES-256 CBC (RFC 3565), en 3DES CBC (anciens
//! logiciels), ou en AES-GCM authentifié (RFC 5084, lot G2b).
//!
//! L'entrée vient de l'extérieur : lue comme pour une signature (BER rendu en DER, profondeur
//! bornée, aucune panique), le contenu déchiffré limité à [`MAX_CONTENT`] octets.

use std::fmt;

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeDecrypt, KeyIvInit};
use const_oid::ObjectIdentifier;
use der::Decode;
use spki::AlgorithmIdentifierOwned;

use crate::ber::{self, Tlv};
use crate::cert::Cert;

const ENVELOPED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.3");
const AUTH_ENVELOPED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.23");
const RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const RSAES_OAEP: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.7");
const MGF1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.8");
const SHA1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.14.3.2.26");
const SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
const AES128_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.2");
const AES192_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.22");
const AES256_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.42");
const DES_EDE3_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.3.7");
const AES128_GCM: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.6");
const AES192_GCM: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.26");
const AES256_GCM: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.46");
const AES128_WRAP: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.5");
const AES192_WRAP: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.25");
const AES256_WRAP: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.45");
/// ECDH (RFC 5753 §7.1.4, RFC 8418) : dérivation ANSI X9.63 sur SHA-1, SHA-224, SHA-256,
/// SHA-384 ou SHA-512 ; la variante « cofactor » revient au même sur P-256 et P-384.
const ECDH_SHA1_KDF: [ObjectIdentifier; 2] =
    [ObjectIdentifier::new_unwrap("1.3.133.16.840.63.0.2"), ObjectIdentifier::new_unwrap("1.3.133.16.840.63.0.3")];
const ECDH_SHA256_KDF: [ObjectIdentifier; 2] = [ObjectIdentifier::new_unwrap("1.3.132.1.11.1"), ObjectIdentifier::new_unwrap("1.3.132.1.14.1")];
const ECDH_SHA384_KDF: [ObjectIdentifier; 2] = [ObjectIdentifier::new_unwrap("1.3.132.1.11.2"), ObjectIdentifier::new_unwrap("1.3.132.1.14.2")];
const ECDH_SHA512_KDF: [ObjectIdentifier; 2] = [ObjectIdentifier::new_unwrap("1.3.132.1.11.3"), ObjectIdentifier::new_unwrap("1.3.132.1.14.3")];

/// Contenu chiffré lu au plus (octets).
pub const MAX_CONTENT: usize = 64 * 1024 * 1024;
/// Destinataires examinés au plus dans un message.
pub const MAX_RECIPIENTS: usize = 64;

/// Étiquettes DER lues ici.
const SEQUENCE: u8 = 0x30;
const SET: u8 = 0x31;
const OBJECT_IDENTIFIER: u8 = 0x06;
const OCTET_STRING: u8 = 0x04;
const CONTEXT_0_PRIMITIVE: u8 = 0x80;
const CONTEXT_0_CONSTRUCTED: u8 = 0xa0;
const CONTEXT_1_CONSTRUCTED: u8 = 0xa1;
const BIT_STRING: u8 = 0x03;
const INTEGER: u8 = 0x02;

/// Clé privée qui déchiffre (celle d'une [`crate::Identity`]).
pub(crate) enum PrivateKey<'a> {
    Rsa(&'a rsa::PrivateKey),
    P256(&'a p256::NonZeroScalar),
    P384(&'a p384::NonZeroScalar),
}
/// Pourquoi un message chiffré n'a pas pu être lu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecryptError {
    /// Ce n'est pas un `EnvelopedData`, ou il est illisible.
    Unreadable,
    /// Un algorithme non pris en charge (courbe autre que P-256 ou P-384, RC2…).
    Unsupported,
    /// Le message n'est chiffré pour aucun des certificats de cette identité.
    NotForMe,
    /// La clé ne déchiffre pas le message (autre clé, message abîmé).
    Failed,
}

impl fmt::Display for DecryptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DecryptError::Unreadable => "unreadable encrypted message",
            DecryptError::Unsupported => "unsupported encryption algorithm",
            DecryptError::NotForMe => "the message is encrypted for another certificate",
            DecryptError::Failed => "the message could not be decrypted",
        })
    }
}

impl std::error::Error for DecryptError {}

/// Le CMS `cms` est-il un message chiffré (`EnvelopedData` ou `AuthEnvelopedData`) ?
pub fn is_enveloped(cms: &[u8]) -> bool {
    content_info(cms).is_some_and(|(oid, _)| oid == ENVELOPED_DATA || oid == AUTH_ENVELOPED_DATA)
}

/// Le type et le contenu (en DER) d'un `ContentInfo`.
fn content_info(cms: &[u8]) -> Option<(ObjectIdentifier, Vec<u8>)> {
    let der = ber::to_der(cms)?;
    let (outer, _) = ber::read(&der)?;
    if outer.tag != SEQUENCE {
        return None;
    }
    let parts = ber::children(outer.value)?;
    let oid = parts.first().filter(|t| t.tag == OBJECT_IDENTIFIER)?;
    let oid = ObjectIdentifier::from_der(oid.whole).ok()?;
    // content [0] EXPLICIT : l'élément qu'il enveloppe.
    let content = parts.get(1).filter(|t| t.tag == CONTEXT_0_CONSTRUCTED)?;
    Some((oid, content.value.to_vec()))
}

/// Un destinataire : comment il est désigné, l'algorithme et la clé de contenu chiffrée.
struct Recipient<'a> {
    rid: Tlv<'a>,
    encrypted_key: &'a [u8],
    how: KeyMethod<'a>,
}

/// Comment la clé de contenu parvient au destinataire.
enum KeyMethod<'a> {
    /// `KeyTransRecipientInfo` : chiffrée avec sa clé RSA.
    Transport(AlgorithmIdentifierOwned),
    /// `KeyAgreeRecipientInfo` : clé convenue par ECDH avec la clé éphémère de l'expéditeur
    /// (point SEC1), le matériel `ukm` facultatif, l'algorithme de dérivation et l'algorithme
    /// d'enveloppe de la clé, tel qu'écrit (il entre dans la dérivation).
    Agreement { originator: &'a [u8], ukm: Option<&'a [u8]>, kdf: ObjectIdentifier, wrap: Tlv<'a> },
}

impl Recipient<'_> {
    /// Ce destinataire désigne-t-il le certificat `cert` (émetteur et numéro de série, ou
    /// identifiant de clé) ?
    fn names(&self, cert: &Cert) -> bool {
        match self.rid.tag {
            SEQUENCE => ber::children(self.rid.value)
                .is_some_and(|parts| matches!(parts.as_slice(), [issuer, serial] if issuer.whole == cert.issuer() && serial.value == cert.serial())),
            // subjectKeyIdentifier [0] IMPLICIT OCTET STRING.
            CONTEXT_0_PRIMITIVE => cert.key_id().is_some_and(|id| id == self.rid.value),
            // rKeyId [0] IMPLICIT RecipientKeyIdentifier (ECDH) : l'identifiant en tête.
            CONTEXT_0_CONSTRUCTED => ber::children(self.rid.value)
                .is_some_and(|parts| parts.first().is_some_and(|id| id.tag == OCTET_STRING && cert.key_id().is_some_and(|k| k == id.value))),
            _ => false,
        }
    }
}

/// Le message chiffré, lu : ses destinataires, l'algorithme du contenu, le contenu chiffré et,
/// pour un `AuthEnvelopedData`, les attributs authentifiés (en DER, en `SET`) et le code
/// d'authentification.
struct Enveloped<'a> {
    recipients: Vec<Recipient<'a>>,
    other_recipients: bool,
    content_algorithm: AlgorithmIdentifierOwned,
    content: Vec<u8>,
    auth: Option<(Vec<u8>, &'a [u8])>,
}

/// Un `KeyAgreeRecipientInfo` (RFC 5652 §6.2.2) : ses destinataires, ajoutés à `out`.
fn key_agreement<'a>(value: &'a [u8], out: &mut Vec<Recipient<'a>>) -> Result<(), DecryptError> {
    let fields = ber::children(value).ok_or(DecryptError::Unreadable)?;
    let [_version, originator, rest @ ..] = fields.as_slice() else { return Err(DecryptError::Unreadable) };
    // originator [0] EXPLICIT : originatorKey [1] IMPLICIT { algorithm, publicKey BIT STRING }.
    let key = ber::children(originator.value)
        .and_then(|inner| inner.into_iter().next())
        .filter(|t| originator.tag == CONTEXT_0_CONSTRUCTED && t.tag == CONTEXT_1_CONSTRUCTED)
        .ok_or(DecryptError::Unsupported)?;
    let public = ber::children(key.value)
        .and_then(|parts| parts.into_iter().nth(1))
        .filter(|t| t.tag == BIT_STRING)
        .and_then(|t| t.value.split_first())
        .filter(|(unused, _)| **unused == 0)
        .map(|(_, point)| point)
        .ok_or(DecryptError::Unreadable)?;
    let (ukm, rest) = match rest {
        [first, more @ ..] if first.tag == CONTEXT_1_CONSTRUCTED => {
            let ukm =
                ber::children(first.value).and_then(|i| i.into_iter().next()).filter(|t| t.tag == OCTET_STRING).ok_or(DecryptError::Unreadable)?;
            (Some(ukm.value), more)
        }
        _ => (None, rest),
    };
    let [algorithm, keys] = rest else { return Err(DecryptError::Unreadable) };
    let algorithm_parts = ber::children(algorithm.value).ok_or(DecryptError::Unreadable)?;
    let [kdf, wrap] = algorithm_parts.as_slice() else { return Err(DecryptError::Unreadable) };
    let kdf = ObjectIdentifier::from_der(kdf.whole).map_err(|_| DecryptError::Unreadable)?;
    for entry in ber::children(keys.value).ok_or(DecryptError::Unreadable)?.into_iter().take(MAX_RECIPIENTS) {
        let parts = ber::children(entry.value).ok_or(DecryptError::Unreadable)?;
        let [rid, key] = parts.as_slice() else { return Err(DecryptError::Unreadable) };
        if key.tag != OCTET_STRING {
            return Err(DecryptError::Unreadable);
        }
        out.push(Recipient { rid: *rid, encrypted_key: key.value, how: KeyMethod::Agreement { originator: public, ukm, kdf, wrap: *wrap } });
    }
    Ok(())
}

fn enveloped(der: &[u8]) -> Result<Enveloped<'_>, DecryptError> {
    let unreadable = |_| DecryptError::Unreadable;
    let (data, _) = ber::read(der).ok_or(DecryptError::Unreadable)?;
    let parts = ber::children(data.value).ok_or(DecryptError::Unreadable)?;
    // version, originatorInfo [0] facultatif, recipientInfos SET, encryptedContentInfo.
    let mut rest = parts.iter().skip(1).skip_while(|t| t.tag == CONTEXT_0_CONSTRUCTED);
    let infos = rest.next().filter(|t| t.tag == SET).ok_or(DecryptError::Unreadable)?;
    let content_info = rest.next().filter(|t| t.tag == SEQUENCE).ok_or(DecryptError::Unreadable)?;

    let mut recipients = Vec::new();
    let mut other_recipients = false;
    for info in ber::children(infos.value).ok_or(DecryptError::Unreadable)?.into_iter().take(MAX_RECIPIENTS) {
        match info.tag {
            SEQUENCE => {
                let fields = ber::children(info.value).ok_or(DecryptError::Unreadable)?;
                let [_version, rid, algorithm, key] = fields.as_slice() else { return Err(DecryptError::Unreadable) };
                if key.tag != OCTET_STRING {
                    return Err(DecryptError::Unreadable);
                }
                let algorithm = AlgorithmIdentifierOwned::from_der(algorithm.whole).map_err(unreadable)?;
                recipients.push(Recipient { rid: *rid, encrypted_key: key.value, how: KeyMethod::Transport(algorithm) });
            }
            // kari [1] IMPLICIT (lot G2b) ; un destinataire illisible n'empêche pas les autres.
            CONTEXT_1_CONSTRUCTED => {
                if key_agreement(info.value, &mut recipients).is_err() {
                    other_recipients = true;
                }
            }
            // kekri [2], pwri [3], ori [4] : pas pris en charge.
            _ => other_recipients = true,
        }
    }
    let after_content: Vec<&Tlv<'_>> = rest.collect();

    let fields = ber::children(content_info.value).ok_or(DecryptError::Unreadable)?;
    let [_type, algorithm, rest @ ..] = fields.as_slice() else { return Err(DecryptError::Unreadable) };
    let content_algorithm = AlgorithmIdentifierOwned::from_der(algorithm.whole).map_err(unreadable)?;
    let content = match rest.first() {
        // encryptedContent [0] IMPLICIT OCTET STRING, d'un tenant ou en morceaux.
        Some(t) if t.tag == CONTEXT_0_PRIMITIVE => t.value.to_vec(),
        Some(t) if t.tag == CONTEXT_0_CONSTRUCTED => {
            let pieces = ber::children(t.value).ok_or(DecryptError::Unreadable)?;
            if pieces.iter().any(|p| p.tag != OCTET_STRING) {
                return Err(DecryptError::Unreadable);
            }
            pieces.iter().flat_map(|p| p.value.iter().copied()).collect()
        }
        // Contenu détaché : pas pour un message.
        _ => return Err(DecryptError::Unreadable),
    };
    if content.len() > MAX_CONTENT {
        return Err(DecryptError::Unreadable);
    }
    // AuthEnvelopedData : authAttrs [1] IMPLICIT facultatif (authentifiés tels qu'en DER, avec
    // l'étiquette d'un SET, RFC 5083 §2.2), puis mac OCTET STRING.
    let (attributes, mac) = match after_content.as_slice() {
        [attrs, mac, ..] if attrs.tag == CONTEXT_1_CONSTRUCTED && mac.tag == OCTET_STRING => {
            let mut set = attrs.whole.to_vec();
            if let Some(tag) = set.first_mut() {
                *tag = SET;
            }
            (Some(set), Some(mac.value))
        }
        [mac, ..] if mac.tag == OCTET_STRING => (Some(Vec::new()), Some(mac.value)),
        _ => (None, None),
    };
    let auth = attributes.zip(mac);
    Ok(Enveloped { recipients, other_recipients, content_algorithm, content, auth })
}

/// La clé convenue par ECDH avec la clé éphémère `originator`, dérivée selon `kdf` (ANSI X9.63,
/// RFC 5753 §7.2) pour déchiffrer la clé enveloppée `encrypted` avec l'algorithme `wrap`.
fn agreed_key(
    key: &PrivateKey<'_>,
    originator: &[u8],
    ukm: Option<&[u8]>,
    kdf: ObjectIdentifier,
    wrap: &Tlv<'_>,
    encrypted: &[u8],
) -> Result<Vec<u8>, DecryptError> {
    let shared: Vec<u8> = match key {
        PrivateKey::P256(secret) => {
            let public = p256::PublicKey::from_sec1_bytes(originator).map_err(|_| DecryptError::Failed)?;
            p256::ecdh::diffie_hellman(*secret, public.as_affine()).raw_secret_bytes().to_vec()
        }
        PrivateKey::P384(secret) => {
            let public = p384::PublicKey::from_sec1_bytes(originator).map_err(|_| DecryptError::Failed)?;
            p384::ecdh::diffie_hellman(*secret, public.as_affine()).raw_secret_bytes().to_vec()
        }
        PrivateKey::Rsa(_) => return Err(DecryptError::Failed),
    };
    let wrap_oid = ber::children(wrap.value)
        .and_then(|parts| parts.into_iter().next())
        .and_then(|oid| ObjectIdentifier::from_der(oid.whole).ok())
        .ok_or(DecryptError::Unreadable)?;
    let length: usize = match wrap_oid {
        AES128_WRAP => 16,
        AES192_WRAP => 24,
        AES256_WRAP => 32,
        _ => return Err(DecryptError::Unsupported),
    };
    let shared_info = shared_info(wrap.whole, ukm, length).ok_or(DecryptError::Unsupported)?;
    let kek = if ECDH_SHA1_KDF.contains(&kdf) {
        x963::<sha1::Sha1>(&shared, &shared_info, length)
    } else if ECDH_SHA256_KDF.contains(&kdf) {
        x963::<sha2::Sha256>(&shared, &shared_info, length)
    } else if ECDH_SHA384_KDF.contains(&kdf) {
        x963::<sha2::Sha384>(&shared, &shared_info, length)
    } else if ECDH_SHA512_KDF.contains(&kdf) {
        x963::<sha2::Sha512>(&shared, &shared_info, length)
    } else {
        return Err(DecryptError::Unsupported);
    };
    let plain = match length {
        16 => aes_unwrap::<aes::Aes128>(&kek, encrypted),
        24 => aes_unwrap::<aes::Aes192>(&kek, encrypted),
        _ => aes_unwrap::<aes::Aes256>(&kek, encrypted),
    };
    plain.ok_or(DecryptError::Failed)
}

/// `ECC-CMS-SharedInfo` (RFC 5753 §7.2) : keyInfo (l'algorithme d'enveloppe `wrap`, en DER,
/// tel qu'écrit dans le message), entityUInfo [0] EXPLICIT facultatif, suppPubInfo [2]
/// EXPLICIT : la longueur de la clé en bits.
pub(crate) fn shared_info(wrap: &[u8], ukm: Option<&[u8]>, length: usize) -> Option<Vec<u8>> {
    let bits = u32::try_from(length.checked_mul(8)?).ok()?.to_be_bytes();
    let mut info = wrap.to_vec();
    if let Some(ukm) = ukm {
        info.extend(ber::tlv(CONTEXT_0_CONSTRUCTED, &ber::tlv(OCTET_STRING, ukm)));
    }
    info.extend(ber::tlv(0xa2, &ber::tlv(OCTET_STRING, &bits)));
    Some(ber::tlv(SEQUENCE, &info))
}

/// Dérivation ANSI X9.63 (SEC 1 §3.6.1) : `length` octets de Hash(Z ‖ compteur ‖ SharedInfo).
pub(crate) fn x963<H: sha2::Digest>(shared: &[u8], info: &[u8], length: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(length.saturating_add(64));
    let mut counter: u32 = 1;
    while out.len() < length {
        let mut hash = H::new();
        hash.update(shared);
        hash.update(counter.to_be_bytes());
        hash.update(info);
        out.extend_from_slice(&hash.finalize());
        counter = counter.saturating_add(1);
    }
    out.truncate(length);
    out
}

/// AES Key Wrap (RFC 3394 §2.2.1) de la clé `key` (multiple de 8 octets, 16 au moins) avec
/// `kek` (lot G2b, envoi).
pub(crate) fn aes_wrap<C>(kek: &[u8], key: &[u8]) -> Option<Vec<u8>>
where
    C: aes::cipher::BlockCipherEncrypt + aes::cipher::KeyInit + aes::cipher::BlockSizeUser<BlockSize = aes::cipher::consts::U16>,
{
    if !key.len().is_multiple_of(8) || key.len() < 16 {
        return None;
    }
    let cipher = C::new_from_slice(kek).ok()?;
    let mut a = [0xa6u8; 8];
    let mut r: Vec<[u8; 8]> = key.as_chunks::<8>().0.to_vec();
    let n = u64::try_from(r.len()).ok()?;
    let mut block = aes::Block::default();
    for j in 0..6u64 {
        for (index, item) in r.iter_mut().enumerate() {
            let i = u64::try_from(index).ok()?.checked_add(1)?;
            let mut b = [0u8; 16];
            for (slot, x) in b.iter_mut().zip(a.iter().chain(item.iter())) {
                *slot = *x;
            }
            block.copy_from_slice(&b);
            cipher.encrypt_block(&mut block);
            let (high, low) = block.split_at(8);
            let t = n.checked_mul(j)?.checked_add(i)?.to_be_bytes();
            for ((slot, x), y) in a.iter_mut().zip(high.iter()).zip(t.iter()) {
                *slot = x ^ y;
            }
            item.copy_from_slice(low);
        }
    }
    let mut out = a.to_vec();
    out.extend(r.concat());
    Some(out)
}

/// AES Key Unwrap (RFC 3394 §2.2.2) : `None` si l'enveloppe est abîmée ou d'une autre clé.
fn aes_unwrap<C>(kek: &[u8], wrapped: &[u8]) -> Option<Vec<u8>>
where
    C: aes::cipher::BlockCipherDecrypt + aes::cipher::KeyInit + aes::cipher::BlockSizeUser<BlockSize = aes::cipher::consts::U16>,
{
    if !wrapped.len().is_multiple_of(8) || wrapped.len() < 24 {
        return None;
    }
    let cipher = C::new_from_slice(kek).ok()?;
    let (head, rest) = wrapped.split_first_chunk::<8>()?;
    let mut a = *head;
    let mut r: Vec<[u8; 8]> = rest.as_chunks::<8>().0.to_vec();
    let n = u64::try_from(r.len()).ok()?;
    let mut block = aes::Block::default();
    for j in (0..6u64).rev() {
        for (index, item) in r.iter_mut().enumerate().rev() {
            let i = u64::try_from(index).ok()?.checked_add(1)?;
            let t = n.checked_mul(j)?.checked_add(i)?.to_be_bytes();
            let mut b = [0u8; 16];
            for ((slot, x), y) in b.iter_mut().zip(a.iter()).zip(t.iter()) {
                *slot = x ^ y;
            }
            for (slot, x) in b.iter_mut().skip(8).zip(item.iter()) {
                *slot = *x;
            }
            block.copy_from_slice(&b);
            cipher.decrypt_block(&mut block);
            let (high, low) = block.split_at(8);
            a.copy_from_slice(high);
            item.copy_from_slice(low);
        }
    }
    // Valeur initiale par défaut (A6…A6), comparée sans s'arrêter au premier écart.
    if a.iter().fold(0u8, |acc, x| acc | (x ^ 0xa6)) != 0 {
        return None;
    }
    Some(r.concat())
}

/// La clé de contenu, déchiffrée avec la clé RSA `key` selon `algorithm`.
fn unwrap_key(key: &rsa::PrivateKey, algorithm: &AlgorithmIdentifierOwned, encrypted: &[u8]) -> Result<Vec<u8>, DecryptError> {
    let plain = match algorithm.oid {
        RSA => key.decrypt(rsa::Pkcs1v15Encrypt, encrypted),
        RSAES_OAEP => match oaep_hashes(algorithm)? {
            (SHA1, SHA1) => key.decrypt(rsa::Oaep::<sha1::Sha1>::new(), encrypted),
            (SHA256, SHA256) => key.decrypt(rsa::Oaep::<sha2::Sha256>::new(), encrypted),
            _ => return Err(DecryptError::Unsupported),
        },
        _ => return Err(DecryptError::Unsupported),
    };
    plain.map_err(|_| DecryptError::Failed)
}

/// Les hachages de RSAES-OAEP (RFC 4055 §4.1) : celui de l'étiquette et celui de MGF1, SHA-1
/// par défaut. Une étiquette (`pSourceFunc`) autre que vide n'est pas prise en charge.
fn oaep_hashes(algorithm: &AlgorithmIdentifierOwned) -> Result<(ObjectIdentifier, ObjectIdentifier), DecryptError> {
    let Some(params) = algorithm.parameters.as_ref() else { return Ok((SHA1, SHA1)) };
    let der = der::Encode::to_der(params).map_err(|_| DecryptError::Unreadable)?;
    let (sequence, _) = ber::read(&der).ok_or(DecryptError::Unreadable)?;
    let mut hashes = (SHA1, SHA1);
    for field in ber::children(sequence.value).ok_or(DecryptError::Unreadable)? {
        let inner = AlgorithmIdentifierOwned::from_der(field.value).map_err(|_| DecryptError::Unreadable)?;
        match field.tag {
            CONTEXT_0_CONSTRUCTED => hashes.0 = inner.oid,
            CONTEXT_1_CONSTRUCTED => {
                if inner.oid != MGF1 {
                    return Err(DecryptError::Unsupported);
                }
                let mgf_hash = inner.parameters.as_ref().ok_or(DecryptError::Unreadable)?;
                let der = der::Encode::to_der(mgf_hash).map_err(|_| DecryptError::Unreadable)?;
                hashes.1 = AlgorithmIdentifierOwned::from_der(&der).map_err(|_| DecryptError::Unreadable)?.oid;
            }
            _ => return Err(DecryptError::Unsupported),
        }
    }
    Ok(hashes)
}

/// Le contenu d'un `AuthEnvelopedData`, déchiffré et authentifié en AES-GCM (RFC 5084) :
/// paramètres `GCMParameters` (nonce, longueur du code, 12 par défaut), attributs authentifiés
/// `aad`, code `mac`.
fn decrypt_gcm(algorithm: &AlgorithmIdentifierOwned, key: &[u8], content: &[u8], aad: &[u8], mac: &[u8]) -> Result<Vec<u8>, DecryptError> {
    use aes_gcm::KeyInit as _;
    use aes_gcm::aead::consts::{U12, U16};
    use aes_gcm::aead::{Aead, Payload};
    let params = algorithm.parameters.as_ref().ok_or(DecryptError::Unreadable)?;
    let params = der::Encode::to_der(params).map_err(|_| DecryptError::Unreadable)?;
    let (sequence, _) = ber::read(&params).ok_or(DecryptError::Unreadable)?;
    let fields = ber::children(sequence.value).ok_or(DecryptError::Unreadable)?;
    let nonce = fields.first().filter(|t| t.tag == OCTET_STRING).ok_or(DecryptError::Unreadable)?.value;
    let icv_len = match fields.get(1) {
        Some(t) if t.tag == INTEGER => match t.value {
            [n] => usize::from(*n),
            _ => return Err(DecryptError::Unsupported),
        },
        _ => 12,
    };
    if mac.len() != icv_len {
        return Err(DecryptError::Failed);
    }
    let nonce = aes_gcm::Nonce::<U12>::try_from(nonce).map_err(|_| DecryptError::Unsupported)?;
    let sealed: Vec<u8> = [content, mac].concat();
    let payload = Payload { msg: &sealed, aad };
    macro_rules! open {
        ($aes:ty, $tag:ty) => {
            aes_gcm::AesGcm::<$aes, U12, $tag>::new_from_slice(key).map_err(|_| DecryptError::Failed)?.decrypt(&nonce, payload)
        };
    }
    let plain = match (algorithm.oid, icv_len) {
        (AES128_GCM, 16) => open!(aes::Aes128, U16),
        (AES192_GCM, 16) => open!(aes::Aes192, U16),
        (AES256_GCM, 16) => open!(aes::Aes256, U16),
        (AES128_GCM, 12) => open!(aes::Aes128, U12),
        (AES192_GCM, 12) => open!(aes::Aes192, U12),
        (AES256_GCM, 12) => open!(aes::Aes256, U12),
        _ => return Err(DecryptError::Unsupported),
    };
    plain.map_err(|_| DecryptError::Failed)
}

/// Le contenu déchiffré avec la clé de contenu `key` selon `algorithm` (IV en paramètre).
fn decrypt_content(algorithm: &AlgorithmIdentifierOwned, key: &[u8], content: &[u8]) -> Result<Vec<u8>, DecryptError> {
    let params = algorithm.parameters.as_ref().ok_or(DecryptError::Unreadable)?;
    let params = der::Encode::to_der(params).map_err(|_| DecryptError::Unreadable)?;
    let iv = der::asn1::OctetString::from_der(&params).map_err(|_| DecryptError::Unreadable)?;
    let iv = iv.as_bytes();
    let plain = match algorithm.oid {
        AES128_CBC => cbc::Decryptor::<aes::Aes128>::new_from_slices(key, iv).map_err(|_| DecryptError::Failed)?.decrypt_padded_vec::<Pkcs7>(content),
        AES192_CBC => cbc::Decryptor::<aes::Aes192>::new_from_slices(key, iv).map_err(|_| DecryptError::Failed)?.decrypt_padded_vec::<Pkcs7>(content),
        AES256_CBC => cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv).map_err(|_| DecryptError::Failed)?.decrypt_padded_vec::<Pkcs7>(content),
        DES_EDE3_CBC => {
            cbc::Decryptor::<des::TdesEde3>::new_from_slices(key, iv).map_err(|_| DecryptError::Failed)?.decrypt_padded_vec::<Pkcs7>(content)
        }
        _ => return Err(DecryptError::Unsupported),
    };
    plain.map_err(|_| DecryptError::Failed)
}

/// La clé de contenu d'un destinataire, avec la clé privée `key`.
fn content_key(key: &PrivateKey<'_>, recipient: &Recipient<'_>) -> Result<Vec<u8>, DecryptError> {
    match (&recipient.how, key) {
        (KeyMethod::Transport(algorithm), PrivateKey::Rsa(rsa)) => unwrap_key(rsa, algorithm, recipient.encrypted_key),
        (KeyMethod::Agreement { originator, ukm, kdf, wrap }, PrivateKey::P256(_) | PrivateKey::P384(_)) => {
            agreed_key(key, originator, *ukm, *kdf, wrap, recipient.encrypted_key)
        }
        // Destinataire d'un autre type de clé que la nôtre.
        _ => Err(DecryptError::Failed),
    }
}

/// Le contenu d'un message chiffré `cms` (`EnvelopedData` ou `AuthEnvelopedData`, en BER ou
/// DER), déchiffré avec la clé privée `key` du certificat `cert` : l'entité MIME qu'il porte.
pub(crate) fn decrypt(key: &PrivateKey<'_>, cert: &Cert, cms: &[u8]) -> Result<Vec<u8>, DecryptError> {
    let (oid, content) = content_info(cms).ok_or(DecryptError::Unreadable)?;
    let authenticated = oid == AUTH_ENVELOPED_DATA;
    if !authenticated && oid != ENVELOPED_DATA {
        return Err(DecryptError::Unreadable);
    }
    let data = enveloped(&content)?;
    if authenticated && data.auth.is_none() {
        return Err(DecryptError::Unreadable);
    }
    // Le destinataire qui nomme ce certificat ; à défaut (identifiant écrit autrement), chacun.
    let named: Vec<&Recipient<'_>> = data.recipients.iter().filter(|r| r.names(cert)).collect();
    if named.is_empty() && data.recipients.is_empty() {
        return Err(if data.other_recipients { DecryptError::Unsupported } else { DecryptError::NotForMe });
    }
    let candidates: Vec<&Recipient<'_>> = if named.is_empty() { data.recipients.iter().collect() } else { named.clone() };
    let mut last = DecryptError::Failed;
    for recipient in candidates {
        let opened = content_key(key, recipient).and_then(|content_key| match (&data.auth, authenticated) {
            (Some((aad, mac)), true) => decrypt_gcm(&data.content_algorithm, &content_key, &data.content, aad, mac),
            _ => decrypt_content(&data.content_algorithm, &content_key, &data.content),
        });
        match opened {
            Ok(plain) => return Ok(plain),
            Err(e) => last = e,
        }
    }
    // Aucun destinataire ne nommait ce certificat, et aucun ne s'est déchiffré.
    Err(if named.is_empty() && last == DecryptError::Failed { DecryptError::NotForMe } else { last })
}

/// [`aes_unwrap`] et [`aes_wrap`] en AES-128, pour les vecteurs de test de la RFC 3394.
#[cfg(test)]
pub(crate) fn aes_unwrap_for_tests(kek: &[u8], wrapped: &[u8]) -> Option<Vec<u8>> {
    aes_unwrap::<aes::Aes128>(kek, wrapped)
}

#[cfg(test)]
pub(crate) fn aes_wrap_for_tests(kek: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    aes_wrap::<aes::Aes128>(kek, key)
}
