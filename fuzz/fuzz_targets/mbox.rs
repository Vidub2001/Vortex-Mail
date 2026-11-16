//! Un fichier importé, `.eml` ou `.mbox` : découpé en messages, réécrit, relu (lot E8a).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    vortexmail_lib::fuzz_api::mbox(data);
});
