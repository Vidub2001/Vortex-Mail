//! Une date `INTERNALDATE` envoyée par le serveur, et l'écriture d'une date
//! quelconque pour `APPEND`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = imap_lite::parse_internal_date(&String::from_utf8_lossy(data));
    if let Some(bytes) = data.get(..8) {
        let mut ts = [0u8; 8];
        ts.copy_from_slice(bytes);
        let _ = imap_lite::format_internal_date(i64::from_le_bytes(ts));
    }
});
