// SPDX-FileCopyrightText: Vortex Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signature des messages envoyés (lot D2 de Vortex Mail ; pas dans MailCraft).
//!
//! - [`read_pkcs12`] lit la clé privée et les certificats d'un fichier PKCS #12 (RFC 7292,
//!   crate `p12-keystore`) ;
//! - [`Identity::new`] retrouve le certificat de la clé (celui dont la clé publique vérifie
//!   une signature faite avec elle) et range la chaîne, le signataire d'abord ;
//! - [`Identity::sign_detached`] écrit la signature CMS détachée (RFC 5652, RFC 8551 §3.5.3)
//!   d'un contenu : attributs signés `contentType`, `signingTime` et `messageDigest`,
//!   certificats joints, puis relue par [`crate::check`] avant d'être rendue.
//!
//! Clés prises : RSA de 2048 à 8192 bits (PKCS #1 v1.5, SHA-256), ECDSA P-256 (SHA-256) et
//! P-384 (SHA-384). Les signatures RSA PKCS #1 v1.5 et ECDSA (RFC 6979) sont déterministes :
//! aucun aléa n'est demandé.

use std::fmt;

use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::content_info::{CmsVersion, ContentInfo};
use cms::signed_data::{CertificateSet, EncapsulatedContentInfo, SignedData, SignerIdentifier, SignerInfo, SignerInfos};
use const_oid::ObjectIdentifier;
use der::asn1::{OctetString, SetOfVec, UtcTime};
use der::{Any, Decode, Encode};
use spki::AlgorithmIdentifierOwned;
use x509_cert::Certificate;
use x509_cert::attr::Attribute;
use x509_cert::time::Time;

use crate::algorithms::{self, Hash};
use crate::cert::Cert;
use crate::signature::{Coverage, Signer, Timestamp, Verdict};
use crate::{Anchors, Context, Signed};

const DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
const SIGNED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
const CONTENT_TYPE: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.3");
const MESSAGE_DIGEST: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
const SIGNING_TIME: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.5");
const SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
const SHA384: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.2");
const SHA256_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
const SHA256_ECDSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
const SHA384_ECDSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.3");

/// Taille d'un fichier PKCS #12 lue au plus.
pub const MAX_PKCS12_BYTES: usize = 1024 * 1024;
/// Certificats gardés au plus avec une clé (le signataire et ses autorités).
pub const MAX_CHAIN: usize = 10;

/// Pourquoi une identité ne peut pas servir, ou un message pas être signé.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignError {
    /// Le fichier PKCS #12 est illisible, ou son mot de passe est faux.
    Unreadable,
    /// Pas de clé privée, ou aucun certificat qui lui corresponde.
    NoKey,
    /// Une clé d'un autre type que RSA (2048 à 8192 bits), ECDSA P-256 ou P-384.
    UnsupportedKey,
    /// Le certificat ne sert pas à signer du courrier.
    NotForEmail,
    /// Le certificat est expiré, ou pas encore valide.
    Expired,
    /// La signature n'a pas pu être faite, ou elle ne se vérifie pas.
    Failed,
}

impl fmt::Display for SignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SignError::Unreadable => "unreadable PKCS #12 file or wrong password",
            SignError::NoKey => "no private key with its certificate",
            SignError::UnsupportedKey => "unsupported key (RSA of 2048 to 8192 bits, ECDSA P-256 or P-384)",
            SignError::NotForEmail => "the certificate is not for signing mail",
            SignError::Expired => "the certificate is expired or not valid yet",
            SignError::Failed => "the signature could not be made",
        })
    }
}

impl std::error::Error for SignError {}

/// Clé et certificats lus d'un fichier PKCS #12 : la clé en PKCS #8 (DER), les certificats en
/// DER, dans l'ordre du fichier.
///
/// # Errors
///
/// [`SignError::Unreadable`] (fichier abîmé, trop gros, mot de passe faux) ou
/// [`SignError::NoKey`] (aucune clé avec son certificat).
pub fn read_pkcs12(data: &[u8], password: &str) -> Result<(Vec<u8>, Vec<Vec<u8>>), SignError> {
    if data.len() > MAX_PKCS12_BYTES {
        return Err(SignError::Unreadable);
    }
    let store = p12_keystore::KeyStore::from_pkcs12(data, password, p12_keystore::Pkcs12ImportPolicy::Strict).map_err(|_| SignError::Unreadable)?;
    let (_, chain) = store.private_key_chain().ok_or(SignError::NoKey)?;
    let certificates: Vec<Vec<u8>> = chain.certs().iter().take(MAX_CHAIN).map(|c| c.as_der().to_vec()).collect();
    if certificates.is_empty() {
        return Err(SignError::NoKey);
    }
    Ok((chain.key().as_der().to_vec(), certificates))
}

/// Clé privée d'un signataire.
enum Key {
    Rsa(Box<rsa::PrivateKey>),
    P256(p256::ecdsa::SigningKey),
    P384(p384::ecdsa::SigningKey),
}

impl Key {
    fn from_pkcs8(der: &[u8]) -> Result<Key, SignError> {
        use rsa::pkcs8::DecodePrivateKey as _;
        use rsa::traits::PublicKeyParts as _;
        if let Ok(key) = rsa::PrivateKey::from_pkcs8_der(der) {
            let bits = key.size().saturating_mul(8);
            if !(algorithms::MIN_RSA_BITS..=algorithms::MAX_RSA_BITS).contains(&bits) {
                return Err(SignError::UnsupportedKey);
            }
            return Ok(Key::Rsa(Box::new(key)));
        }
        if let Ok(key) = p256::ecdsa::SigningKey::from_pkcs8_der(der) {
            return Ok(Key::P256(key));
        }
        if let Ok(key) = p384::ecdsa::SigningKey::from_pkcs8_der(der) {
            return Ok(Key::P384(key));
        }
        Err(SignError::UnsupportedKey)
    }

    /// Algorithme de hachage de la signature.
    fn hash(&self) -> Hash {
        match self {
            Key::Rsa(_) | Key::P256(_) => Hash::Sha256,
            Key::P384(_) => Hash::Sha384,
        }
    }

    /// Algorithme de signature, tel que le nomme un `SignerInfo`.
    fn algorithm(&self) -> AlgorithmIdentifierOwned {
        match self {
            // RFC 4055 §5 : paramètres NULL.
            Key::Rsa(_) => AlgorithmIdentifierOwned { oid: SHA256_RSA, parameters: Some(Any::null()) },
            Key::P256(_) => AlgorithmIdentifierOwned { oid: SHA256_ECDSA, parameters: None },
            Key::P384(_) => AlgorithmIdentifierOwned { oid: SHA384_ECDSA, parameters: None },
        }
    }

    /// Signature de `data` (RSA PKCS #1 v1.5, ou ECDSA en DER).
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, SignError> {
        use p256::ecdsa::signature::Signer as _;
        match self {
            Key::Rsa(key) => {
                let digest = Hash::Sha256.digest(data);
                key.sign(rsa::pkcs1v15::Pkcs1v15Sign::new::<sha2::Sha256>(), &digest).map_err(|_| SignError::Failed)
            }
            Key::P256(key) => {
                let signature: p256::ecdsa::Signature = key.try_sign(data).map_err(|_| SignError::Failed)?;
                Ok(signature.to_der().as_bytes().to_vec())
            }
            Key::P384(key) => {
                let signature: p384::ecdsa::Signature = key.try_sign(data).map_err(|_| SignError::Failed)?;
                Ok(signature.to_der().as_bytes().to_vec())
            }
        }
    }

    /// La clé publique de `cert` est-elle celle de cette clé ? Vérifié en signant.
    fn matches(&self, cert: &Cert) -> bool {
        const PROBE: &[u8] = b"Vortex Mail : cette cle est-elle celle du certificat ?";
        self.sign(PROBE).is_ok_and(|signature| algorithms::verify(cert.public_key(), &self.algorithm(), Some(self.hash()), PROBE, &signature).is_ok())
    }
}

/// Identité d'un signataire : sa clé privée et son certificat, suivi des autorités qui
/// l'accompagnent.
pub struct Identity {
    key: Key,
    signer: Cert,
    chain: Vec<Vec<u8>>,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity").field("key", &"<clé privée>").field("signer", &self.signer.fingerprint()).finish()
    }
}

impl Identity {
    /// Identité faite d'une clé en PKCS #8 (DER) et de certificats en DER, dans n'importe quel
    /// ordre : le certificat de la clé est cherché parmi eux. Les autres sont gardés à sa suite
    /// (au plus [`MAX_CHAIN`] en tout), sauf ceux qui ne sont pas des autorités.
    ///
    /// # Errors
    ///
    /// [`SignError::UnsupportedKey`], ou [`SignError::NoKey`] quand aucun certificat n'est
    /// celui de la clé.
    pub fn new(pkcs8: &[u8], certificates: &[Vec<u8>]) -> Result<Identity, SignError> {
        let key = Key::from_pkcs8(pkcs8)?;
        let certs: Vec<Cert> = certificates.iter().take(MAX_CHAIN).filter_map(|der| Cert::from_der(der)).collect();
        let (mine, others): (Vec<Cert>, Vec<Cert>) = certs.into_iter().partition(|cert| key.matches(cert));
        let signer = mine.into_iter().next().ok_or(SignError::NoKey)?;
        let mut chain = vec![signer.der().to_vec()];
        chain.extend(others.iter().filter(|cert| cert.is_ca() && **cert != signer).map(|cert| cert.der().to_vec()));
        Ok(Identity { key, signer, chain })
    }

    /// Le signataire tel que son certificat le décrit (nom, adresses, émetteur, validité,
    /// empreinte).
    pub fn signer(&self) -> Signer {
        crate::verify::signer_of(&self.signer)
    }

    /// Les certificats à garder avec la clé, le signataire d'abord (DER).
    pub fn certificates(&self) -> &[Vec<u8>] {
        &self.chain
    }

    /// Le contenu d'un message chiffré pour ce certificat (`EnvelopedData`, lot D3a) : l'entité
    /// MIME qu'il porte : clé RSA, ou clé P-256 ou P-384 par ECDH (lot G2b).
    ///
    /// # Errors
    ///
    /// Voir [`DecryptError`](crate::DecryptError).
    pub fn decrypt(&self, cms: &[u8]) -> Result<Vec<u8>, crate::DecryptError> {
        let key = match &self.key {
            Key::Rsa(key) => crate::decrypt::PrivateKey::Rsa(key.as_ref()),
            Key::P256(key) => crate::decrypt::PrivateKey::P256(key.as_nonzero_scalar()),
            Key::P384(key) => crate::decrypt::PrivateKey::P384(key.as_nonzero_scalar()),
        };
        crate::decrypt::decrypt(&key, &self.signer, cms)
    }

    /// Le certificat est-il pour l'adresse `address` (sans tenir compte de la casse) ?
    pub fn is_for(&self, address: &str) -> bool {
        self.signer.is_for(address)
    }

    /// Le nom de l'algorithme de hachage pour le paramètre `micalg` de `multipart/signed`
    /// (RFC 8551 §3.5.3.2).
    pub fn micalg(&self) -> &'static str {
        match self.key.hash() {
            Hash::Sha384 => "sha-384",
            _ => "sha-256",
        }
    }

    /// Le certificat peut-il signer du courrier à l'instant `now` ?
    ///
    /// # Errors
    ///
    /// [`SignError::NotForEmail`] ou [`SignError::Expired`].
    pub fn usable_at(&self, now: Timestamp) -> Result<(), SignError> {
        if !self.signer.for_email() || self.signer.is_ca() {
            return Err(SignError::NotForEmail);
        }
        if !self.signer.valid_at(now) {
            return Err(SignError::Expired);
        }
        Ok(())
    }

    /// Signature CMS détachée de `content` (DER), datée de `now`. Elle est relue avant d'être
    /// rendue : une signature qui ne se vérifierait pas n'est jamais envoyée.
    ///
    /// # Errors
    ///
    /// Celles de [`Identity::usable_at`], ou [`SignError::Failed`].
    pub fn sign_detached(&self, content: &[u8], now: Timestamp) -> Result<Vec<u8>, SignError> {
        self.usable_at(now)?;
        let cms = self.signed_data(content, now).ok_or(SignError::Failed)?;
        let context = Context { anchors: &Anchors::default(), from: "", now, known: &[], coverage: Coverage::Whole };
        let checked = crate::check(Signed::Detached { content, signature: &cms }, &context).ok_or(SignError::Failed)?;
        let fingerprint = checked.signature.signer.as_ref().map(|s| s.fingerprint.clone());
        match checked.signature.verdict {
            Verdict::Changed | Verdict::Unchecked => Err(SignError::Failed),
            _ if fingerprint.as_deref() != Some(self.signer.fingerprint().as_str()) => Err(SignError::Failed),
            _ => Ok(cms),
        }
    }

    fn signed_data(&self, content: &[u8], now: Timestamp) -> Option<Vec<u8>> {
        let hash = self.key.hash();
        let digest_oid = if hash == Hash::Sha384 { SHA384 } else { SHA256 };
        let seconds = u64::try_from(now).ok()?;
        let time = Time::UtcTime(UtcTime::from_unix_duration(std::time::Duration::from_secs(seconds)).ok()?);
        let attribute =
            |oid: ObjectIdentifier, value: Any| -> Option<Attribute> { Some(Attribute { oid, values: SetOfVec::try_from(vec![value]).ok()? }) };
        let attributes: SetOfVec<Attribute> = SetOfVec::try_from(vec![
            attribute(CONTENT_TYPE, Any::encode_from(&DATA).ok()?)?,
            attribute(SIGNING_TIME, Any::encode_from(&time).ok()?)?,
            attribute(MESSAGE_DIGEST, Any::encode_from(&OctetString::new(hash.digest(content)).ok()?).ok()?)?,
        ])
        .ok()?;
        // Ce qui est signé : les attributs en DER, sous leur étiquette SET OF (RFC 5652 §5.4).
        let signature = self.key.sign(&attributes.to_der().ok()?).ok()?;
        let certificate = Certificate::from_der(self.signer.der()).ok()?;
        let info = SignerInfo {
            version: CmsVersion::V1,
            sid: SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
                issuer: certificate.tbs_certificate().issuer().clone(),
                serial_number: certificate.tbs_certificate().serial_number().clone(),
            }),
            digest_alg: AlgorithmIdentifierOwned { oid: digest_oid, parameters: None },
            signed_attrs: Some(attributes),
            signature_algorithm: self.key.algorithm(),
            signature: OctetString::new(signature).ok()?,
            unsigned_attrs: None,
        };
        let certificates: Vec<CertificateChoices> =
            self.chain.iter().filter_map(|der| Certificate::from_der(der).ok()).map(CertificateChoices::Certificate).collect();
        let data = SignedData {
            version: CmsVersion::V1,
            digest_algorithms: SetOfVec::try_from(vec![AlgorithmIdentifierOwned { oid: digest_oid, parameters: None }]).ok()?,
            encap_content_info: EncapsulatedContentInfo { econtent_type: DATA, econtent: None },
            certificates: Some(CertificateSet::try_from(certificates).ok()?),
            crls: None,
            signer_infos: SignerInfos::try_from(vec![info]).ok()?,
        };
        ContentInfo { content_type: SIGNED_DATA, content: Any::encode_from(&data).ok()? }.to_der().ok()
    }
}
