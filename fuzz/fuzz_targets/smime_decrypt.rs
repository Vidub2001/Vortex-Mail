//! Message chiffré reçu (lot G2) : reconnu (`is_enveloped`), remis en DER, puis
//! déchiffré avec les clés de test de `crates/smime/testdata` comme à
//! l'ouverture : Bob (RSA) et Carol (P-256, ECDH ; lot G2b).
#![no_main]

use std::sync::OnceLock;

use vortex_smime::{ber, is_enveloped, Identity};

fn identities() -> &'static [Identity] {
    static KEYS: OnceLock<Vec<Identity>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let bob = Identity::new(
            include_bytes!("../../crates/smime/testdata/bob.key.der"),
            &[include_bytes!("../../crates/smime/testdata/bob.cert.der").to_vec()],
        );
        let carol = Identity::new(
            include_bytes!("../../crates/smime/testdata/carol.key.der"),
            &[include_bytes!("../../crates/smime/testdata/carol.cert.der").to_vec()],
        );
        [bob, carol].into_iter().filter_map(Result::ok).collect()
    })
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = is_enveloped(data);
    let _ = ber::to_der(data);
    let _ = ber::signed_content(data);
    for identity in identities() {
        let _ = identity.decrypt(data);
    }
});
