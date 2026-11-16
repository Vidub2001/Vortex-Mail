//! Une réponse IMAP complète (ligne et littéraux), lue d'un coup.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = imap_lite::parse_response(data);
});
