//! Lecture et écriture JSON minimales pour l'API d'Outlook.com : un arbre de
//! valeurs, sans dépendance. Les objets gardent l'ordre de leurs clés.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

static NULL: Json = Json::Null;

impl Json {
    /// Objet à partir de paires clé / valeur.
    pub(crate) fn obj<const N: usize>(pairs: [(&str, Json); N]) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// Valeur d'une clé (`Null` si absente ou si ce n'est pas un objet).
    pub(crate) fn get(&self, key: &str) -> &Json {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map_or(&NULL, |(_, v)| v),
            _ => &NULL,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Texte de la valeur, ou chaîne vide.
    pub(crate) fn text(&self) -> &str {
        self.as_str().unwrap_or("")
    }

    pub(crate) fn as_bool(&self) -> bool {
        matches!(self, Json::Bool(true))
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::as_conversions,
        reason = "entier vérifié (pas de décimales, moins de 2^53) : conversion exacte"
    )]
    pub(crate) fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 => Some(*n as i64),
            _ => None,
        }
    }

    pub(crate) fn items(&self) -> &[Json] {
        match self {
            Json::Arr(v) => v,
            _ => &[],
        }
    }

    /// Valeur absente ou `null`.
    pub(crate) fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    /// Ajoute ou remplace une clé (objet seulement).
    pub(crate) fn set(&mut self, key: &str, value: Json) {
        if let Json::Obj(pairs) = self {
            match pairs.iter_mut().find(|(k, _)| k == key) {
                Some((_, v)) => *v = value,
                None => pairs.push((key.to_string(), value)),
            }
        }
    }

    pub(crate) fn to_text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::as_conversions,
        reason = "entier vérifié (pas de décimales, moins de 10^15) : conversion exacte"
    )]
    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    let _ = write!(out, "{}", *n as i64);
                } else {
                    let _ = write!(out, "{n}");
                }
            }
            Json::Str(s) => write_str(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Json::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::Str(s.to_string())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::Str(s)
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Json {
        Json::Bool(b)
    }
}

impl From<i64> for Json {
    #[expect(clippy::cast_precision_loss, clippy::as_conversions, reason = "les nombres JSON sont des flottants ; exacts jusqu'à 2^53")]
    fn from(n: i64) -> Json {
        Json::Num(n as f64)
    }
}

impl From<Vec<Json>> for Json {
    fn from(v: Vec<Json>) -> Json {
        Json::Arr(v)
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ================================================================ lecture

pub(crate) fn parse(text: &str) -> Result<Json, String> {
    let mut p = Parser { s: text.as_bytes(), i: 0, depth: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(p.error(tr!("texte après la valeur")));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn error(&self, what: &str) -> String {
        tr!("JSON illisible ({what}, position {pos})", what = what, pos = self.i)
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn expect(&mut self, b: u8) -> Result<(), String> {
        if self.peek() == Some(b) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.error(&tr!("« {c} » attendu", c = char::from(b))))
        }
    }

    fn literal(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.error(tr!("valeur inconnue")))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.error(tr!("valeur attendue"))),
        }
    }

    fn nested(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > 200 {
            return Err(self.error(tr!("imbrication trop profonde")));
        }
        Ok(())
    }

    fn object(&mut self) -> Result<Json, String> {
        self.nested()?;
        self.i += 1;
        let mut pairs = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Json::Obj(pairs));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            self.expect(b':')?;
            self.ws();
            let v = self.value()?;
            pairs.push((key, v));
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Json::Obj(pairs));
                }
                _ => return Err(self.error(tr!("« , » ou « } » attendu"))),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.nested()?;
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(self.error(tr!("« , » ou « ] » attendu"))),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.s.get(self.i..self.i + 4).ok_or_else(|| self.error(tr!("\\u incomplet")))?;
        // Quatre chiffres hexadécimaux exactement : `from_str_radix` admettrait `+41`.
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return Err(self.error(tr!("\\u invalide")));
        }
        let v = std::str::from_utf8(digits)
            .ok()
            .and_then(|d| u32::from_str_radix(d, 16).ok())
            .ok_or_else(|| self.error(tr!("\\u invalide")))?;
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(b) = self.peek() else { return Err(self.error(tr!("chaîne non terminée"))) };
            self.i += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(e) = self.peek() else { return Err(self.error(tr!("échappement incomplet"))) };
                    self.i += 1;
                    let c = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) && self.s[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF)
                            } else {
                                hi
                            };
                            char::from_u32(code).unwrap_or('\u{FFFD}')
                        }
                        _ => return Err(self.error(tr!("échappement inconnu"))),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                // Caractère de contrôle brut : interdit par la RFC 8259 (§7).
                0x00..=0x1f => return Err(self.error(tr!("caractère de contrôle dans une chaîne"))),
                _ => out.push(b),
            }
        }
        String::from_utf8(out).map_err(|_| self.error(tr!("UTF-8 invalide")))
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
            self.i += 1;
        }
        // Grammaire de la RFC 8259 (§6) et valeur finie : `01`, `1.`, `+1`
        // et `1e999` (infini, réécrit « inf » : plus du JSON) sont refusés
        // (relecture complète n° 46).
        std::str::from_utf8(&self.s[start..self.i])
            .ok()
            .filter(|t| valid_number(t))
            .and_then(|t| t.parse::<f64>().ok())
            .filter(|n| n.is_finite())
            .map(Json::Num)
            .ok_or_else(|| self.error(tr!("nombre invalide")))
    }
}

/// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?` (RFC 8259 §6).
fn valid_number(t: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let t = t.strip_prefix('-').unwrap_or(t);
    let (mantissa, exponent) = match t.find(['e', 'E']) {
        Some(i) => (&t[..i], Some(&t[i + 1..])),
        None => (t, None),
    };
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    let int_ok = digits(int) && (int == "0" || !int.starts_with('0'));
    let frac_ok = frac.is_none_or(digits);
    let exp_ok = exponent.is_none_or(|e| digits(e.strip_prefix(['+', '-']).unwrap_or(e)));
    int_ok && frac_ok && exp_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_like_document() {
        let text = r#"{"@odata.nextLink":"https://x/y?$skip=2","value":[
            {"id":"AAMk=","subject":"Réunion \"équipe\"","isAllDay":false,
             "start":{"dateTime":"2026-11-04T10:00:00.0000000","timeZone":"UTC"},
             "attendees":[],"importance":null,"n":-12.5e1}
        ]}"#;
        let v = parse(text).unwrap();
        assert_eq!(v.get("@odata.nextLink").text(), "https://x/y?$skip=2");
        let e = &v.get("value").items()[0];
        assert_eq!(e.get("subject").text(), "Réunion \"équipe\"");
        assert!(!e.get("isAllDay").as_bool());
        assert_eq!(e.get("start").get("dateTime").text(), "2026-11-04T10:00:00.0000000");
        assert!(e.get("importance").is_null());
        assert!(e.get("missing").is_null());
        assert_eq!(e.get("n"), &Json::Num(-125.0));
        assert_eq!(parse("\"\\ud83d\\ude00\"").unwrap().text(), "😀");
    }

    #[test]
    fn round_trip() {
        let mut v = Json::obj([
            ("subject", "Dîner \"chez\" nous\n".into()),
            ("isAllDay", true.into()),
            ("count", 3i64.into()),
            ("list", vec![Json::Null, "a".into()].into()),
        ]);
        v.set("count", 4i64.into());
        v.set("extra", "x".into());
        let text = v.to_text();
        assert_eq!(text, r#"{"subject":"Dîner \"chez\" nous\n","isAllDay":true,"count":4,"list":[null,"a"],"extra":"x"}"#);
        assert_eq!(parse(&text).unwrap(), v);
    }

    #[test]
    fn errors() {
        assert!(parse("{").is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("{} x").is_err());
        assert!(parse("").is_err());
        // Relecture complète n° 46 : grammaire des nombres, valeur finie,
        // \u à quatre chiffres, pas de caractère de contrôle brut.
        for bad in ["01", "1.", "+1", ".5", "1e", "1e999", "-", "[\"\\u+041\"]", "[\"a\u{1}b\"]"] {
            assert!(parse(bad).is_err(), "refusé : {bad}");
        }
        for good in ["0", "-0.5", "12e3", "1E-2", "1.25e+2"] {
            assert!(parse(good).is_ok(), "accepté : {good}");
        }
    }
}
