//! Une carte de visite (vCard) : lecture, réécriture, relecture.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    vortexmail_lib::fuzz_api::vcard(&String::from_utf8_lossy(data));
});
