//! Lecture des réponses brutes : lignes CRLF et littéraux `{n}`.

use std::io::{Read, Write};

use crate::limits::{exceeded, Limit, MAX_COMMAND_LITERALS, MAX_COMMAND_TEXT, MAX_LINE, MAX_LITERAL, MAX_LITERALS, MAX_RESPONSES};
use crate::{Error, Result};

/// Capacité du tampon de lecture gardée d'une commande à l'autre (audit P10).
const BUF_KEPT: usize = 64 * 1024;

/// Flux avec tampon de lecture, qui garde l'accès en écriture.
pub(crate) struct Connection<S> {
    stream: S,
    buf: Vec<u8>,
    start: usize,
    /// Ce qui a été reçu depuis le début de la commande en cours (plafonds).
    responses: usize,
    text: usize,
    literal_bytes: usize,
}

impl<S: Read + Write> Connection<S> {
    pub(crate) fn new(stream: S) -> Self {
        Self { stream, buf: Vec::with_capacity(16 * 1024), start: 0, responses: 0, text: 0, literal_bytes: 0 }
    }

    /// Une nouvelle commande commence : ses plafonds repartent de zéro, et le
    /// tampon grossi par une très longue ligne reprend sa taille ordinaire
    /// (audit P10 : sinon chaque connexion du compte la garderait).
    pub(crate) fn begin_command(&mut self) {
        self.responses = 0;
        self.text = 0;
        self.literal_bytes = 0;
        if self.buf.capacity() > BUF_KEPT {
            self.buf.drain(..self.start);
            self.start = 0;
            self.buf.shrink_to(BUF_KEPT);
        }
    }

    pub(crate) fn into_inner(self) -> S {
        self.stream
    }

    pub(crate) fn write_all(&mut self, data: &[u8]) -> Result<()> {
        self.stream.write_all(data)?;
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> Result<()> {
        self.stream.flush()?;
        Ok(())
    }

    fn fill(&mut self) -> Result<()> {
        // Compacte : les octets déjà consommés sont libérés.
        if self.start > 0 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        let mut chunk = [0u8; 16 * 1024];
        let n = self.stream.read(&mut chunk)?;
        if n == 0 {
            return Err(Error::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "connexion fermée par le serveur")));
        }
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(())
    }

    /// Une ligne sans son `\r\n` final.
    fn read_line(&mut self) -> Result<Vec<u8>> {
        // Nombre d'octets déjà examinés après `start` (stable malgré le compactage).
        let mut scanned = 0;
        loop {
            let from = self.start + scanned;
            if let Some(pos) = self.buf[from..].iter().position(|&b| b == b'\n') {
                let end = from + pos;
                if end - self.start > MAX_LINE {
                    return Err(exceeded(Limit::Line));
                }
                let mut line = self.buf[self.start..end].to_vec();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.start = end + 1;
                return Ok(line);
            }
            scanned = self.buf.len() - self.start;
            // Ligne sans fin : on s'arrête avant d'épuiser la mémoire.
            if scanned > MAX_LINE {
                return Err(exceeded(Limit::Line));
            }
            self.fill()?;
        }
    }

    /// Ajoute à `out` les `n` octets d'un littéral. Ce qui n'est pas déjà dans
    /// le tampon est lu directement dans `out` (audit P10) : le tampon garde sa
    /// taille ordinaire, et le littéral n'est pas recopié une fois de plus.
    fn read_literal_into(&mut self, out: &mut Vec<u8>, n: usize) -> Result<()> {
        // Refusé avant de lire quoi que ce soit : la taille vient du serveur.
        if n > MAX_LITERAL {
            return Err(exceeded(Limit::Literal));
        }
        let buffered = (self.buf.len() - self.start).min(n);
        out.reserve(n);
        out.extend_from_slice(&self.buf[self.start..self.start + buffered]);
        self.start += buffered;
        let mut at = out.len();
        out.resize(at + (n - buffered), 0);
        while at < out.len() {
            let read = self.stream.read(&mut out[at..])?;
            if read == 0 {
                return Err(Error::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "connexion fermée par le serveur")));
            }
            at += read;
        }
        Ok(())
    }

    /// Une réponse complète, littéraux inclus (sous la forme `{n}\r\n<octets>`).
    pub(crate) fn read_response(&mut self) -> Result<Vec<u8>> {
        self.responses += 1;
        if self.responses > MAX_RESPONSES {
            return Err(exceeded(Limit::Responses));
        }
        let mut out = Vec::new();
        let mut literals = 0;
        loop {
            let line = self.read_line()?;
            self.text = self.text.saturating_add(line.len());
            if self.text > MAX_COMMAND_TEXT {
                return Err(exceeded(Limit::CommandText));
            }
            out.extend_from_slice(&line);
            match literal_len(&line) {
                Some(n) => {
                    literals += 1;
                    if literals > MAX_LITERALS {
                        return Err(exceeded(Limit::Literals));
                    }
                    self.literal_bytes = self.literal_bytes.saturating_add(n);
                    if self.literal_bytes > MAX_COMMAND_LITERALS {
                        return Err(exceeded(Limit::CommandLiterals));
                    }
                    out.extend_from_slice(b"\r\n");
                    self.read_literal_into(&mut out, n)?;
                }
                None => return Ok(out),
            }
        }
    }
}

/// Taille du littéral annoncé en fin de ligne : `... {123}` ou `... {123+}`.
pub(crate) fn literal_len(line: &[u8]) -> Option<usize> {
    let inner = line.strip_suffix(b"}")?;
    let open = inner.iter().rposition(|&b| b == b'{')?;
    let digits = &inner[open + 1..];
    let digits = digits.strip_suffix(b"+").unwrap_or(digits);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Duplex(Cursor<Vec<u8>>);
    impl Read for Duplex {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            // Un octet à la fois : vérifie la gestion des lectures partielles.
            let n = b.len().min(1);
            self.0.read(&mut b[..n])
        }
    }
    impl Write for Duplex {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn reads_lines_and_literals_byte_by_byte() {
        let data = b"* OK ready\r\n* 1 FETCH (BODY[] {5}\r\nab\r\nc UID 7)\r\nA1 OK done\r\n".to_vec();
        let mut c = Connection::new(Duplex(Cursor::new(data)));
        assert_eq!(c.read_response().unwrap(), b"* OK ready");
        assert_eq!(c.read_response().unwrap(), b"* 1 FETCH (BODY[] {5}\r\nab\r\nc UID 7)");
        assert_eq!(c.read_response().unwrap(), b"A1 OK done");
        assert!(c.read_response().is_err());
    }

    /// Flux lu par blocs entiers, pour les tests de plafonds (gros volumes).
    struct Fast(Cursor<Vec<u8>>);
    impl Read for Fast {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(b)
        }
    }
    impl Write for Fast {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn fast(data: Vec<u8>) -> Connection<Fast> {
        Connection::new(Fast(Cursor::new(data)))
    }

    fn is_limit(r: Result<Vec<u8>>) -> bool {
        matches!(r, Err(Error::Protocol(t)) if t.contains("trop volumineuse"))
    }

    #[test]
    fn line_without_end_is_refused() {
        // Une ligne sans fin : refusée au-delà du plafond, sans attendre la fin du flux.
        let mut data = b"* OK ".to_vec();
        data.resize(MAX_LINE + 64 * 1024, b'a');
        assert!(is_limit(fast(data).read_response()));
    }

    #[test]
    fn long_line_within_limit_is_read() {
        let mut data = b"* SEARCH".to_vec();
        while data.len() < MAX_LINE - 16 {
            data.extend_from_slice(b" 1234567");
        }
        let len = data.len();
        data.extend_from_slice(b"\r\n");
        // Le résultat d'une recherche sur un très gros dossier passe encore.
        assert_eq!(fast(data).read_response().unwrap().len(), len);
    }

    #[test]
    fn huge_literal_is_refused_before_reading() {
        // Taille maximale d'un `usize`, puis une taille juste au-dessus du plafond :
        // l'erreur tombe tout de suite, sans lire ni réserver ces octets.
        let mut c = fast(b"* 1 FETCH (BODY[] {18446744073709551615}\r\nx)\r\n".to_vec());
        assert!(is_limit(c.read_response()));
        let line = format!("* 1 FETCH (BODY[] {{{}}}\r\n", MAX_LITERAL + 1);
        assert!(is_limit(fast(line.into_bytes()).read_response()));
    }

    #[test]
    fn too_many_literals_in_one_response() {
        let mut data = b"* 1 FETCH (".to_vec();
        for _ in 0..=MAX_LITERALS {
            data.extend_from_slice(b"X {1}\r\nx ");
        }
        data.extend_from_slice(b")\r\n");
        assert!(is_limit(fast(data).read_response()));
    }

    #[test]
    fn responses_per_command_are_capped_and_reset() {
        let data = b"* 1 EXISTS\r\n".repeat(MAX_RESPONSES + 1);
        let mut c = fast(data);
        for _ in 0..MAX_RESPONSES {
            c.read_response().unwrap();
        }
        assert!(is_limit(c.read_response()));

        // Une nouvelle commande repart de zéro.
        let mut c = fast(b"* 1 EXISTS\r\n".repeat(MAX_RESPONSES + 1));
        for _ in 0..MAX_RESPONSES {
            c.read_response().unwrap();
        }
        c.begin_command();
        assert!(c.read_response().is_ok());
    }

    #[test]
    fn literal_detection() {
        assert_eq!(literal_len(b"* 1 FETCH (BODY[] {123}"), Some(123));
        assert_eq!(literal_len(b"A APPEND x {5+}"), Some(5));
        assert_eq!(literal_len(b"* OK {weird}"), None);
        assert_eq!(literal_len(b"* OK done"), None);
    }

    #[test]
    fn p10_big_literal_does_not_grow_the_buffer() {
        // Message de 4 Mo : lu directement dans la réponse, le tampon reste petit.
        let size = 4 << 20;
        let mut data = format!("* 1 FETCH (BODY[] {{{size}}}\r\n").into_bytes();
        data.resize(data.len() + size, b'x');
        data.extend_from_slice(b" UID 7)\r\nA1 OK fini\r\n");
        let mut c = fast(data);
        let response = c.read_response().unwrap();
        assert_eq!(response.len(), "* 1 FETCH (BODY[] {4194304}\r\n".len() + size + " UID 7)".len());
        assert!(response.ends_with(b"xx UID 7)"));
        assert!(c.buf.capacity() <= BUF_KEPT, "tampon de {} octets", c.buf.capacity());
        assert_eq!(c.read_response().unwrap(), b"A1 OK fini");
    }

    #[test]
    fn p10_buffer_shrinks_after_a_long_line() {
        let mut data = b"* SEARCH".to_vec();
        while data.len() < 2 << 20 {
            data.extend_from_slice(b" 1234567");
        }
        data.extend_from_slice(b"\r\nA1 OK fini\r\n");
        let mut c = fast(data);
        c.read_response().unwrap();
        assert!(c.buf.capacity() > BUF_KEPT, "la longue ligne a grossi le tampon");
        c.begin_command();
        assert!(c.buf.capacity() <= BUF_KEPT, "tampon de {} octets", c.buf.capacity());
        // Ce qui restait à lire n'est pas perdu.
        assert_eq!(c.read_response().unwrap(), b"A1 OK fini");
    }
}
