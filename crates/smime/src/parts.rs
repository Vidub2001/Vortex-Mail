// SPDX-FileCopyrightText: MailCraft contributors, Vortex Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The halves of a Digitally signed message, for its Digital signature to be checked
//! ([`crate::check`]): of `multipart/signed` (RFC 1847 §2.1), the signed part exactly as it
//! came, MIME header and all, and the signature part's bytes; of S/MIME's
//! `application/pkcs7-mime` (RFC 8551 §3.2), the CMS it carries. Read from the bytes the message
//! came as; nothing here checks anything.
//!
//! Adapté de `mailcraft-mime` (`crates/mime/src/signed.rs`) pour Vortex Mail : la lecture des
//! en-têtes, des paramètres et des parties est écrite ici, sans le reste du lecteur MIME de
//! MailCraft. L'entrée vient de l'extérieur : rien ici ne panique, et le travail est linéaire.

/// How deep into the first part of nested multiparts a signed message is looked for.
const MAX_DEPTH: usize = 4;
/// The most header lines read from one header (a hostile message may send thousands).
const MAX_HEADER_LINES: usize = 1_000;

/// Which kind of Digital signature a message carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignedKind {
    /// `multipart/signed` with `application/pkcs7-signature`: S/MIME, detached.
    SmimeDetached,
    /// `application/pkcs7-mime`: S/MIME's CMS in one part (signed data, or enveloped data,
    /// which the CMS itself tells).
    SmimeOpaque,
}

/// The halves of a Digitally signed message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedParts {
    pub kind: SignedKind,
    /// The signed part as it came, header and body, without the line end before the next
    /// boundary (RFC 1847 §2.1). Empty for [`SignedKind::SmimeOpaque`], whose CMS holds it.
    pub content: Vec<u8>,
    /// The signature part's bytes, its transfer encoding undone (the CMS); for
    /// [`SignedKind::SmimeOpaque`], the CMS.
    pub signature: Vec<u8>,
    /// The signed part is the whole message body (it is the message's top level): `false` when
    /// it is only the first part of a wrapper (a list's footer added after it, say), whose other
    /// parts nobody signed.
    pub whole: bool,
    /// The message's header names exactly one From mailbox (one field, one address): a
    /// signature can be for its sender only then.
    pub one_from: bool,
    /// That one From address, empty otherwise.
    pub from: String,
}

/// The halves of the message `bytes` when it is Digitally signed with S/MIME (or, for
/// [`SignedKind::SmimeOpaque`], may be): at its top level, or the first part of a multipart
/// wrapping it, as deep as [`MAX_DEPTH`] ([`SignedParts::whole`] says which), never inside a
/// `multipart/alternative` (whose other versions are shown instead as often). `None` for any other
/// message. Never fails.
pub fn signed_parts(bytes: &[u8]) -> Option<SignedParts> {
    let (headers, body) = split_header(bytes);
    let mut parts = of_part(&headers, body, 0)?;
    let froms: Vec<&str> = headers.iter().filter(|(n, _)| n == "from").map(|(_, v)| v.as_str()).collect();
    if let [one] = froms.as_slice() {
        let list = mail_core::compose::parse_address_list(one);
        if let [address] = list.as_slice() {
            parts.one_from = true;
            parts.from.clone_from(address);
        }
    }
    Some(parts)
}

/// Header fields (names in lower case, values unfolded) and the body after the blank line.
type Headers = Vec<(String, String)>;

fn get<'h>(headers: &'h Headers, name: &str) -> Option<&'h str> {
    headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

/// A header and its body: the header ends at the first empty line (CRLF or LF).
fn split_header(bytes: &[u8]) -> (Headers, &[u8]) {
    let mut headers: Headers = Vec::new();
    let mut pos = 0;
    while let Some(rest) = bytes.get(pos..).filter(|r| !r.is_empty()) {
        let end = rest.iter().position(|&b| b == b'\n').map_or(bytes.len(), |i| pos + i + 1);
        let line = bytes.get(pos..end).unwrap_or_default();
        let text = line.strip_suffix(b"\n").unwrap_or(line);
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        pos = end;
        if text.is_empty() {
            return (headers, bytes.get(pos..).unwrap_or_default());
        }
        let text = String::from_utf8_lossy(text);
        if text.starts_with([' ', '\t']) {
            if let Some((_, value)) = headers.last_mut() {
                value.push(' ');
                value.push_str(text.trim());
            }
            continue;
        }
        if headers.len() >= MAX_HEADER_LINES {
            continue;
        }
        if let Some((name, value)) = text.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    (headers, &[])
}

/// A header value with parameters: `type/subtype; name="value"; …`.
#[derive(Debug, Default)]
struct Params {
    value: String,
    params: Vec<(String, String)>,
}

impl Params {
    fn get(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// The value (lower case) and parameters (names in lower case, values unquoted) of a header.
fn parse_params(text: &str) -> Params {
    let mut pieces = Vec::new();
    let (mut current, mut quoted, mut escaped) = (String::new(), false, false);
    for c in text.chars() {
        if escaped {
            current.push(c);
            escaped = false;
        } else if quoted && c == '\\' {
            escaped = true;
        } else if c == '"' {
            quoted = !quoted;
        } else if c == ';' && !quoted {
            pieces.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    pieces.push(current);
    let mut pieces = pieces.into_iter();
    let value = pieces.next().unwrap_or_default().trim().to_ascii_lowercase();
    let params = pieces
        .filter_map(|p| {
            let (name, value) = p.split_once('=')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect();
    Params { value, params }
}

/// The parts of a multipart body between its boundaries (RFC 2046 §5.1.1), at most `max`: each
/// as it came, without the line end that belongs to the next boundary.
fn split_multipart<'b>(body: &'b [u8], boundary: &str, max: usize) -> Vec<&'b [u8]> {
    let delimiter = format!("--{boundary}");
    let mut parts = Vec::new();
    // Start of the current part (just after a boundary line), if one is open.
    let mut open: Option<usize> = None;
    let mut pos = 0;
    while let Some(rest) = body.get(pos..).filter(|r| !r.is_empty()) {
        let end = rest.iter().position(|&b| b == b'\n').map_or(body.len(), |i| pos + i + 1);
        let line = body.get(pos..end).unwrap_or_default();
        if let Some(rest) = line.strip_prefix(delimiter.as_bytes()) {
            let rest = rest.trim_ascii();
            let close = rest == b"--";
            if close || rest.is_empty() {
                if let Some(start) = open {
                    // The line end before the boundary belongs to the boundary.
                    let mut part = body.get(start..pos).unwrap_or_default();
                    part = part.strip_suffix(b"\n").unwrap_or(part);
                    part = part.strip_suffix(b"\r").unwrap_or(part);
                    parts.push(part);
                    if parts.len() >= max {
                        return parts;
                    }
                }
                if close {
                    return parts;
                }
                open = Some(end);
            }
        }
        pos = end;
    }
    parts
}

fn of_part(headers: &Headers, body: &[u8], depth: usize) -> Option<SignedParts> {
    let content_type = get(headers, "content-type").map(parse_params).unwrap_or_default();
    let media_type = content_type.value.as_str();
    let disposition = get(headers, "content-disposition").map(parse_params).unwrap_or_default();
    let pkcs7 = matches!(media_type, "application/pkcs7-mime" | "application/x-pkcs7-mime");
    // (A part of such a type sent along as a file is not the message's.)
    if disposition.value == "attachment" && (depth > 0 || !pkcs7) {
        return None;
    }
    match media_type {
        "multipart/signed" => {
            let protocol = content_type.get("protocol").unwrap_or_default().trim().to_ascii_lowercase();
            if !matches!(protocol.as_str(), "application/pkcs7-signature" | "application/x-pkcs7-signature") {
                return None;
            }
            let boundary = content_type.get("boundary").filter(|b| !b.is_empty())?;
            let parts = split_multipart(body, boundary, 3);
            let [content, signature] = parts.as_slice() else { return None };
            let (sig_headers, sig_body) = split_header(signature);
            // The second part is the signature only when it is of the type `protocol` names (RFC
            // 1847 §2.1): any other is a part of the message, shown as one, and nobody's signature.
            let sig_type = get(&sig_headers, "content-type").map(parse_params).unwrap_or_default().value;
            if !matches!(sig_type.as_str(), "application/pkcs7-signature" | "application/x-pkcs7-signature") {
                return None;
            }
            Some(SignedParts {
                kind: SignedKind::SmimeDetached,
                content: content.to_vec(),
                signature: decoded(&sig_headers, sig_body),
                whole: depth == 0,
                one_from: false,
                from: String::new(),
            })
        }
        _ if pkcs7 => {
            // (`smime-type` names what is inside; the CMS says it too, and is what is believed.)
            let decoded = decoded(headers, body);
            (!decoded.is_empty()).then_some(SignedParts {
                kind: SignedKind::SmimeOpaque,
                content: Vec::new(),
                signature: decoded,
                whole: depth == 0,
                one_from: false,
                from: String::new(),
            })
        }
        // (Never into `multipart/alternative`: a signed first version beside an unsigned one
        // would lend the other its signature.)
        multipart if multipart.starts_with("multipart/") && multipart != "multipart/alternative" && depth < MAX_DEPTH => {
            let boundary = content_type.get("boundary").filter(|b| !b.is_empty())?;
            let first = split_multipart(body, boundary, 1);
            let (part_headers, part_body) = split_header(first.first()?);
            of_part(&part_headers, part_body, depth.saturating_add(1))
        }
        _ => None,
    }
}

/// `body` with the transfer encoding its `headers` name undone: base64; anything else as it is
/// (a CMS is always sent in base64, RFC 8551 §3.1.3).
fn decoded(headers: &Headers, body: &[u8]) -> Vec<u8> {
    let encoding = get(headers, "content-transfer-encoding").map(|e| e.trim().to_ascii_lowercase()).unwrap_or_default();
    if encoding == "base64" { mail_core::compose::base64_decode(&String::from_utf8_lossy(body)).unwrap_or_default() } else { body.to_vec() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_halves() {
        let mail = b"From: Alice <alice@example.com>\r\nContent-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"B\"\r\n\r\n\
            preamble\r\n--B\r\nContent-Type: text/plain\r\n\r\nHello\r\n--B\r\nContent-Type: application/pkcs7-signature\r\nContent-Transfer-Encoding: base64\r\n\r\nAAEC\r\n--B--\r\n";
        let parts = signed_parts(mail).expect("signed");
        assert_eq!(parts.kind, SignedKind::SmimeDetached);
        assert_eq!(parts.content, b"Content-Type: text/plain\r\n\r\nHello");
        assert_eq!(parts.signature, vec![0, 1, 2]);
        assert!(parts.whole && parts.one_from);
        assert_eq!(parts.from, "alice@example.com");
    }

    #[test]
    fn other_messages_are_not_signed() {
        assert_eq!(signed_parts(b"Content-Type: text/plain\r\n\r\nHi"), None);
        // Signature part of another type: not a signature.
        let mail = b"Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; boundary=B\n\n--B\nContent-Type: text/plain\n\nx\n--B\nContent-Type: text/plain\n\ny\n--B--\n";
        assert_eq!(signed_parts(mail), None);
        // Inside multipart/alternative: never.
        let alt = b"Content-Type: multipart/alternative; boundary=A\n\n--A\nContent-Type: application/pkcs7-mime\nContent-Transfer-Encoding: base64\n\nAAEC\n--A--\n";
        assert_eq!(signed_parts(alt), None);
        // Two From addresses: no sender for the signature.
        let two = b"From: a@x.fr, b@y.fr\nContent-Type: application/pkcs7-mime\nContent-Transfer-Encoding: base64\n\nAAEC\n";
        let parts = signed_parts(two).expect("opaque");
        assert!(!parts.one_from && parts.from.is_empty());
    }

    #[test]
    fn hostile_input_does_not_panic() {
        for bytes in
            [&b""[..], b"\n", b"Content-Type: multipart/signed; boundary=\"\"\n\n--\n", b"Content-Type: multipart/mixed; boundary=x\n\n--x\n--x--"]
        {
            let _ = signed_parts(bytes);
        }
        assert!(split_multipart(b"--B\r\n--B--", "B", 3).iter().all(|p| p.is_empty()));
    }
}
