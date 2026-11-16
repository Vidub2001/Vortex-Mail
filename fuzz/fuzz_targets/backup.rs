//! Une sauvegarde ou le coffre portable : enveloppe chiffrée, puis contenu
//! déchiffré (`crypto.rs`, `backup.rs`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    vortexmail_lib::fuzz_api::backup(&String::from_utf8_lossy(data));
});
