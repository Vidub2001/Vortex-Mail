//! Langage de recherche (lot E7) : `de:`, `à:`, `objet:`, `avant:`, `après:`,
//! `est:nonlu`, `est:suivi`, `avec:pj`, `"phrase exacte"`, `-mot`, et leurs
//! équivalents anglais (`from:`, `to:`, `subject:`, `before:`, `after:`,
//! `is:unread`, `is:flagged`, `has:attachment`). Le même [`SearchQuery`] sert
//! à la recherche dans le cache et à `UID SEARCH` sur les serveurs.

use std::fmt;

/// Date de calendrier, sans heure ni fuseau.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: i32,
    month: u8,
    day: u8,
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

impl Date {
    /// Date valide (29 février compris les années bissextiles), sinon `None`.
    #[must_use]
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let last = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        ((1..=9999).contains(&year) && (1..=last).contains(&day)).then_some(Self { year, month, day })
    }

    /// Lit `2026-11-16`, `16/11/2026` ou `16.11.2026` (jour avant le mois, comme en France).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split(['-', '/', '.']).collect();
        let &[a, b, c] = parts.as_slice() else { return None };
        let number = |s: &str| s.parse::<u32>().ok();
        let (year, month, day) = if a.len() == 4 { (number(a)?, number(b)?, number(c)?) } else { (number(c)?, number(b)?, number(a)?) };
        Self::new(i32::try_from(year).ok()?, u8::try_from(month).ok()?, u8::try_from(day).ok()?)
    }

    /// Nombre de jours depuis le 1er janvier 1970.
    #[must_use]
    pub fn days_since_epoch(self) -> i64 {
        // Algorithme « days from civil » de Howard Hinnant.
        let (month, day) = (i64::from(self.month), i64::from(self.day));
        let year = i64::from(self.year) - i64::from(month <= 2);
        let era = if year >= 0 { year } else { year - 399 } / 400;
        let yoe = year - era * 400;
        let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// Début du jour en secondes depuis 1970, à minuit UTC.
    #[must_use]
    pub fn utc_midnight(self) -> i64 {
        self.days_since_epoch() * 86_400
    }

    /// Forme des dates de `SEARCH` en IMAP : `16-Nov-2026` (RFC 3501).
    #[must_use]
    pub fn imap(self) -> String {
        format!("{}-{}-{}", self.day, MONTHS[usize::from(self.month - 1)], self.year)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Recherche décomposée : chaque critère rempli doit être vrai (ET).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Mots ou phrases à trouver (objet, expéditeur, aperçu ; texte entier sur le serveur).
    pub words: Vec<String>,
    /// Mots ou phrases à exclure (`-mot`).
    pub excluded: Vec<String>,
    /// Expéditeur, nom ou adresse (`de:`).
    pub from: Vec<String>,
    /// Destinataire, en copie compris (`à:`).
    pub to: Vec<String>,
    /// Objet (`objet:`).
    pub subject: Vec<String>,
    /// Reçu avant ce jour, ce jour exclu (`avant:`).
    pub before: Option<Date>,
    /// Reçu ce jour ou après (`après:`).
    pub after: Option<Date>,
    /// `Some(true)` : non lus seulement (`est:nonlu`) ; `Some(false)` : lus (`est:lu`).
    pub unread: Option<bool>,
    /// Suivis seulement (`est:suivi`).
    pub flagged: bool,
    /// Avec pièce jointe seulement (`avec:pj`).
    pub attachment: bool,
}

/// Mots de la recherche, guillemets respectés : `de:"Jean Dupont" "mot à mot"`.
/// Les espaces collés aux guillemets français (`« mot »`) sont retirés.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut push = |current: &mut String| {
        let token = current.trim();
        if !token.is_empty() {
            out.push(token.to_string());
        }
        current.clear();
    };
    for c in text.chars() {
        match c {
            '"' | '«' | '»' | '“' | '”' => quoted = !quoted && c != '»' && c != '”',
            c if c.is_whitespace() && !quoted => push(&mut current),
            c => current.push(c),
        }
    }
    push(&mut current);
    out
}

/// Clé sans accents ni majuscules : `Après` → `apres`.
fn plain_key(key: &str) -> String {
    key.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' => 'a',
            'é' | 'è' | 'ê' => 'e',
            'ï' | 'î' => 'i',
            'ô' => 'o',
            'ù' | 'û' => 'u',
            c => c,
        })
        .collect()
}

impl SearchQuery {
    /// Décompose le texte saisi. Un mot de la forme `clé:valeur` dont la clé
    /// n'est pas reconnue reste un mot ordinaire (`12:30`, `http://…`).
    ///
    /// # Errors
    /// Date illisible après `avant:` ou `après:`, ou valeur inconnue après
    /// `est:` ou `avec:` : message traduit qui cite le mot fautif.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut q = Self::default();
        for token in tokens(text) {
            if let Some(word) = token.strip_prefix('-').filter(|w| !w.is_empty()) {
                q.excluded.push(word.to_string());
                continue;
            }
            let Some((key, value)) = token.split_once(':').filter(|(k, v)| !k.is_empty() && !v.trim().is_empty()) else {
                q.words.push(token);
                continue;
            };
            // `objet:« ordre du jour »` : espaces des guillemets français retirés.
            let value = value.trim().to_string();
            match plain_key(key).as_str() {
                "de" | "from" => q.from.push(value),
                "a" | "to" | "pour" => q.to.push(value),
                "objet" | "sujet" | "subject" => q.subject.push(value),
                "avant" | "before" => q.before = Some(Self::day(&token, &value)?),
                "apres" | "depuis" | "after" | "since" => q.after = Some(Self::day(&token, &value)?),
                "est" | "is" => match plain_key(&value).replace(['-', ' '], "").as_str() {
                    "nonlu" | "unread" => q.unread = Some(true),
                    "lu" | "read" => q.unread = Some(false),
                    "suivi" | "flagged" | "indicateur" => q.flagged = true,
                    _ => return Err(Self::unknown(&token)),
                },
                "avec" | "has" => match plain_key(&value).replace(['-', ' '], "").as_str() {
                    "pj" | "piecejointe" | "piecesjointes" | "attachment" | "attachments" => q.attachment = true,
                    _ => return Err(Self::unknown(&token)),
                },
                _ => q.words.push(token),
            }
        }
        Ok(q)
    }

    fn day(token: &str, value: &str) -> Result<Date, String> {
        Date::parse(value).ok_or_else(|| {
            let what = crate::lang::pick(
                "Date non comprise (écrire par exemple avant:16/11/2026) :",
                "Date not understood (write for example before:2026-11-16):",
                "Fecha no comprendida (escriba por ejemplo antes:16/11/2026):",
            );
            format!("{what} « {token} »")
        })
    }

    fn unknown(token: &str) -> String {
        let what = crate::lang::pick(
            "Critère inconnu (est:nonlu, est:lu, est:suivi, avec:pj) :",
            "Unknown criterion (is:unread, is:read, is:flagged, has:attachment):",
            "Criterio desconocido (est:nonlu, est:lu, est:suivi, avec:pj):",
        );
        format!("{what} « {token} »")
    }

    /// Aucun critère : rien à chercher.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        let d = Date::parse("16/11/2026").unwrap();
        assert_eq!(d, Date::parse("2026-11-16").unwrap());
        assert_eq!(d, Date::parse("16.11.2026").unwrap());
        assert_eq!(d.to_string(), "2026-11-16");
        assert_eq!(d.imap(), "16-Nov-2026");
        assert_eq!(d.utc_midnight(), 1_794_787_200);
        assert_eq!(Date::parse("1970-01-01").unwrap().days_since_epoch(), 0);
        assert_eq!(Date::parse("29/02/2024").unwrap().imap(), "29-Feb-2024");
        assert_eq!(Date::parse("29/02/2026"), None, "pas bissextile");
        assert_eq!(Date::parse("31/04/2026"), None);
        assert_eq!(Date::parse("16/11"), None, "année obligatoire");
        assert_eq!(Date::parse("hier"), None);
    }

    #[test]
    fn e7_langage_de_recherche() {
        let q = SearchQuery::parse(
            r#"de:marie Objet:"ordre du jour" après:01/11/2026 avant:2026-11-16 est:non-lu avec:pj budget -brouillon "mot à mot""#,
        )
        .unwrap();
        assert_eq!(q.from, ["marie"]);
        assert_eq!(q.subject, ["ordre du jour"]);
        assert_eq!(q.after, Date::new(2026, 11, 1));
        assert_eq!(q.before, Date::new(2026, 11, 16));
        assert_eq!(q.unread, Some(true));
        assert!(q.attachment);
        assert_eq!(q.words, ["budget", "mot à mot"]);
        assert_eq!(q.excluded, ["brouillon"]);

        let en = SearchQuery::parse("from:a@x.fr to:b@y.fr subject:devis is:flagged has:attachment since:2026-01-01").unwrap();
        assert_eq!((en.from.len(), en.to.len(), en.subject.len()), (1, 1, 1));
        assert!(en.flagged && en.attachment && en.after.is_some());
        assert_eq!(SearchQuery::parse("à:paul est:lu").unwrap().unread, Some(false));
        assert_eq!(SearchQuery::parse("À:paul").unwrap().to, ["paul"]);

        // Clé inconnue, valeur vide, tiret seul : mots ordinaires.
        assert_eq!(
            SearchQuery::parse("rendez-vous 12:30 http://x.fr de: -").unwrap().words,
            ["rendez-vous", "12:30", "http://x.fr", "de:", "-"]
        );
        // Guillemets français.
        assert_eq!(SearchQuery::parse("« petit déjeuner »").unwrap().words, ["petit déjeuner"]);
        assert_eq!(SearchQuery::parse("objet:« ordre du jour »").unwrap().subject, ["ordre du jour"]);

        assert!(SearchQuery::parse("avant:32/13/2026").unwrap_err().contains("avant:32/13/2026"));
        assert!(SearchQuery::parse("est:important").unwrap_err().contains("est:important"));
        assert_eq!(SearchQuery::parse("   ").unwrap(), SearchQuery::default(), "rien à chercher");
        assert!(!SearchQuery::parse("est:suivi").unwrap().is_empty());
    }
}
