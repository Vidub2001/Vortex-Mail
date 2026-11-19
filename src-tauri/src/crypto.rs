//! Chiffrement des fichiers de sauvegarde, sans dépendance :
//!
//! - clé dérivée du mot de passe par PBKDF2-HMAC-SHA256 (sel aléatoire) ;
//! - chiffrement `ChaCha20` (RFC 8439) ;
//! - authentification HMAC-SHA256 du tout (chiffrer puis authentifier) :
//!   un mauvais mot de passe ou un fichier modifié est refusé avant tout
//!   déchiffrement.
//!
//! Chaque fonction est vérifiée par les vecteurs de test officiels.

use crate::json::{self, Json};
use mail_core::compose::{base64, base64_decode};

// ================================================================ SHA-256 (avec état)

const K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];
const H0: [u32; 8] = [0x6a09_e667, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a, 0x510e_527f, 0x9b05_688c, 0x1f83_d9ab, 0x5be0_cd19];

#[expect(clippy::many_single_char_names, reason = "noms de la spécification (FIPS 180-4), pour la relire ligne à ligne")]
fn compress(state: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for (i, word) in block.chunks(4).enumerate().take(16) {
        w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *s = s.wrapping_add(v);
    }
}

/// SHA-256 incrémental (permet de précalculer les états HMAC de PBKDF2).
/// Tampon fixe : copier un état (deux fois par tour de PBKDF2, 600 000 tours)
/// n'alloue rien (audit P7).
#[derive(Clone)]
struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    filled: usize,
    length: u64,
}

impl Sha256 {
    fn new() -> Self {
        Sha256 { state: H0, buffer: [0; 64], filled: 0, length: 0 }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.length += u64::try_from(data.len()).expect("une longueur en octets tient sur 64 bits");
        if self.filled > 0 {
            let take = (64 - self.filled).min(data.len());
            self.buffer[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled < 64 {
                // Bloc toujours incomplet : toute la saisie y est entrée.
                return;
            }
            let block = self.buffer;
            compress(&mut self.state, &block);
            self.filled = 0;
        }
        while data.len() >= 64 {
            compress(&mut self.state, &data[..64]);
            data = &data[64..];
        }
        self.buffer[..data.len()].copy_from_slice(data);
        self.filled = data.len();
    }

    fn finish(mut self) -> [u8; 32] {
        let bits = self.length * 8;
        // Bourrage : 0x80, des zéros, puis la longueur en bits ; un ou deux blocs.
        let mut tail = [0u8; 128];
        tail[..self.filled].copy_from_slice(&self.buffer[..self.filled]);
        tail[self.filled] = 0x80;
        let total = if self.filled < 56 { 64 } else { 128 };
        tail[total - 8..total].copy_from_slice(&bits.to_be_bytes());
        for block in tail[..total].chunks(64) {
            compress(&mut self.state, block);
        }
        let mut out = [0u8; 32];
        for (i, v) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }
}

pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finish()
}

// ================================================================ HMAC et PBKDF2

/// HMAC-SHA256 avec états intérieur et extérieur précalculés.
#[derive(Clone)]
struct Hmac {
    inner: Sha256,
    outer: Sha256,
}

impl Hmac {
    fn new(key: &[u8]) -> Self {
        let mut k = [0u8; 64];
        if key.len() > 64 {
            k[..32].copy_from_slice(&sha256(key));
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let mut inner = Sha256::new();
        inner.update(&k.map(|b| b ^ 0x36));
        let mut outer = Sha256::new();
        outer.update(&k.map(|b| b ^ 0x5c));
        Hmac { inner, outer }
    }

    fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut inner = self.inner.clone();
        for p in parts {
            inner.update(p);
        }
        let mut outer = self.outer.clone();
        outer.update(&inner.finish());
        outer.finish()
    }
}

#[cfg(test)]
pub(crate) fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    Hmac::new(key).mac(&[data])
}

pub(crate) fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, length: usize) -> Vec<u8> {
    let prf = Hmac::new(password);
    let mut out = Vec::with_capacity(length);
    let mut block = 1u32;
    while out.len() < length {
        let mut u = prf.mac(&[salt, &block.to_be_bytes()]);
        let mut t = u;
        for _ in 1..iterations {
            u = prf.mac(&[&u]);
            for (x, y) in t.iter_mut().zip(u) {
                *x ^= y;
            }
        }
        out.extend_from_slice(&t);
        block += 1;
    }
    out.truncate(length);
    out
}

// ================================================================ ChaCha20 (RFC 8439)

#[expect(clippy::many_single_char_names, reason = "noms de la spécification (RFC 8439), pour la relire ligne à ligne")]
fn quarter(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

fn chacha_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let word = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let mut init = [0u32; 16];
    init[..4].copy_from_slice(&[0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]);
    for i in 0..8 {
        init[4 + i] = word(&key[i * 4..]);
    }
    init[12] = counter;
    for i in 0..3 {
        init[13 + i] = word(&nonce[i * 4..]);
    }
    let mut s = init;
    for _ in 0..10 {
        quarter(&mut s, 0, 4, 8, 12);
        quarter(&mut s, 1, 5, 9, 13);
        quarter(&mut s, 2, 6, 10, 14);
        quarter(&mut s, 3, 7, 11, 15);
        quarter(&mut s, 0, 5, 10, 15);
        quarter(&mut s, 1, 6, 11, 12);
        quarter(&mut s, 2, 7, 8, 13);
        quarter(&mut s, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        out[i * 4..i * 4 + 4].copy_from_slice(&s[i].wrapping_add(init[i]).to_le_bytes());
    }
    out
}

/// Chiffre ou déchiffre (même opération) à partir du compteur `counter`.
///
/// # Panics
///
/// Au-delà de 256 Gio de données : le compteur de blocs de `ChaCha20` n'a que
/// 32 bits, et le réutiliser exposerait le texte chiffré.
pub(crate) fn chacha20(key: &[u8; 32], nonce: &[u8; 12], counter: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for (i, chunk) in data.chunks(64).enumerate() {
        let block = u32::try_from(i).expect("plus de 256 Gio à chiffrer d'un coup");
        let stream = chacha_block(key, counter.wrapping_add(block), nonce);
        out.extend(chunk.iter().zip(stream).map(|(a, b)| a ^ b));
    }
    out
}

// ================================================================ fichier de sauvegarde

const FORMAT: &str = "vortexmail-backup";
const VERSION: i64 = 1;
/// Ralentit les essais de mots de passe ; enregistré dans le fichier. 600 000
/// pour PBKDF2-HMAC-SHA256, comme le recommande l'OWASP (200 000 avant le
/// lot F5 : ces fichiers restent lisibles, et le coffre est réécrit au
/// déverrouillage ; relecture complète n° 26).
pub(crate) const ITERATIONS: u32 = 600_000;

/// Nombre d'itérations d'un fichier chiffré, s'il est lisible.
pub(crate) fn sealed_iterations(file: &str) -> Option<u32> {
    envelope(file).ok().map(|e| e.iterations)
}

/// Égalité en temps constant (le MAC ne se devine pas octet par octet).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn keys(password: &str, salt: &[u8], iterations: u32) -> ([u8; 32], [u8; 32]) {
    let derived = pbkdf2_sha256(password.as_bytes(), salt, iterations, 64);
    let mut enc = [0u8; 32];
    let mut mac = [0u8; 32];
    enc.copy_from_slice(&derived[..32]);
    mac.copy_from_slice(&derived[32..]);
    (enc, mac)
}

fn authenticate(mac_key: &[u8; 32], iterations: u32, salt: &[u8], nonce: &[u8], data: &[u8]) -> [u8; 32] {
    let header = format!("{FORMAT}|{VERSION}|{iterations}|");
    Hmac::new(mac_key).mac(&[header.as_bytes(), salt, nonce, data])
}

/// Chiffre `plain` avec le mot de passe ; renvoie le contenu du fichier (JSON).
pub(crate) fn seal(password: &str, plain: &[u8]) -> Result<String, String> {
    seal_with(password, plain, ITERATIONS)
}

/// Clés tirées d'un mot de passe une fois pour toutes (coffre du mode
/// portable, audit P7 : 600 000 tours de PBKDF2 à chaque secret enregistré).
/// Chaque fichier écrit garde un nonce aléatoire neuf.
pub(crate) struct SealKey {
    iterations: u32,
    salt: [u8; 16],
    enc: [u8; 32],
    mac: [u8; 32],
}

impl SealKey {
    /// Nouveau sel, clés tirées de `password`.
    ///
    /// # Errors
    ///
    /// Mot de passe trop court, ou générateur aléatoire du système indisponible.
    pub(crate) fn derive(password: &str) -> Result<Self, String> {
        Self::derive_with(password, ITERATIONS)
    }

    fn derive_with(password: &str, iterations: u32) -> Result<Self, String> {
        if password.chars().count() < 8 {
            return Err(tr!("Mot de passe trop court : 8 caractères au moins.").into());
        }
        let salt = crate::oauth::random_bytes::<16>()?;
        let (enc, mac) = keys(password, &salt, iterations);
        Ok(SealKey { iterations, salt, enc, mac })
    }

    /// Comme [`seal`], sans refaire le calcul de clé.
    ///
    /// # Errors
    ///
    /// Générateur aléatoire du système indisponible.
    pub(crate) fn seal(&self, plain: &[u8]) -> Result<String, String> {
        let nonce = crate::oauth::random_bytes::<12>()?;
        Ok(sealed_file(self.iterations, &self.salt, &nonce, &self.enc, &self.mac, plain))
    }
}

pub(crate) fn seal_with(password: &str, plain: &[u8], iterations: u32) -> Result<String, String> {
    if password.chars().count() < 8 {
        return Err(tr!("Mot de passe trop court : 8 caractères au moins.").into());
    }
    let salt = crate::oauth::random_bytes::<16>()?;
    let nonce = crate::oauth::random_bytes::<12>()?;
    let (enc, mac_key) = keys(password, &salt, iterations);
    Ok(sealed_file(iterations, &salt, &nonce, &enc, &mac_key, plain))
}

/// Contenu du fichier chiffré (JSON), clés déjà tirées.
fn sealed_file(iterations: u32, salt: &[u8], nonce: &[u8; 12], enc: &[u8; 32], mac_key: &[u8; 32], plain: &[u8]) -> String {
    let data = chacha20(enc, nonce, 1, plain);
    let mac = authenticate(mac_key, iterations, salt, nonce, &data);
    Json::obj([
        ("format", FORMAT.into()),
        ("version", VERSION.into()),
        ("kdf", "pbkdf2-sha256".into()),
        ("iterations", i64::from(iterations).into()),
        ("cipher", "chacha20+hmac-sha256".into()),
        ("salt", base64(salt).into()),
        ("nonce", base64(nonce).into()),
        ("data", base64(&data).into()),
        ("mac", base64(&mac).into()),
    ])
    .to_text()
}

/// Pourquoi un fichier chiffré ne s'ouvre pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenError {
    /// Pas un fichier chiffré par Vortex Mail, ou fichier abîmé dans sa structure.
    NotSealed,
    /// Format d'une version plus récente de Vortex Mail.
    NewerVersion,
    /// Contrôle d'intégrité en échec : autre mot de passe, ou données modifiées.
    WrongPassword,
}

impl OpenError {
    #[must_use]
    pub(crate) fn message(self) -> String {
        match self {
            OpenError::NotSealed => tr!("Ce fichier n'est pas une sauvegarde de Vortex Mail.").to_string(),
            OpenError::NewerVersion => tr!("Sauvegarde créée par une version plus récente de Vortex Mail : mettez-la à jour.").to_string(),
            OpenError::WrongPassword => tr!("Mot de passe incorrect, ou fichier endommagé.").to_string(),
        }
    }
}

/// Vérifie puis déchiffre un fichier de sauvegarde.
pub(crate) fn open(password: &str, file: &str) -> Result<Vec<u8>, String> {
    open_sealed(password, file).map_err(OpenError::message)
}

/// Comme `open`, avec la raison d'un échec (coffre du mode portable : un
/// fichier abîmé n'est pas un mot de passe incorrect, relecture C3).
///
/// # Errors
///
/// Voir [`OpenError`].
pub(crate) fn open_sealed(password: &str, file: &str) -> Result<Vec<u8>, OpenError> {
    let e = envelope(file)?;
    let (enc, mac_key) = keys(password, &e.salt, e.iterations);
    if !same(&authenticate(&mac_key, e.iterations, &e.salt, &e.nonce, &e.data), &e.mac) {
        return Err(OpenError::WrongPassword);
    }
    Ok(chacha20(&enc, &e.nonce, 1, &e.data))
}

/// Enveloppe d'un fichier chiffré, lue sans aucun calcul de clé.
pub(crate) struct Envelope {
    iterations: u32,
    salt: Vec<u8>,
    nonce: [u8; 12],
    data: Vec<u8>,
    mac: Vec<u8>,
}

/// Lit l'enveloppe JSON d'une sauvegarde ou du coffre (format, version,
/// itérations bornées, champs en base64), avant tout calcul de clé.
///
/// # Errors
///
/// Voir [`OpenError`] : pas une sauvegarde, ou version plus récente.
pub(crate) fn envelope(file: &str) -> Result<Envelope, OpenError> {
    let not_backup = || OpenError::NotSealed;
    let v = json::parse(file.trim()).map_err(|_| not_backup())?;
    if v.get("format").text() != FORMAT {
        return Err(not_backup());
    }
    let version = v.get("version").as_i64().unwrap_or(0);
    if version != VERSION {
        return Err(OpenError::NewerVersion);
    }
    let iterations =
        v.get("iterations").as_i64().and_then(|n| u32::try_from(n).ok()).filter(|n| (1..=10_000_000).contains(n)).ok_or_else(not_backup)?;
    let field = |name: &str| base64_decode(v.get(name).text()).map_err(|_| not_backup());
    let (salt, nonce, data, mac) = (field("salt")?, field("nonce")?, field("data")?, field("mac")?);
    let nonce: [u8; 12] = nonce.try_into().map_err(|_| not_backup())?;
    Ok(Envelope { iterations, salt, nonce, data, mac })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    #[test]
    fn p7_cle_du_coffre_tiree_une_fois() {
        // 1 000 tours : le même calcul que 600 000, en un instant pour le test.
        let key = SealKey::derive_with("mot de passe maître", 1000).unwrap();
        let (a, b) = (key.seal(b"secret 1").unwrap(), key.seal(b"secret 2").unwrap());
        assert_eq!(open("mot de passe maître", &a).unwrap(), b"secret 1", "lisible comme un fichier ordinaire");
        assert_eq!(open("mot de passe maître", &b).unwrap(), b"secret 2");
        assert_ne!(envelope(&a).unwrap().nonce, envelope(&b).unwrap().nonce, "un nonce neuf à chaque écriture");
        assert_eq!(sealed_iterations(&a), Some(1000));
        assert!(open("autre mot de passe", &a).is_err());
        assert!(SealKey::derive("court").is_err(), "8 caractères au moins");
    }

    #[test]
    fn sha256_vectors() {
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        // Saisie en plusieurs morceaux, à cheval sur les blocs.
        let long = vec![b'a'; 1000];
        let mut h = Sha256::new();
        for part in long.chunks(37) {
            h.update(part);
        }
        assert_eq!(h.finish(), sha256(&long));
        // Vecteur du NIST (un million de « a »), saisi par morceaux de 7 octets :
        // blocs incomplets complétés au fil des appels (tampon fixe, audit P7).
        let million = vec![b'a'; 1_000_000];
        let mut h = Sha256::new();
        for part in million.chunks(7) {
            h.update(part);
        }
        assert_eq!(hex(&h.finish()), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(hex(&sha256(long)), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    }

    #[test]
    fn hmac_and_pbkdf2_vectors() {
        assert_eq!(
            hex(&hmac_sha256(b"key", b"The quick brown fox jumps over the lazy dog")),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
        // RFC 7914, section 11.
        assert_eq!(
            hex(&pbkdf2_sha256(b"passwd", b"salt", 1, 64)),
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc\
             49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783"
        );
        assert_eq!(
            hex(&pbkdf2_sha256(b"Password", b"NaCl", 80000, 64)),
            "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56\
             a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d"
        );
    }

    #[test]
    fn chacha20_rfc8439() {
        let key: [u8; 32] = std::array::from_fn(|i| u8::try_from(i).unwrap());
        let nonce = [0, 0, 0, 0, 0, 0, 0, 0x4a, 0, 0, 0, 0];
        let plain = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let cipher = chacha20(&key, &nonce, 1, plain);
        assert_eq!(
            hex(&cipher),
            "6e2e359a2568f98041ba0728dd0d6981e97e7aec1d4360c20a27afccfd9fae0bf91b65c5524733ab8f593dabcd62b357\
             1639d624e65152ab8f530c359f0861d807ca0dbf500d6a6156a38e088a22b65e52bc514d16ccf806818ce91ab7793736\
             5af90bbf74a35be6b40b8eedf2785e42874d"
        );
        assert_eq!(chacha20(&key, &nonce, 1, &cipher), plain);
    }

    #[test]
    fn backup_file_round_trip() {
        let secret = "comptes, mots de passe et réglages".repeat(20);
        let file = seal_with("correct horse battery", secret.as_bytes(), 1000).unwrap();
        assert!(file.contains("\"format\":\"vortexmail-backup\""));
        assert!(!file.contains("réglages"));
        assert_eq!(open("correct horse battery", &file).unwrap(), secret.as_bytes());
        assert_eq!(open("mauvais mot de passe", &file).unwrap_err(), "Mot de passe incorrect, ou fichier endommagé.");

        // Une seule lettre modifiée dans les données : refusé.
        let mut v = json::parse(&file).unwrap();
        let data = v.get("data").text().to_string();
        let flipped = match data.strip_prefix('A') {
            Some(rest) => format!("B{rest}"),
            None => format!("A{}", &data[1..]),
        };
        v.set("data", flipped.into());
        assert!(open("correct horse battery", &v.to_text()).is_err());

        assert!(open("x", "pas du json").unwrap_err().contains("n'est pas une sauvegarde"));
        assert!(seal("court", b"x").is_err());
        // Deux sauvegardes du même contenu diffèrent (sel et nonce aléatoires).
        assert_ne!(seal_with("correct horse battery", b"x", 10).unwrap(), seal_with("correct horse battery", b"x", 10).unwrap());
    }
}
