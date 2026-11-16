//! Un message reçu, décodé comme à l'affichage : en-têtes, corps, images
//! intégrées, pièces jointes (nom nettoyé, contrôle avant ouverture).
//!
//! Depuis le passage à `mail-parser` 1.0 (lot 9e), toute panique arrête la
//! cible, y compris dans la crate : `mimeparse` la rattraperait dans
//! l'application, mais elle doit être signalée aux auteurs. Les deux mails
//! qui faisaient paniquer les versions 0.9 et 0.11 sont dans
//! `tests/corpus/fuzz/mime/`, rejoués à chaque passage.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    vortexmail_lib::fuzz_api::message(data);
});
