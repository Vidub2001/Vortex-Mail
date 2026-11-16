//! Message signé reçu (lot G2) : ses deux moitiés lues dans le message, puis la
//! signature CMS vérifiée comme à l'affichage (`smime::check`), avec une
//! autorité de confiance pour parcourir aussi les chaînes de certificats.
#![no_main]

use std::sync::OnceLock;

use vortex_smime::{check, signed_parts, Anchors, Context, Coverage, Signed, SignedKind};

fn anchors() -> &'static Anchors {
    static ANCHORS: OnceLock<Anchors> = OnceLock::new();
    ANCHORS.get_or_init(|| Anchors::new([include_bytes!("../../crates/smime/testdata/ca.rsa.der").as_slice()]))
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Some(parts) = signed_parts(data) else { return };
    let signed = match parts.kind {
        SignedKind::SmimeDetached => Signed::Detached { content: &parts.content, signature: &parts.signature },
        SignedKind::SmimeOpaque => Signed::Opaque(&parts.signature),
    };
    let context = Context {
        anchors: anchors(),
        from: &parts.from,
        now: 1_800_000_000,
        known: &[],
        coverage: if parts.whole { Coverage::Whole } else { Coverage::Part },
    };
    let _ = check(signed, &context);
});
