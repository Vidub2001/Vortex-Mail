//! Base64 standard (RFC 4648) et variante « UTF-7 modifié » d'IMAP (`,` au lieu de `/`, sans `=`).

const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const IMAP: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

fn encode_with(data: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (usize::from(b[0]) << 16) | (usize::from(b[1]) << 8) | usize::from(b[2]);
        let chars = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        let keep = chunk.len() + 1;
        for (i, c) in chars.iter().enumerate() {
            if i < keep {
                out.push(char::from(alphabet[*c]));
            } else if pad {
                out.push('=');
            }
        }
    }
    out
}

fn decode_with(text: &str, alphabet: &[u8; 64]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in text.bytes() {
        if c == b'=' {
            break;
        }
        let v = u32::try_from(alphabet.iter().position(|&a| a == c)?).ok()?;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            let [.., byte] = (acc >> bits).to_be_bytes();
            out.push(byte);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

pub(crate) fn encode(data: &[u8]) -> String {
    encode_with(data, STD, true)
}

pub(crate) fn encode_imap(data: &[u8]) -> String {
    encode_with(data, IMAP, false)
}

pub(crate) fn decode_imap(text: &str) -> Option<Vec<u8>> {
    decode_with(text, IMAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_vectors() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"\0user\0pass"), "AHVzZXIAcGFzcw==");
    }

    #[test]
    fn imap_roundtrip() {
        let data = [0x00, 0xe9, 0xff, 0xfe, 0x10];
        assert_eq!(decode_imap(&encode_imap(&data)).unwrap(), data);
    }
}
