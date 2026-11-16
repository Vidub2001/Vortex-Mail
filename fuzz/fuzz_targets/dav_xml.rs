//! Une réponse XML d'un serveur WebDAV (CalDAV, CardDAV).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    vortexmail_lib::fuzz_api::dav_xml(&String::from_utf8_lossy(data));
});
