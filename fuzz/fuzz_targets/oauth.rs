//! Le retour de la page de connexion `OAuth2` et le jeton d'identité (`oauth.rs`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    vortexmail_lib::fuzz_api::oauth(&String::from_utf8_lossy(data));
});
