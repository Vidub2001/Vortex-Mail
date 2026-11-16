//! Un fichier iCalendar (invitation reçue, réponse d'un serveur CalDAV) :
//! lecture, réécriture, conversion des répétitions pour Outlook.com.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    vortexmail_lib::fuzz_api::ical(&String::from_utf8_lossy(data));
});
