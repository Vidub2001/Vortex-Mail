//! Un corps HTML, nettoyé comme à l'affichage (balises, redirections,
//! images distantes).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    vortexmail_lib::fuzz_api::html(&String::from_utf8_lossy(data));
});
