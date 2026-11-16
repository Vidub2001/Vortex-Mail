//! Certificat reçu dans un message signé ou gardé pour un correspondant
//! (lot G2) : résumé affiché, signataire, possibilité de lui écrire chiffré.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = vortex_smime::certificate_summary(data);
    let _ = vortex_smime::certificate_signer(data);
    let _ = vortex_smime::can_encrypt_to(data);
});
