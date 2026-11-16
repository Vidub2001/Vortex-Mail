//! Les réponses d'un serveur SMTP, lues l'une après l'autre (`transport.rs`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    vortexmail_lib::fuzz_api::smtp(data);
});
