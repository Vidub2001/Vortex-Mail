//! Analyse des réponses IMAP en jetons.

use crate::limits::{exceeded, Limit, MAX_DEPTH};
use crate::{Error, Result};

/// Élément d'une réponse IMAP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// Atome : `FETCH`, `\Seen`, `123`, `BODY[HEADER.FIELDS (FROM)]`…
    Atom(String),
    /// Chaîne entre guillemets ou littéral `{n}`.
    Str(Vec<u8>),
    /// Liste entre parenthèses.
    List(Vec<Token>),
    /// `NIL` : valeur absente.
    Nil,
}

impl Token {
    /// Vrai si le jeton est l'atome `name` (casse ignorée).
    #[must_use]
    pub fn is_atom(&self, name: &str) -> bool {
        matches!(self, Token::Atom(a) if a.eq_ignore_ascii_case(name))
    }

    /// Texte d'un atome ou d'une chaîne (UTF-8 avec remplacement si besoin).
    #[must_use]
    pub fn text(&self) -> Option<String> {
        match self {
            Token::Atom(a) => Some(a.clone()),
            Token::Str(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }
    }

    /// Octets d'un atome ou d'une chaîne, sans conversion.
    #[must_use]
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Token::Atom(a) => Some(a.as_bytes()),
            Token::Str(b) => Some(b),
            _ => None,
        }
    }

    /// Valeur numérique d'un atome.
    #[must_use]
    pub fn number(&self) -> Option<u64> {
        match self {
            Token::Atom(a) => a.parse().ok(),
            _ => None,
        }
    }

    /// Éléments d'une liste.
    #[must_use]
    pub fn list(&self) -> Option<&[Token]> {
        match self {
            Token::List(l) => Some(l),
            _ => None,
        }
    }
}

/// Statut d'une réponse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// `OK` : succès.
    Ok,
    /// `NO` : refus.
    No,
    /// `BAD` : commande incomprise.
    Bad,
    /// `BYE` : le serveur ferme la session.
    Bye,
    /// `PREAUTH` : session déjà authentifiée.
    Preauth,
}

/// Réponse du serveur, une ligne (littéraux compris).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// `* OK …`, `A001 NO …` : statut, avec son code `[…]` éventuel.
    Status {
        /// Étiquette de la commande, absente pour `*`.
        tag: Option<String>,
        /// `OK`, `NO`, `BAD`, `BYE` ou `PREAUTH`.
        kind: StatusKind,
        /// Code entre crochets (`[TRYCREATE]`, `[ALERT]`…).
        code: Option<String>,
        /// Texte lisible qui suit.
        text: String,
    },
    /// Données non étiquetées : `* LIST …`, `* 3 EXISTS`, `* 1 FETCH (…)`.
    Data(Vec<Token>),
    /// `+ …` : le serveur attend la suite (littéral, authentification).
    Continuation(String),
}

fn split_word(s: &[u8]) -> (&[u8], &[u8]) {
    match s.iter().position(|&b| b == b' ') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, &[]),
    }
}

fn status_kind(word: &[u8]) -> Option<StatusKind> {
    match word.to_ascii_uppercase().as_slice() {
        b"OK" => Some(StatusKind::Ok),
        b"NO" => Some(StatusKind::No),
        b"BAD" => Some(StatusKind::Bad),
        b"BYE" => Some(StatusKind::Bye),
        b"PREAUTH" => Some(StatusKind::Preauth),
        _ => None,
    }
}

fn status(tag: Option<String>, kind: StatusKind, rest: &[u8]) -> Response {
    let text = String::from_utf8_lossy(rest).trim().to_string();
    if let Some(inner) = text.strip_prefix('[') {
        if let Some(close) = inner.find(']') {
            return Response::Status { tag, kind, code: Some(inner[..close].to_string()), text: inner[close + 1..].trim().to_string() };
        }
    }
    Response::Status { tag, kind, code: None, text }
}

/// Analyse une réponse complète du serveur (ligne et littéraux, sans le `\r\n` final).
///
/// # Errors
///
/// Réponse mal formée, ou qui dépasse les plafonds de lecture ([`Error::Protocol`]).
pub fn parse_response(raw: &[u8]) -> Result<Response> {
    if let Some(rest) = raw.strip_prefix(b"+") {
        return Ok(Response::Continuation(String::from_utf8_lossy(rest).trim().to_string()));
    }
    let (tag, rest) = split_word(raw);
    if tag == b"*" {
        let (word, after) = split_word(rest);
        return match status_kind(word) {
            Some(kind) => Ok(status(None, kind, after)),
            None => Ok(Response::Data(parse_tokens(rest)?)),
        };
    }
    let (word, after) = split_word(rest);
    match status_kind(word) {
        Some(kind @ (StatusKind::Ok | StatusKind::No | StatusKind::Bad)) => {
            Ok(status(Some(String::from_utf8_lossy(tag).into_owned()), kind, after))
        }
        _ => Err(Error::Protocol(String::from_utf8_lossy(raw).chars().take(200).collect())),
    }
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
    /// Parenthèses ouvertes : bornée, sinon une ligne `((((…` ferait déborder la pile.
    depth: usize,
}

pub(crate) fn parse_tokens(s: &[u8]) -> Result<Vec<Token>> {
    Lexer { s, i: 0, depth: 0 }.items(None)
}

impl Lexer<'_> {
    fn err(&self, what: &str) -> Error {
        let around: String = String::from_utf8_lossy(&self.s[self.i.saturating_sub(20)..self.s.len().min(self.i + 20)]).into_owned();
        Error::Protocol(format!("{what} vers « {around} »"))
    }

    fn items(&mut self, closing: Option<u8>) -> Result<Vec<Token>> {
        let mut out = Vec::new();
        loop {
            while self.i < self.s.len() && self.s[self.i] == b' ' {
                self.i += 1;
            }
            let Some(&c) = self.s.get(self.i) else {
                return match closing {
                    Some(_) => Err(self.err("liste non fermée")),
                    None => Ok(out),
                };
            };
            if Some(c) == closing {
                self.i += 1;
                return Ok(out);
            }
            let token = match c {
                b'(' => {
                    if self.depth >= MAX_DEPTH {
                        return Err(exceeded(Limit::Depth));
                    }
                    self.i += 1;
                    self.depth += 1;
                    let list = self.items(Some(b')'))?;
                    self.depth -= 1;
                    Token::List(list)
                }
                b'"' => self.quoted()?,
                b'{' => self.literal()?,
                b')' => return Err(self.err("parenthèse fermante inattendue")),
                _ => self.atom(),
            };
            out.push(token);
        }
    }

    fn quoted(&mut self) -> Result<Token> {
        self.i += 1;
        let mut out = Vec::new();
        while let Some(&c) = self.s.get(self.i) {
            self.i += 1;
            match c {
                b'\\' => {
                    if let Some(&n) = self.s.get(self.i) {
                        out.push(n);
                        self.i += 1;
                    }
                }
                b'"' => return Ok(Token::Str(out)),
                _ => out.push(c),
            }
        }
        Err(self.err("chaîne non fermée"))
    }

    fn literal(&mut self) -> Result<Token> {
        let close = self.s[self.i..].iter().position(|&b| b == b'}').ok_or_else(|| self.err("littéral mal formé"))?;
        let spec = &self.s[self.i + 1..self.i + close];
        let spec = spec.strip_suffix(b"+").unwrap_or(spec);
        let n: usize =
            std::str::from_utf8(spec).ok().and_then(|s| s.parse().ok()).ok_or_else(|| self.err("taille de littéral invalide"))?;
        self.i += close + 1;
        if self.s[self.i..].starts_with(b"\r\n") {
            self.i += 2;
        }
        // `checked_add` : une taille géante annoncée par le serveur ne doit pas
        // déborder et faire paniquer la découpe.
        let end = self.i.checked_add(n).filter(|&end| end <= self.s.len()).ok_or_else(|| self.err("littéral tronqué"))?;
        let data = self.s[self.i..end].to_vec();
        self.i = end;
        Ok(Token::Str(data))
    }

    /// Atome ; après un mot-clé de section (`BODY`, `BINARY`…), les crochets
    /// peuvent contenir espaces et parenthèses : `BODY[HEADER.FIELDS (FROM TO)]`.
    /// Ailleurs, `[` est un caractère ordinaire : un mot-clé légal comme `[todo`
    /// posé par un autre logiciel ne doit pas avaler la fin de la liste
    /// (relecture Y7).
    fn atom(&mut self) -> Token {
        let start = self.i;
        let mut depth = 0usize;
        while let Some(&c) = self.s.get(self.i) {
            match c {
                b'[' if depth > 0 || is_section_keyword(&self.s[start..self.i]) => depth += 1,
                b']' => depth = depth.saturating_sub(1),
                b' ' | b'(' | b')' if depth == 0 => break,
                _ => {}
            }
            self.i += 1;
        }
        let text = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
        if text.eq_ignore_ascii_case("NIL") {
            Token::Nil
        } else {
            Token::Atom(text)
        }
    }
}

/// Mot-clé suivi d'une section entre crochets dans une réponse FETCH.
fn is_section_keyword(head: &[u8]) -> bool {
    let upper = head.to_ascii_uppercase();
    matches!(upper.as_slice(), b"BODY" | b"BODY.PEEK" | b"BINARY" | b"BINARY.PEEK" | b"BINARY.SIZE")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_with_bracket_is_a_plain_atom() {
        let r = parse_response(b"* 5 FETCH (UID 9 FLAGS (\\Seen [todo) BODY[HEADER.FIELDS (FROM)] {0}\r\n)").unwrap();
        let Response::Data(tokens) = r else { panic!("réponse de données attendue") };
        let Token::List(items) = &tokens[2] else { panic!("liste attendue") };
        let Token::List(flags) = &items[3] else { panic!("liste des flags attendue") };
        assert_eq!(flags.len(), 2);
        assert!(flags[1].is_atom("[todo"));
        assert!(items[4].is_atom("BODY[HEADER.FIELDS (FROM)]"));
    }

    fn atom(s: &str) -> Token {
        Token::Atom(s.into())
    }

    #[test]
    fn list_response() {
        let r = parse_response(br#"* LIST (\HasNoChildren \Sent) "/" "Envoy&AOk-s""#).unwrap();
        assert_eq!(
            r,
            Response::Data(vec![
                atom("LIST"),
                Token::List(vec![atom("\\HasNoChildren"), atom("\\Sent")]),
                Token::Str(b"/".to_vec()),
                Token::Str(b"Envoy&AOk-s".to_vec()),
            ])
        );
        let r = parse_response(br"* LIST () NIL INBOX").unwrap();
        assert_eq!(r, Response::Data(vec![atom("LIST"), Token::List(vec![]), Token::Nil, atom("INBOX")]));
    }

    #[test]
    fn fetch_with_literal_and_section() {
        let raw = b"* 3 FETCH (UID 42 FLAGS (\\Seen) BODY[HEADER.FIELDS (FROM SUBJECT)] {12}\r\nSubject: a\r\n)";
        let Response::Data(t) = parse_response(raw).unwrap() else { panic!() };
        assert_eq!(t[0], atom("3"));
        assert_eq!(t[1], atom("FETCH"));
        let items = t[2].list().unwrap();
        assert_eq!(items[0], atom("UID"));
        assert_eq!(items[1].number(), Some(42));
        assert_eq!(items[4], atom("BODY[HEADER.FIELDS (FROM SUBJECT)]"));
        assert_eq!(items[5], Token::Str(b"Subject: a\r\n".to_vec()));
    }

    #[test]
    fn statuses_and_codes() {
        assert_eq!(
            parse_response(b"A0003 OK [APPENDUID 38505 3955] APPEND completed").unwrap(),
            Response::Status {
                tag: Some("A0003".into()),
                kind: StatusKind::Ok,
                code: Some("APPENDUID 38505 3955".into()),
                text: "APPEND completed".into()
            }
        );
        // Texte libre avec parenthèse non fermée : ne doit pas planter.
        assert!(matches!(parse_response(b"* OK Bienvenue (serveur").unwrap(), Response::Status { kind: StatusKind::Ok, .. }));
        assert!(matches!(parse_response(b"+ Ready").unwrap(), Response::Continuation(_)));
        assert!(matches!(parse_response(b"* BYE Autologout").unwrap(), Response::Status { kind: StatusKind::Bye, .. }));
        assert!(parse_response(b"A1 WHAT").is_err());
    }

    #[test]
    fn condstore_responses() {
        assert_eq!(
            parse_response(b"* OK [HIGHESTMODSEQ 715194045007] Highest").unwrap(),
            Response::Status { tag: None, kind: StatusKind::Ok, code: Some("HIGHESTMODSEQ 715194045007".into()), text: "Highest".into() }
        );
        assert_eq!(
            parse_response(b"* VANISHED (EARLIER) 300:310,405,411").unwrap(),
            Response::Data(vec![atom("VANISHED"), Token::List(vec![atom("EARLIER")]), atom("300:310,405,411")])
        );
        let Response::Data(t) = parse_response(b"* 1 FETCH (UID 4 MODSEQ (65402) FLAGS (\\Seen))").unwrap() else { panic!() };
        let items = t[2].list().unwrap();
        assert_eq!(items[3], Token::List(vec![atom("65402")]));
    }

    #[test]
    fn deep_nesting_is_refused_without_overflowing_the_stack() {
        // 200 000 parenthèses : faisait déborder la pile et fermait l'application.
        // Le test tourne dans un thread à petite pile, comme la synchronisation.
        let handle = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let mut line = b"* 1 FETCH ".to_vec();
                line.resize(200_000, b'(');
                parse_response(&line)
            })
            .unwrap();
        let result = handle.join().expect("pas de débordement de pile");
        assert!(matches!(result, Err(Error::Protocol(t)) if t.contains("trop volumineuse")));

        // Une imbrication raisonnable (structure MIME profonde) passe.
        let mut line = b"* 1 FETCH ".to_vec();
        line.extend(std::iter::repeat_n(b'(', MAX_DEPTH));
        line.extend(std::iter::repeat_n(b')', MAX_DEPTH));
        assert!(parse_response(&line).is_ok());
    }

    #[test]
    fn giant_literal_size_does_not_panic() {
        // Faisait déborder `self.i + n`, puis paniquer la découpe.
        let raw = b"* 1 FETCH (UID 5 BODY[] {18446744073709551615} x)";
        assert!(matches!(parse_response(raw), Err(Error::Protocol(_))));
        let raw = b"* 1 FETCH (UID 5 BODY[] {18446744073709551600}\r\nx)";
        assert!(matches!(parse_response(raw), Err(Error::Protocol(_))));
    }

    #[test]
    fn quoted_escapes() {
        let t = parse_tokens(br#""a \"b\" \\c""#).unwrap();
        assert_eq!(t, vec![Token::Str(br#"a "b" \c"#.to_vec())]);
    }
}
