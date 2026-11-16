//! Noms de dossiers IMAP en « UTF-7 modifié » (RFC 3501 §5.1.3).
//! Ex. `Envoy&AOk-s` ⇄ `Envoyés`.

use crate::base64;

/// Décode un nom de dossier tel qu'envoyé par le serveur. En cas de séquence
/// invalide, le texte brut est conservé tel quel.
#[must_use]
pub fn decode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let encoded = &after[..end];
        if encoded.is_empty() {
            out.push('&');
        } else {
            match decode_utf16(encoded) {
                Some(s) => out.push_str(&s),
                None => out.push_str(&rest[start..start + 2 + end]),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn decode_utf16(encoded: &str) -> Option<String> {
    let bytes = base64::decode_imap(encoded)?;
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    String::from_utf16(&units).ok()
}

/// Encode un nom de dossier pour l'envoyer au serveur.
#[must_use]
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if !pending.is_empty() {
            let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
            out.push('&');
            out.push_str(&base64::encode_imap(&bytes));
            out.push('-');
            pending.clear();
        }
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut pending, &mut out);
            if c == '&' {
                out.push_str("&-");
            } else {
                out.push(c);
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut buf));
        }
    }
    flush(&mut pending, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_names() {
        assert_eq!(decode("Envoy&AOk-s"), "Envoyés");
        assert_eq!(decode("&AMk-l&AOk-ments supprim&AOk-s"), "Éléments supprimés");
        assert_eq!(decode("Tom &- Jerry"), "Tom & Jerry");
        assert_eq!(decode("INBOX"), "INBOX");
        assert_eq!(encode("Envoyés"), "Envoy&AOk-s");
        assert_eq!(encode("Tom & Jerry"), "Tom &- Jerry");
    }

    #[test]
    fn roundtrip_non_latin() {
        for s in ["日本語", "Réunion 📅", "a&b", "Clients/Société Générale"] {
            assert_eq!(decode(&encode(s)), s);
        }
    }

    #[test]
    fn invalid_is_kept() {
        assert_eq!(decode("abc&def"), "abc&def");
    }
}
