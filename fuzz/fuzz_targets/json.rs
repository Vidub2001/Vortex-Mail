//! Une réponse JSON d'Outlook.com ou de Google.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    vortexmail_lib::fuzz_api::json(&String::from_utf8_lossy(data));
});
