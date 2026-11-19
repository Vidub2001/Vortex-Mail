use std::fmt::Write as _;
use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use crate::base64;
use crate::date;
use crate::reader::Connection;
use crate::response::{parse_response, Response, StatusKind, Token};
use crate::utf7;
use crate::{Error, Result};

/// Résultat d'une commande réussie : données reçues et code de la réponse finale.
#[derive(Debug, Default)]
pub struct Outcome {
    pub data: Vec<Response>,
    pub code: Option<String>,
    pub text: String,
}

enum Part<'a> {
    Text(String),
    Literal(&'a [u8]),
}

/// Critère de `UID SEARCH` (RFC 3501 §6.4.4), pour [`Client::uid_search_keys`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchKey {
    /// Mot-clé seul : `ALL`, `SEEN`, `UNSEEN`, `FLAGGED`, `UNFLAGGED`.
    Flag(&'static str),
    /// Mot-clé suivi d'un texte : `TEXT`, `FROM`, `TO`, `CC`, `SUBJECT`.
    /// Un texte non ASCII part en littéral, avec `CHARSET UTF-8`.
    Text(&'static str, String),
    /// Mot-clé suivi d'une date IMAP (`16-Nov-2026`) : `BEFORE`, `SINCE`.
    Date(&'static str, String),
    /// `NOT <critère>`.
    Not(Box<SearchKey>),
    /// `OR <critère> <critère>`.
    Or(Box<SearchKey>, Box<SearchKey>),
}

impl SearchKey {
    /// Un texte du critère demande un littéral (caractère hors ASCII imprimable).
    fn needs_utf8(&self) -> bool {
        match self {
            Self::Text(_, value) => needs_literal(value),
            Self::Not(k) => k.needs_utf8(),
            Self::Or(a, b) => a.needs_utf8() || b.needs_utf8(),
            Self::Flag(_) | Self::Date(..) => false,
        }
    }

    /// Ajoute le critère à la commande : texte en cours dans `line`, littéraux à part.
    fn write<'a>(&'a self, line: &mut String, parts: &mut Vec<Part<'a>>) {
        // Mot-clé et date : lettres, chiffres et tirets seulement, rien d'autre ne passe.
        let atom = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>();
        match self {
            Self::Flag(name) => line.push_str(&atom(name)),
            Self::Date(name, date) => {
                let _ = write!(line, "{} {}", atom(name), atom(date));
            }
            Self::Text(name, value) => {
                let _ = write!(line, "{} ", atom(name));
                if needs_literal(value) {
                    parts.push(Part::Text(std::mem::take(line)));
                    parts.push(Part::Literal(value.as_bytes()));
                } else {
                    line.push_str(&quote(value));
                }
            }
            Self::Not(k) => {
                line.push_str("NOT ");
                k.write(line, parts);
            }
            Self::Or(a, b) => {
                line.push_str("OR ");
                a.write(line, parts);
                line.push(' ');
                b.write(line, parts);
            }
        }
    }
}

/// Dossier annoncé par `LIST`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    /// Nom complet tel que le serveur le connaît (UTF-7 modifié), à renvoyer tel quel.
    pub name: String,
    /// Séparateur de hiérarchie (`/` ou `.`), absent si le serveur n'en a pas.
    pub delimiter: Option<char>,
    /// Attributs (`\Noselect`, `\HasChildren`, `\Sent`…), tels que reçus.
    pub attributes: Vec<String>,
}

impl ListEntry {
    /// Vrai si le dossier porte l'attribut `attr` (casse ignorée).
    #[must_use]
    pub fn has_attribute(&self, attr: &str) -> bool {
        self.attributes.iter().any(|a| a.eq_ignore_ascii_case(attr))
    }

    /// Vrai si le dossier peut être sélectionné (ni `\Noselect` ni `\NonExistent`).
    #[must_use]
    pub fn selectable(&self) -> bool {
        !self.has_attribute("\\Noselect") && !self.has_attribute("\\NonExistent")
    }

    /// Nom complet décodé, ex. `Clients/Société`.
    #[must_use]
    pub fn decoded_name(&self) -> String {
        utf7::decode(&self.name)
    }

    /// Dernier segment décodé, ex. `Société`.
    #[must_use]
    pub fn leaf_name(&self) -> String {
        let decoded = self.decoded_name();
        match self.delimiter {
            Some(d) => decoded.rsplit(d).next().unwrap_or(&decoded).to_string(),
            None => decoded,
        }
    }

    /// Nom (brut) du dossier parent.
    #[must_use]
    pub fn parent(&self) -> Option<String> {
        let d = self.delimiter?;
        self.name.rsplit_once(d).map(|(p, _)| p.to_string())
    }
}

/// Réponse à `STATUS` : compteurs d'un dossier sans le sélectionner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailboxStatus {
    /// Nombre de messages (`MESSAGES`).
    pub messages: u32,
    /// Nombre de messages non lus (`UNSEEN`).
    pub unseen: u32,
    /// Prochain UID attribué (`UIDNEXT`).
    pub uid_next: Option<u32>,
    /// Validité des UID (`UIDVALIDITY`) : s'il change, les UID connus ne valent plus.
    pub uid_validity: Option<u32>,
}

/// Réponse à `SELECT` : état du dossier sélectionné.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectInfo {
    /// Nombre de messages (`EXISTS`).
    pub exists: u32,
    /// Validité des UID (`UIDVALIDITY`) : s'il change, les UID connus ne valent plus.
    pub uid_validity: Option<u32>,
    /// Prochain UID attribué (`UIDNEXT`).
    pub uid_next: Option<u32>,
    /// Dossier ouvert en lecture seule (`[READ-ONLY]`).
    pub read_only: bool,
    /// `HIGHESTMODSEQ` (RFC 7162) : annoncé par un serveur CONDSTORE.
    pub highest_modseq: Option<u64>,
    /// `NOMODSEQ` : le dossier ne gère pas les numéros de modification.
    pub no_modseq: bool,
}

/// Ensemble d'UID reçu du serveur (`1:5,9`), gardé sous forme d'intervalles :
/// un `VANISHED (EARLIER) 1:4000000` ne coûte rien en mémoire.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UidSet {
    ranges: Vec<(u32, u32)>,
}

impl UidSet {
    /// Analyse `1:5,9` ; `*` et les valeurs invalides sont refusés.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let mut ranges = Vec::new();
        for part in s.trim().split(',') {
            let (a, b) = if let Some((a, b)) = part.split_once(':') {
                (a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)
            } else {
                let n = part.parse::<u32>().ok()?;
                (n, n)
            };
            ranges.push((a.min(b), a.max(b)));
        }
        Some(Self { ranges })
    }

    /// Vrai si l'ensemble ne contient aucun intervalle.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Vrai si `uid` appartient à l'un des intervalles.
    #[must_use]
    pub fn contains(&self, uid: u32) -> bool {
        self.ranges.iter().any(|&(a, b)| (a..=b).contains(&uid))
    }

    /// Ajoute les intervalles d'un autre ensemble.
    pub fn extend(&mut self, other: UidSet) {
        self.ranges.extend(other.ranges);
    }

    /// UID un par un (intervalles développés, doublons possibles si le serveur en envoie).
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.ranges.iter().flat_map(|&(a, b)| a..=b)
    }
}

/// Résultat de [`Client::uid_fetch_changed`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Messages modifiés depuis le MODSEQ donné.
    pub items: Vec<FetchItem>,
    /// UID supprimés (`VANISHED`, avec QRESYNC seulement).
    pub vanished: UidSet,
}

/// Réponse `* n FETCH (…)` : paires clé/valeur, clés en majuscules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchItem {
    /// Numéro de séquence du message.
    pub seq: u32,
    /// Paires clé/valeur reçues, dans l'ordre du serveur.
    pub items: Vec<(String, Token)>,
}

impl FetchItem {
    /// Valeur de la clé `key` (casse ignorée).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Token> {
        self.items.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
    }

    /// `UID` du message.
    #[must_use]
    pub fn uid(&self) -> Option<u32> {
        self.get("UID")?.number().and_then(|n| u32::try_from(n).ok())
    }

    /// Taille du message (`RFC822.SIZE`), en octets.
    #[must_use]
    pub fn size(&self) -> Option<u32> {
        self.get("RFC822.SIZE")?.number().and_then(|n| u32::try_from(n).ok())
    }

    /// Drapeaux (`FLAGS`) ; liste vide s'ils n'ont pas été demandés.
    pub fn flags(&self) -> Vec<String> {
        self.get("FLAGS").and_then(Token::list).map_or_default(|l| l.iter().filter_map(Token::text).collect())
    }

    /// Vrai si le message porte le drapeau `flag` (casse ignorée).
    #[must_use]
    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags().iter().any(|f| f.eq_ignore_ascii_case(flag))
    }

    /// `MODSEQ (n)` (RFC 7162).
    #[must_use]
    pub fn modseq(&self) -> Option<u64> {
        match self.get("MODSEQ")? {
            Token::List(l) => l.first()?.number(),
            other => other.number(),
        }
    }

    /// Date de réception côté serveur (`INTERNALDATE`), en secondes Unix.
    #[must_use]
    pub fn internal_date(&self) -> Option<i64> {
        date::parse_internal_date(&self.get("INTERNALDATE")?.text()?)
    }

    /// Contenu d'une section : `body("BODY[]")` pour le message complet,
    /// `body("BODY[HEADER")` pour une sélection d'en-têtes (préfixe).
    #[must_use]
    pub fn body(&self, section_prefix: &str) -> Option<&[u8]> {
        let prefix = section_prefix.to_ascii_uppercase();
        self.items
            .iter()
            .find(|(k, _)| match k.strip_prefix(prefix.as_str()) {
                // `BODY[]` correspond à `BODY[]` ou à une lecture partielle `BODY[]<0>` ;
                // un préfixe ouvert (`BODY[HEADER`) accepte toute suite.
                Some(rest) => rest.is_empty() || rest.starts_with('<') || !prefix.ends_with(']'),
                None => false,
            })
            .and_then(|(_, v)| v.bytes())
    }
}

/// UID par commande, au plus (audit P4) : `uid_set` compresse les suites,
/// mais 5 000 UID non contigus donnent une ligne de 30 000 caractères que
/// certains serveurs refusent. 500 UID font au plus 5 500 caractères.
const UIDS_PER_COMMAND: usize = 500;

/// Ensemble d'UID compact : `[1,2,3,7]` → `1:3,7`.
pub(crate) fn uid_set(uids: &[u32]) -> String {
    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = String::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        if !out.is_empty() {
            out.push(',');
        }
        if start == end {
            out.push_str(&start.to_string());
        } else {
            let _ = write!(out, "{start}:{end}");
        }
        i += 1;
    }
    out
}

/// Chaîne entre guillemets, avec échappement.
pub(crate) fn quote(s: &str) -> String {
    // CR, LF et NUL n'ont pas de forme entre guillemets : retirés, ils ne
    // peuvent pas terminer la commande et en commencer une autre.
    let s: String = s.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')).collect();
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Une chaîne non ASCII ou avec contrôle doit partir en littéral.
fn needs_literal(s: &str) -> bool {
    s.bytes().any(|b| !(0x20..=0x7e).contains(&b))
}

fn astring_part(s: &str) -> Part<'_> {
    if needs_literal(s) {
        Part::Literal(s.as_bytes())
    } else {
        Part::Text(quote(s))
    }
}

fn parse_capabilities(text: &str) -> Vec<String> {
    text.split_whitespace().skip_while(|w| w.eq_ignore_ascii_case("CAPABILITY")).map(str::to_ascii_uppercase).collect()
}

fn code_number<T: std::str::FromStr>(code: Option<&str>, name: &str) -> Option<T> {
    let code = code?;
    let mut words = code.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case(name) {
        return None;
    }
    words.next()?.parse().ok()
}

/// UID des réponses `* VANISHED [(EARLIER)] <ensemble>` (QRESYNC).
fn vanished_uids(data: &[Response]) -> UidSet {
    let mut set = UidSet::default();
    for r in data {
        let Response::Data(t) = r else { continue };
        if !t.first().is_some_and(|x| x.is_atom("VANISHED")) {
            continue;
        }
        // `(EARLIER)` facultatif avant l'ensemble.
        let uids = if t.get(1).is_some_and(|x| x.list().is_some()) { t.get(2) } else { t.get(1) };
        if let Some(parsed) = uids.and_then(Token::text).as_deref().and_then(UidSet::parse) {
            set.extend(parsed);
        }
    }
    set
}

/// Réponse `* STATUS <dossier> (MESSAGES n UNSEEN n …)`.
fn parse_status(r: &Response) -> Option<(String, MailboxStatus)> {
    let Response::Data(t) = r else { return None };
    if !t.first().is_some_and(|x| x.is_atom("STATUS")) {
        return None;
    }
    let name = t.get(1)?.text()?;
    let items = t.get(2).and_then(Token::list)?;
    let mut st = MailboxStatus::default();
    for pair in items.chunks(2) {
        let (Some(k), Some(v)) = (pair[0].text(), pair.get(1).and_then(Token::number)) else {
            continue;
        };
        let v = u32::try_from(v).unwrap_or(u32::MAX);
        match k.to_ascii_uppercase().as_str() {
            "MESSAGES" => st.messages = v,
            "UNSEEN" => st.unseen = v,
            "UIDNEXT" => st.uid_next = Some(v),
            "UIDVALIDITY" => st.uid_validity = Some(v),
            _ => {}
        }
    }
    Some((name, st))
}

/// Session IMAP sur un flux `S` (en clair ou TLS, fourni par l'appelant).
/// La borne `Read + Write` est portée par les blocs `impl` (C-STRUCT-BOUNDS).
pub struct Client<S> {
    conn: Connection<S>,
    tag: u32,
    capabilities: Vec<String>,
    selected: Option<String>,
    /// `UIDVALIDITY` du dossier sélectionné, tel que `SELECT` l'a annoncé.
    selected_validity: Option<u32>,
    /// Extensions activées par `ENABLE` (en majuscules).
    enabled: Vec<String>,
    /// `ENABLE` déjà tenté (une seule fois par session).
    enable_tried: bool,
    /// Un dossier a déjà été sélectionné : `ENABLE` n'est plus permis.
    has_selected: bool,
    /// Textes `[ALERT]` reçus et pas encore relevés (RFC 3501 §7.1 : ils
    /// doivent être montrés à l'utilisateur ; relecture complète n° 22).
    alerts: Vec<String>,
}

/// N'affiche ni le flux ni ce qui y transite (identifiants, contenu des messages).
impl<S> std::fmt::Debug for Client<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("tag", &self.tag)
            .field("selected", &self.selected)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl<S: Read + Write> Client<S> {
    /// Lit le message d'accueil du serveur.
    ///
    /// # Errors
    ///
    /// Connexion perdue, accueil `BYE` ([`Error::Bye`]), ou accueil illisible ou
    /// inattendu ([`Error::Protocol`]).
    pub fn connect(stream: S) -> Result<Self> {
        let mut client = Self::resume(stream);
        let raw = client.conn.read_response()?;
        let greeting = parse_response(&raw)?;
        client.note_alert(&greeting);
        match greeting {
            Response::Status { tag: None, kind: StatusKind::Ok | StatusKind::Preauth, code, .. } => {
                if let Some(code) = code.filter(|c| c.to_ascii_uppercase().starts_with("CAPABILITY ")) {
                    client.capabilities = parse_capabilities(&code);
                }
                Ok(client)
            }
            Response::Status { kind: StatusKind::Bye, text, .. } => Err(Error::Bye(text)),
            _ => {
                Err(Error::Protocol(format!("accueil inattendu : {}", String::from_utf8_lossy(&raw).chars().take(200).collect::<String>())))
            }
        }
    }

    /// Reprend une session sans attendre d'accueil (après STARTTLS).
    pub fn resume(stream: S) -> Self {
        Self {
            conn: Connection::new(stream),
            tag: 0,
            capabilities: Vec::new(),
            selected: None,
            selected_validity: None,
            enabled: Vec::new(),
            enable_tried: false,
            has_selected: false,
            alerts: Vec::new(),
        }
    }

    /// Rend le flux, par exemple pour le chiffrer après `STARTTLS`.
    pub fn into_inner(self) -> S {
        self.conn.into_inner()
    }

    /// Extensions annoncées par le serveur (`CAPABILITY`).
    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// Vrai si le serveur annonce l'extension `cap` (casse ignorée).
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c.eq_ignore_ascii_case(cap))
    }

    /// Extension activée par `ENABLE`, ex. `is_enabled("QRESYNC")`.
    pub fn is_enabled(&self, ext: &str) -> bool {
        self.enabled.iter().any(|c| c.eq_ignore_ascii_case(ext))
    }

    /// `CHANGEDSINCE` et `HIGHESTMODSEQ` utilisables (CONDSTORE, impliqué par QRESYNC).
    pub fn condstore(&self) -> bool {
        self.has_capability("CONDSTORE") || self.has_capability("QRESYNC") || self.is_enabled("CONDSTORE")
    }

    /// QRESYNC activé : les suppressions arrivent en `VANISHED`.
    pub fn qresync(&self) -> bool {
        self.is_enabled("QRESYNC")
    }

    /// `ENABLE <extensions>` (RFC 5161) ; renvoie celles que le serveur a activées.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn enable(&mut self, extensions: &[&str]) -> Result<Vec<String>> {
        self.enable_tried = true;
        let out = self.run(&format!("ENABLE {}", extensions.join(" ")))?;
        let mut now = Vec::new();
        for r in &out.data {
            let Response::Data(t) = r else { continue };
            if t.first().is_some_and(|x| x.is_atom("ENABLED")) {
                now.extend(t[1..].iter().filter_map(Token::text).map(|c| c.to_ascii_uppercase()));
            }
        }
        for ext in &now {
            if !self.is_enabled(ext) {
                self.enabled.push(ext.clone());
            }
        }
        Ok(now)
    }

    /// Active QRESYNC et CONDSTORE s'ils sont annoncés, une fois par session et
    /// seulement avant la première sélection (sinon sans effet). Un refus du
    /// serveur n'est pas une erreur : la synchronisation complète reste possible.
    ///
    /// # Errors
    ///
    /// Connexion perdue ou réponse illisible ; un refus du serveur n'en est pas une.
    pub fn enable_sync_extensions(&mut self) -> Result<()> {
        if self.enable_tried || !self.has_capability("ENABLE") {
            return Ok(());
        }
        if self.has_selected {
            self.enable_tried = true;
            return Ok(());
        }
        let wanted: Vec<&str> = ["CONDSTORE", "QRESYNC"].into_iter().filter(|c| self.has_capability(c)).collect();
        if wanted.is_empty() {
            self.enable_tried = true;
            return Ok(());
        }
        match self.enable(&wanted) {
            Ok(_) | Err(Error::No(_) | Error::Bad(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Dossier actuellement sélectionné (nom brut).
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// `UIDVALIDITY` du dossier sélectionné (RFC 3501 §2.3.1.1) : si elle
    /// diffère de celle connue, les UID mémorisés désignent d'autres messages.
    #[must_use]
    pub fn selected_uid_validity(&self) -> Option<u32> {
        self.selected_validity
    }

    /// Alertes du serveur (`[ALERT]`) reçues depuis le dernier relevé : quota
    /// presque plein, arrêt programmé… À montrer telles quelles à l'utilisateur.
    pub fn take_alerts(&mut self) -> Vec<String> {
        std::mem::take(&mut self.alerts)
    }

    /// Garde le texte d'une réponse `[ALERT]` : dix au plus en attente, 300
    /// caractères chacune, sans caractère de contrôle.
    fn note_alert(&mut self, response: &Response) {
        const MAX_ALERTS: usize = 10;
        const MAX_CHARS: usize = 300;
        if let Response::Status { code: Some(code), text, .. } = response {
            if code.eq_ignore_ascii_case("ALERT") && self.alerts.len() < MAX_ALERTS {
                let clean: String = text.chars().filter(|c| !c.is_control()).take(MAX_CHARS).collect();
                if !clean.trim().is_empty() && !self.alerts.contains(&clean) {
                    self.alerts.push(clean);
                }
            }
        }
    }

    fn next_tag(&mut self) -> String {
        // Chaque commande a son étiquette : ses plafonds de lecture repartent de zéro.
        self.conn.begin_command();
        self.tag += 1;
        format!("F{:04}", self.tag)
    }

    fn command(&mut self, parts: &[Part<'_>]) -> Result<Outcome> {
        let tag = self.next_tag();
        let mut line = format!("{tag} ").into_bytes();
        let mut data = Vec::new();
        for part in parts {
            match part {
                Part::Text(t) => line.extend_from_slice(t.as_bytes()),
                Part::Literal(bytes) => {
                    line.extend_from_slice(format!("{{{}}}\r\n", bytes.len()).as_bytes());
                    self.conn.write_all(&line)?;
                    self.conn.flush()?;
                    line.clear();
                    self.wait_continuation(&tag, &mut data)?;
                    self.conn.write_all(bytes)?;
                }
            }
        }
        line.extend_from_slice(b"\r\n");
        self.conn.write_all(&line)?;
        self.conn.flush()?;
        self.collect(&tag, data)
    }

    fn wait_continuation(&mut self, tag: &str, data: &mut Vec<Response>) -> Result<()> {
        loop {
            let raw = self.conn.read_response()?;
            let response = parse_response(&raw)?;
            self.note_alert(&response);
            match response {
                Response::Continuation(_) => return Ok(()),
                Response::Status { tag: Some(t), kind, text, .. } if t == tag => {
                    return Err(match kind {
                        StatusKind::No => Error::No(text),
                        _ => Error::Bad(text),
                    })
                }
                Response::Status { tag: None, kind: StatusKind::Bye, text, .. } => return Err(Error::Bye(text)),
                other => data.push(other),
            }
        }
    }

    fn collect(&mut self, tag: &str, mut data: Vec<Response>) -> Result<Outcome> {
        let mut bye: Option<String> = None;
        loop {
            let raw = match self.conn.read_response() {
                Ok(raw) => raw,
                Err(e) => return Err(bye.map_or(e, Error::Bye)),
            };
            let response = parse_response(&raw)?;
            self.note_alert(&response);
            match response {
                Response::Status { tag: Some(t), kind, code, text } if t == tag => {
                    return match kind {
                        StatusKind::Ok => Ok(Outcome { data, code, text }),
                        StatusKind::No => Err(Error::No(text)),
                        _ => Err(Error::Bad(text)),
                    };
                }
                Response::Status { tag: Some(_), .. } => {} // réponse orpheline : ignorée
                Response::Status { tag: None, kind: StatusKind::Bye, text, code } => {
                    bye = Some(text.clone());
                    data.push(Response::Status { tag: None, kind: StatusKind::Bye, code, text });
                }
                Response::Continuation(t) => return Err(Error::Protocol(format!("continuation inattendue : {t}"))),
                other => data.push(other),
            }
        }
    }

    /// Commande simple, sans littéral.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn run(&mut self, command: &str) -> Result<Outcome> {
        self.command(&[Part::Text(command.to_string())])
    }

    /// Relit les capacités du serveur (`CAPABILITY`), après une connexion.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn refresh_capabilities(&mut self) -> Result<()> {
        let out = self.run("CAPABILITY")?;
        for r in out.data {
            if let Response::Data(tokens) = r {
                if tokens.first().is_some_and(|t| t.is_atom("CAPABILITY")) {
                    self.capabilities = tokens[1..].iter().filter_map(Token::text).map(|c| c.to_ascii_uppercase()).collect();
                }
            }
        }
        Ok(())
    }

    /// Lance STARTTLS et rend le flux brut, à envelopper dans TLS puis à
    /// reprendre avec [`Client::resume`].
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn starttls(mut self) -> Result<S> {
        self.run("STARTTLS")?;
        Ok(self.into_inner())
    }

    fn after_auth(&mut self, out: Outcome) -> Result<()> {
        match out.code.filter(|c| c.to_ascii_uppercase().starts_with("CAPABILITY ")) {
            Some(code) => self.capabilities = parse_capabilities(&code),
            None => self.refresh_capabilities()?,
        }
        Ok(())
    }

    /// Authentification « mot de passe normal » (`LOGIN`).
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    /// Un identifiant ou un mot de passe refusé donne [`Error::No`].
    pub fn login(&mut self, user: &str, password: &str) -> Result<()> {
        let out = self.command(&[Part::Text("LOGIN ".into()), astring_part(user), Part::Text(" ".into()), astring_part(password)])?;
        self.after_auth(out)
    }

    /// Authentification `OAuth2` (SASL XOAUTH2), pour Outlook.com et Gmail.
    /// En cas de refus, le serveur envoie le détail en continuation : on
    /// l'acquitte par une ligne vide, puis il répond `NO`.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    /// Un jeton refusé donne [`Error::No`].
    pub fn authenticate_xoauth2(&mut self, user: &str, access_token: &str) -> Result<()> {
        let tag = self.next_tag();
        let ir = base64::encode(format!("user={user}\x01auth=Bearer {access_token}\x01\x01").as_bytes());
        let mut data = Vec::new();
        if self.has_capability("SASL-IR") {
            self.conn.write_all(format!("{tag} AUTHENTICATE XOAUTH2 {ir}\r\n").as_bytes())?;
            self.conn.flush()?;
        } else {
            self.conn.write_all(format!("{tag} AUTHENTICATE XOAUTH2\r\n").as_bytes())?;
            self.conn.flush()?;
            self.wait_continuation(&tag, &mut data)?;
            self.conn.write_all(format!("{ir}\r\n").as_bytes())?;
            self.conn.flush()?;
        }
        loop {
            let raw = self.conn.read_response()?;
            match parse_response(&raw)? {
                Response::Continuation(_) => {
                    self.conn.write_all(b"\r\n")?;
                    self.conn.flush()?;
                }
                Response::Status { tag: Some(t), kind, code, text } if t == tag => {
                    return match kind {
                        StatusKind::Ok => self.after_auth(Outcome { data, code, text }),
                        StatusKind::No => Err(Error::No(text)),
                        _ => Err(Error::Bad(text)),
                    };
                }
                Response::Status { tag: None, kind: StatusKind::Bye, text, .. } => return Err(Error::Bye(text)),
                other => data.push(other),
            }
        }
    }

    /// Authentification SASL PLAIN.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    /// Un identifiant ou un mot de passe refusé donne [`Error::No`].
    pub fn authenticate_plain(&mut self, user: &str, password: &str) -> Result<()> {
        let tag = self.next_tag();
        self.conn.write_all(format!("{tag} AUTHENTICATE PLAIN\r\n").as_bytes())?;
        self.conn.flush()?;
        let mut data = Vec::new();
        self.wait_continuation(&tag, &mut data)?;
        let token = base64::encode(format!("\0{user}\0{password}").as_bytes());
        self.conn.write_all(format!("{token}\r\n").as_bytes())?;
        self.conn.flush()?;
        let out = self.collect(&tag, data)?;
        self.after_auth(out)
    }

    /// Tous les dossiers du compte (`LIST "" "*"`).
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn list(&mut self) -> Result<Vec<ListEntry>> {
        self.list_command("LIST")
    }

    /// Dossiers auxquels le compte est abonné (ce que Thunderbird affiche par défaut).
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn lsub(&mut self) -> Result<Vec<ListEntry>> {
        self.list_command("LSUB")
    }

    fn list_command(&mut self, verb: &str) -> Result<Vec<ListEntry>> {
        let out = self.run(&format!("{verb} \"\" \"*\""))?;
        let mut entries = Vec::new();
        for r in out.data {
            let Response::Data(t) = r else { continue };
            if t.len() < 4 || !t[0].is_atom(verb) {
                continue;
            }
            let attributes = t[1].list().unwrap_or(&[]).iter().filter_map(Token::text).collect();
            let delimiter = t[2].text().and_then(|d| d.chars().next());
            let Some(name) = t[3].text() else { continue };
            entries.push(ListEntry { name, delimiter, attributes });
        }
        Ok(entries)
    }

    /// Séparateur hiérarchique du serveur (`LIST "" ""`), `/` à défaut.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn delimiter(&mut self) -> Result<char> {
        let out = self.run("LIST \"\" \"\"")?;
        for r in out.data {
            let Response::Data(t) = r else { continue };
            if t.len() >= 3 && t[0].is_atom("LIST") {
                if let Some(d) = t[2].text().and_then(|d| d.chars().next()) {
                    return Ok(d);
                }
            }
        }
        Ok('/')
    }

    /// Crée un dossier.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn create(&mut self, mailbox: &str) -> Result<()> {
        self.run(&format!("CREATE {}", quote(mailbox)))?;
        Ok(())
    }

    /// Supprime un dossier.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn delete(&mut self, mailbox: &str) -> Result<()> {
        self.run(&format!("DELETE {}", quote(mailbox)))?;
        Ok(())
    }

    /// Renomme (ou déplace dans l'arborescence) un dossier ; ses sous-dossiers suivent.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.run(&format!("RENAME {} {}", quote(from), quote(to)))?;
        if self.selected.as_deref() == Some(from) {
            self.selected = None;
            self.selected_validity = None;
        }
        Ok(())
    }

    /// Abonne le compte à un dossier.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn subscribe(&mut self, mailbox: &str) -> Result<()> {
        self.run(&format!("SUBSCRIBE {}", quote(mailbox)))?;
        Ok(())
    }

    /// Désabonne le compte d'un dossier.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn unsubscribe(&mut self, mailbox: &str) -> Result<()> {
        self.run(&format!("UNSUBSCRIBE {}", quote(mailbox)))?;
        Ok(())
    }

    /// Vide le dossier : sélection, tout marquer `\Deleted`, puis `EXPUNGE`.
    /// Renvoie le nombre de messages supprimés.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn empty_mailbox(&mut self, mailbox: &str) -> Result<u32> {
        let info = self.select(mailbox)?;
        if info.exists > 0 {
            self.run("STORE 1:* +FLAGS.SILENT (\\Deleted)")?;
            self.run("EXPUNGE")?;
        }
        Ok(info.exists)
    }

    /// Compteurs d'un dossier (`STATUS`), sans le sélectionner.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn status(&mut self, mailbox: &str) -> Result<MailboxStatus> {
        let out = self.run(&format!("STATUS {} (MESSAGES UNSEEN UIDNEXT UIDVALIDITY)", quote(mailbox)))?;
        Ok(out.data.iter().find_map(parse_status).map_or_default(|(_, st)| st))
    }

    /// `STATUS` de plusieurs dossiers en rafale : toutes les commandes partent
    /// d'un coup, puis les réponses sont lues (un aller-retour au lieu d'un par
    /// dossier). `None` : dossier refusé par le serveur.
    ///
    /// # Errors
    ///
    /// Connexion perdue ou réponse illisible ; un dossier refusé donne `None`
    /// à sa place, pas une erreur.
    pub fn status_many(&mut self, mailboxes: &[String]) -> Result<Vec<Option<MailboxStatus>>> {
        if mailboxes.is_empty() {
            return Ok(Vec::new());
        }
        let mut tags = Vec::with_capacity(mailboxes.len());
        let mut batch = Vec::new();
        for mailbox in mailboxes {
            let tag = self.next_tag();
            batch.extend_from_slice(format!("{tag} STATUS {} (MESSAGES UNSEEN UIDNEXT UIDVALIDITY)\r\n", quote(mailbox)).as_bytes());
            tags.push(tag);
        }
        self.conn.write_all(&batch)?;
        self.conn.flush()?;

        let mut found: std::collections::HashMap<String, MailboxStatus> = std::collections::HashMap::new();
        let mut refused = std::collections::HashSet::new();
        let mut pending = tags.len();
        while pending > 0 {
            let raw = self.conn.read_response()?;
            match parse_response(&raw)? {
                Response::Status { tag: Some(t), kind, .. } if tags.contains(&t) => {
                    pending -= 1;
                    if !matches!(kind, StatusKind::Ok) {
                        refused.insert(t);
                    }
                }
                Response::Status { tag: None, kind: StatusKind::Bye, text, .. } => return Err(Error::Bye(text)),
                Response::Data(t) => {
                    if let Some((name, st)) = parse_status(&Response::Data(t)) {
                        found.insert(name, st);
                    }
                }
                _ => {}
            }
        }
        Ok(mailboxes
            .iter()
            .zip(&tags)
            .map(|(m, tag)| if refused.contains(tag) { None } else { Some(found.remove(m).unwrap_or_default()) })
            .collect())
    }

    /// Sélectionne un dossier (`SELECT`) et rend ses compteurs.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn select(&mut self, mailbox: &str) -> Result<SelectInfo> {
        self.selected = None;
        self.selected_validity = None;
        let out = self.run(&format!("SELECT {}", quote(mailbox)))?;
        let mut info = SelectInfo::default();
        for r in &out.data {
            match r {
                Response::Data(t) if t.len() >= 2 && t[1].is_atom("EXISTS") => {
                    info.exists = t[0].number().and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
                }
                Response::Status { tag: None, code, .. } => {
                    if let Some(v) = code_number(code.as_deref(), "UIDVALIDITY") {
                        info.uid_validity = Some(v);
                    }
                    if let Some(v) = code_number(code.as_deref(), "UIDNEXT") {
                        info.uid_next = Some(v);
                    }
                    if let Some(v) = code_number(code.as_deref(), "HIGHESTMODSEQ") {
                        info.highest_modseq = Some(v);
                    }
                    if code.as_deref().is_some_and(|c| c.trim().eq_ignore_ascii_case("NOMODSEQ")) {
                        info.no_modseq = true;
                    }
                }
                _ => {}
            }
        }
        info.read_only = out.code.as_deref().is_some_and(|c| c.eq_ignore_ascii_case("READ-ONLY"));
        self.selected = Some(mailbox.to_string());
        self.selected_validity = info.uid_validity;
        self.has_selected = true;
        Ok(info)
    }

    fn fetch_items(out: Outcome) -> Vec<FetchItem> {
        let mut items = Vec::new();
        for r in out.data {
            let Response::Data(t) = r else { continue };
            if t.len() < 3 || !t[1].is_atom("FETCH") {
                continue;
            }
            let Some(seq) = t[0].number().and_then(|n| u32::try_from(n).ok()) else { continue };
            let Some(list) = t[2].list() else { continue };
            let pairs = list.chunks(2).filter_map(|p| Some((p[0].text()?.to_ascii_uppercase(), p.get(1)?.clone()))).collect();
            items.push(FetchItem { seq, items: pairs });
        }
        items
    }

    /// `FETCH <séquence> <requête>`, ex. `fetch("1:*", "(UID FLAGS)")`.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn fetch(&mut self, sequence: &str, query: &str) -> Result<Vec<FetchItem>> {
        Ok(Self::fetch_items(self.run(&format!("FETCH {sequence} {query}"))?))
    }

    /// `UID FETCH <uids> <requête>` ; ne garde que les réponses portant un UID.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn uid_fetch(&mut self, uids: &[u32], query: &str) -> Result<Vec<FetchItem>> {
        let mut items = Vec::new();
        for chunk in uids.chunks(UIDS_PER_COMMAND) {
            items.extend(self.uid_fetch_set(&uid_set(chunk), query)?);
        }
        Ok(items)
    }

    /// `UID FETCH` avec un ensemble libre, ex. `"120:*"` (tout à partir de l'UID 120).
    /// Attention : `n:*` renvoie au moins le dernier message même si son UID est < n.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn uid_fetch_set(&mut self, set: &str, query: &str) -> Result<Vec<FetchItem>> {
        let items = Self::fetch_items(self.run(&format!("UID FETCH {set} {query}"))?);
        Ok(items.into_iter().filter(|i| i.uid().is_some()).collect())
    }

    /// `UID FETCH <set> <requête> (CHANGEDSINCE <modseq>)` (RFC 7162) : seuls
    /// les messages modifiés depuis `modseq`. Avec QRESYNC activé, ajoute
    /// `VANISHED` : les UID supprimés depuis arrivent dans [`Changes::vanished`].
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn uid_fetch_changed(&mut self, set: &str, query: &str, modseq: u64) -> Result<Changes> {
        let vanished = if self.qresync() { " VANISHED" } else { "" };
        let out = self.run(&format!("UID FETCH {set} {query} (CHANGEDSINCE {modseq}{vanished})"))?;
        let vanished = vanished_uids(&out.data);
        let items = Self::fetch_items(out).into_iter().filter(|i| i.uid().is_some()).collect();
        Ok(Changes { items, vanished })
    }

    /// Ex. `uid_store(&[3], "+FLAGS.SILENT (\\Seen)")`.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn uid_store(&mut self, uids: &[u32], operation: &str) -> Result<()> {
        for chunk in uids.chunks(UIDS_PER_COMMAND) {
            self.run(&format!("UID STORE {} {operation}", uid_set(chunk)))?;
        }
        Ok(())
    }

    /// Supprime définitivement des messages marqués `\Deleted`.
    ///
    /// Sans UIDPLUS, il n'existe que `EXPUNGE`, qui purge **tous** les messages
    /// marqués du dossier, y compris ceux qu'un autre logiciel a seulement
    /// marqués comme supprimés (relecture Y5). Ceux-là sont protégés : leur
    /// marque est retirée le temps de la purge, puis remise. Si une étape
    /// échoue, rien n'est perdu : au pire un message reste marqué, ou un
    /// message d'un autre logiciel perd sa marque mais reste sur le serveur.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn uid_expunge(&mut self, uids: &[u32]) -> Result<()> {
        if self.has_capability("UIDPLUS") {
            for chunk in uids.chunks(UIDS_PER_COMMAND) {
                self.run(&format!("UID EXPUNGE {}", uid_set(chunk)))?;
            }
            return Ok(());
        }
        let ours: std::collections::HashSet<u32> = uids.iter().copied().collect();
        let others: Vec<u32> = self.uid_search_raw("DELETED")?.into_iter().filter(|u| !ours.contains(u)).collect();
        if !others.is_empty() {
            self.uid_store(&others, "-FLAGS.SILENT (\\Deleted)")?;
        }
        let purged = self.run("EXPUNGE").map(|_| ());
        if !others.is_empty() {
            self.uid_store(&others, "+FLAGS.SILENT (\\Deleted)")?;
        }
        purged
    }

    /// Déplacement dans le même compte : `UID MOVE` si disponible,
    /// sinon `UID COPY` + `\Deleted` + expunge.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn uid_move(&mut self, uids: &[u32], mailbox: &str) -> Result<()> {
        let verb = if self.has_capability("MOVE") { "MOVE" } else { "COPY" };
        for chunk in uids.chunks(UIDS_PER_COMMAND) {
            self.run(&format!("UID {verb} {} {}", uid_set(chunk), quote(mailbox)))?;
        }
        if verb == "COPY" {
            self.uid_store(uids, "+FLAGS.SILENT (\\Deleted)")?;
            self.uid_expunge(uids)?;
        }
        Ok(())
    }

    /// Dépose un message ; renvoie son UID si le serveur le communique (UIDPLUS).
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn append(&mut self, mailbox: &str, flags: &[String], internal_date: Option<i64>, content: &[u8]) -> Result<Option<u32>> {
        let mut head = format!("APPEND {}", quote(mailbox));
        // \Recent ne peut pas être posé par le client.
        let flags: Vec<&str> = flags.iter().map(String::as_str).filter(|f| !f.eq_ignore_ascii_case("\\Recent")).collect();
        if !flags.is_empty() {
            let _ = write!(head, " ({})", flags.join(" "));
        }
        if let Some(ts) = internal_date {
            let _ = write!(head, " \"{}\"", date::format_internal_date(ts));
        }
        head.push(' ');
        let out = self.command(&[Part::Text(head), Part::Literal(content)])?;
        Ok(out.code.as_deref().and_then(|c| {
            let mut w = c.split_whitespace();
            (w.next()?.eq_ignore_ascii_case("APPENDUID")).then(|| w.nth(1)?.parse().ok()).flatten()
        }))
    }

    /// `UID SEARCH <critère> <valeur>` dans le dossier sélectionné, ex.
    /// `uid_search("TEXT", "facture")` ou `uid_search("HEADER Message-ID", "<id@x>")`.
    /// Une valeur non ASCII part en UTF-8 (`CHARSET UTF-8` et littéral).
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn uid_search(&mut self, criterion: &str, value: &str) -> Result<Vec<u32>> {
        let out = if needs_literal(value) {
            self.command(&[Part::Text(format!("UID SEARCH CHARSET UTF-8 {criterion} ")), Part::Literal(value.as_bytes())])?
        } else {
            self.run(&format!("UID SEARCH {criterion} {}", quote(value)))?
        };
        Ok(search_results(&out.data))
    }

    /// `UID SEARCH` avec plusieurs critères, tous vrais (ET) ; sans critère, `ALL`.
    /// Un texte non ASCII part en littéral, la commande portant `CHARSET UTF-8`.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn uid_search_keys(&mut self, keys: &[SearchKey]) -> Result<Vec<u32>> {
        let mut line = String::from("UID SEARCH ");
        if keys.iter().any(SearchKey::needs_utf8) {
            line.push_str("CHARSET UTF-8 ");
        }
        let mut parts = Vec::new();
        if keys.is_empty() {
            line.push_str("ALL");
        }
        for (i, key) in keys.iter().enumerate() {
            if i > 0 {
                line.push(' ');
            }
            key.write(&mut line, &mut parts);
        }
        parts.push(Part::Text(line));
        let out = self.command(&parts)?;
        Ok(search_results(&out.data))
    }

    /// `UID SEARCH <critères>` brut (critères ASCII uniquement), ex. `"UNSEEN"`.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn uid_search_raw(&mut self, criteria: &str) -> Result<Vec<u32>> {
        let out = self.run(&format!("UID SEARCH {criteria}"))?;
        Ok(search_results(&out.data))
    }

    /// Écoute IMAP IDLE (RFC 2177) sur le dossier sélectionné.
    ///
    /// Rend la main dès que le serveur signale un changement (`EXISTS`,
    /// `EXPUNGE`, `FETCH`) — résultat `true` —, quand `max` est écoulé ou que
    /// `keep_going()` devient faux — résultat `false`. La session redevient
    /// utilisable normalement ensuite.
    ///
    /// Le flux doit avoir un délai de lecture (`set_read_timeout`) : il fixe la
    /// fréquence à laquelle `keep_going` et `max` sont vérifiés.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn idle(&mut self, max: Duration, keep_going: impl Fn() -> bool) -> Result<bool> {
        let tag = self.next_tag();
        self.conn.write_all(format!("{tag} IDLE\r\n").as_bytes())?;
        self.conn.flush()?;

        let mut changed = false;
        let mut data = Vec::new();
        self.wait_continuation(&tag, &mut data)?;
        if data.iter().any(is_change) {
            changed = true;
        }

        let start = Instant::now();
        while !changed && keep_going() && start.elapsed() < max {
            match self.conn.read_response() {
                Ok(raw) => match parse_response(&raw)? {
                    Response::Status { tag: None, kind: StatusKind::Bye, text, .. } => return Err(Error::Bye(text)),
                    other => changed = is_change(&other),
                },
                // Délai de lecture écoulé sans nouvelle : on revérifie les conditions.
                Err(Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) => return Err(e),
            }
        }

        self.conn.write_all(b"DONE\r\n")?;
        self.conn.flush()?;
        let out = self.collect(&tag, Vec::new())?;
        Ok(changed || out.data.iter().any(is_change))
    }

    /// `NOOP` : garde la session ouverte et relève les nouveautés.
    ///
    /// # Errors
    ///
    /// Connexion perdue ([`Error::Io`], [`Error::Bye`]), commande refusée par le
    /// serveur ([`Error::No`], [`Error::Bad`]) ou réponse illisible ([`Error::Protocol`]).
    pub fn noop(&mut self) -> Result<()> {
        self.run("NOOP").map(|_| ())
    }

    /// Ferme la session (`LOGOUT`) ; un `BYE` du serveur n'est pas une erreur.
    ///
    /// # Errors
    ///
    /// Connexion perdue avant la fin, ou réponse illisible.
    pub fn logout(mut self) -> Result<()> {
        match self.run("LOGOUT") {
            Ok(_) | Err(Error::Bye(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// UID trouvés dans les réponses `* SEARCH 1 2 3`, triés.
fn search_results(data: &[Response]) -> Vec<u32> {
    let mut uids: Vec<u32> = data
        .iter()
        .filter_map(|r| match r {
            Response::Data(t) if t.first().is_some_and(|x| x.is_atom("SEARCH")) => Some(t),
            _ => None,
        })
        .flat_map(|t| t[1..].iter().filter_map(|x| x.number().and_then(|n| u32::try_from(n).ok())))
        .collect();
    uids.sort_unstable();
    uids.dedup();
    uids
}

/// Réponse non étiquetée signalant un changement dans le dossier.
fn is_change(r: &Response) -> bool {
    match r {
        Response::Data(t) => {
            (t.len() >= 2 && ["EXISTS", "EXPUNGE", "FETCH", "RECENT"].iter().any(|k| t[1].is_atom(k)))
                || t.first().is_some_and(|x| x.is_atom("VANISHED"))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flux de test : réponses du serveur jouées dans l'ordre, commandes enregistrées.
    struct Script {
        input: std::io::Cursor<Vec<u8>>,
        sent: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    }
    impl Read for Script {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(b)
        }
    }
    impl Write for Script {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.sent.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn scripted(server: &str) -> (Client<Script>, std::rc::Rc<std::cell::RefCell<Vec<u8>>>) {
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let stream = Script { input: std::io::Cursor::new(server.as_bytes().to_vec()), sent: std::rc::Rc::clone(&sent) };
        (Client::resume(stream), sent)
    }

    #[test]
    fn alertes_du_serveur() {
        // Relecture complète n° 22 : étiquetée ou non, OK ou NO, gardées une fois.
        let (mut c, _) = scripted(
            "* OK [ALERT] Quota presque atteint\r\nF0001 OK [ALERT] Quota presque atteint\r\n\
             * NO [alert] Maintenance\u{7} ce soir\r\nF0002 NO [ALERT] Refus\r\n",
        );
        c.run("NOOP").unwrap();
        assert!(c.run("NOOP").is_err(), "refus du serveur");
        assert_eq!(c.take_alerts(), vec!["Quota presque atteint", "Maintenance ce soir", "Refus"]);
        assert!(c.take_alerts().is_empty(), "relevées une seule fois");
    }

    #[test]
    fn p4_grands_ensembles_d_uid_decoupes() {
        // 1 200 UID non contigus : trois commandes de 500 au plus.
        let uids: Vec<u32> = (1..=1200).map(|n| n * 2).collect();
        let (mut c, sent) = scripted("F0001 OK done\r\nF0002 OK done\r\nF0003 OK done\r\n");
        c.uid_store(&uids, "+FLAGS.SILENT (\\Seen)").unwrap();
        let sent = String::from_utf8(sent.borrow().clone()).unwrap();
        let lines: Vec<&str> = sent.lines().collect();
        assert_eq!(lines.len(), 3, "trois commandes");
        assert!(lines[0].starts_with("F0001 UID STORE 2,4,6,") && lines[0].contains(",1000 +FLAGS"), "{}", lines[0]);
        assert!(lines.iter().all(|l| l.len() < 6_000), "lignes bornées");
        assert!(lines[2].contains(",2400 +FLAGS"), "{}", lines[2]);
    }

    #[test]
    fn search_ascii_and_utf8() {
        let (mut c, sent) = scripted("* SEARCH 12 3 7\r\nF0001 OK done\r\n");
        assert_eq!(c.uid_search("TEXT", "facture").unwrap(), vec![3, 7, 12]);
        assert_eq!(String::from_utf8_lossy(&sent.borrow()), "F0001 UID SEARCH TEXT \"facture\"\r\n");

        let (mut c, sent) = scripted("+ go\r\n* SEARCH\r\nF0001 OK done\r\n");
        assert_eq!(c.uid_search("TEXT", "été").unwrap(), Vec::<u32>::new());
        let sent = String::from_utf8(sent.borrow().clone()).unwrap();
        assert!(sent.starts_with("F0001 UID SEARCH CHARSET UTF-8 TEXT {5}\r\nété"), "{sent}");
    }

    #[test]
    fn e7_plusieurs_criteres() {
        let (mut c, sent) = scripted("* SEARCH 4 2\r\nF0001 OK done\r\n");
        let keys = [
            SearchKey::Text("FROM", "marie".into()),
            SearchKey::Or(Box::new(SearchKey::Text("TO", "paul".into())), Box::new(SearchKey::Text("CC", "paul".into()))),
            SearchKey::Date("SINCE", "1-Nov-2026".into()),
            SearchKey::Not(Box::new(SearchKey::Text("TEXT", "brouillon".into()))),
            SearchKey::Flag("UNSEEN"),
        ];
        assert_eq!(c.uid_search_keys(&keys).unwrap(), vec![2, 4]);
        assert_eq!(
            String::from_utf8_lossy(&sent.borrow()),
            "F0001 UID SEARCH FROM \"marie\" OR TO \"paul\" CC \"paul\" SINCE 1-Nov-2026 NOT TEXT \"brouillon\" UNSEEN\r\n"
        );

        // Texte accentué au milieu : littéral, puis la suite de la commande.
        let (mut c, sent) = scripted("+ go\r\n* SEARCH\r\nF0001 OK done\r\n");
        let keys = [SearchKey::Text("SUBJECT", "réunion".into()), SearchKey::Flag("FLAGGED")];
        assert_eq!(c.uid_search_keys(&keys).unwrap(), Vec::<u32>::new());
        let sent = String::from_utf8(sent.borrow().clone()).unwrap();
        assert_eq!(sent, "F0001 UID SEARCH CHARSET UTF-8 SUBJECT {8}\r\nréunion FLAGGED\r\n");

        // Sans critère : tout ; un mot-clé ou une date piégés ne passent pas.
        let (mut c, sent) = scripted("* SEARCH\r\nF0001 OK done\r\n");
        c.uid_search_keys(&[SearchKey::Date("BEFORE", "1-Nov-2026\r\nX LOGOUT".into())]).unwrap();
        assert_eq!(String::from_utf8_lossy(&sent.borrow()), "F0001 UID SEARCH BEFORE 1-Nov-2026XLOGOUT\r\n");
        let (mut c, sent) = scripted("* SEARCH\r\nF0001 OK done\r\n");
        c.uid_search_keys(&[]).unwrap();
        assert_eq!(String::from_utf8_lossy(&sent.borrow()), "F0001 UID SEARCH ALL\r\n");
    }

    #[test]
    fn status_in_one_round_trip() {
        let (mut c, sent) = scripted(
            "* STATUS INBOX (MESSAGES 12 UNSEEN 2 UIDNEXT 40 UIDVALIDITY 7)\r\n\
             F0001 OK done\r\n\
             F0002 NO no such mailbox\r\n\
             * STATUS \"Sent Items\" (MESSAGES 3 UNSEEN 0)\r\n\
             F0003 OK done\r\n",
        );
        let names = vec!["INBOX".to_string(), "Gone".to_string(), "Sent Items".to_string()];
        let out = c.status_many(&names).unwrap();
        assert_eq!(out[0].as_ref().map(|s| (s.messages, s.unseen, s.uid_next)), Some((12, 2, Some(40))));
        assert_eq!(out[1], None);
        assert_eq!(out[2].as_ref().map(|s| s.messages), Some(3));
        let sent = String::from_utf8(sent.borrow().clone()).unwrap();
        assert_eq!(sent.lines().count(), 3);
        assert!(sent.starts_with("F0001 STATUS \"INBOX\""), "{sent}");
        // Les commandes suivantes reprennent la numérotation.
        assert_eq!(c.next_tag(), "F0004");
    }

    #[test]
    fn uid_sets() {
        assert_eq!(uid_set(&[7, 1, 2, 3, 9, 10]), "1:3,7,9:10");
        assert_eq!(uid_set(&[5]), "5");
        assert_eq!(uid_set(&[]), "");
    }

    #[test]
    fn quoting() {
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(quote("Dossier\r\nA1 DELETE INBOX"), "\"DossierA1 DELETE INBOX\"");
        assert!(needs_literal("mot de passé"));
        assert!(!needs_literal("p@ss w0rd!"));
    }

    #[test]
    fn fetch_sections() {
        let item = FetchItem {
            seq: 1,
            items: vec![
                ("BODY[HEADER.FIELDS (FROM)]".into(), Token::Str(b"From: a".to_vec())),
                ("BODY[TEXT]<0>".into(), Token::Str(b"partiel".to_vec())),
            ],
        };
        assert_eq!(item.body("BODY[HEADER"), Some(&b"From: a"[..]));
        assert_eq!(item.body("BODY[TEXT]"), Some(&b"partiel"[..]));
        assert_eq!(item.body("BODY[]"), None);
    }

    #[test]
    fn select_reports_highest_modseq() {
        let (mut c, _) = scripted(
            "* 3 EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n* OK [UIDNEXT 40] ok\r\n\
             * OK [HIGHESTMODSEQ 715194045007] ok\r\nF0001 OK [READ-WRITE] done\r\n",
        );
        let info = c.select("INBOX").unwrap();
        assert_eq!((info.exists, info.uid_validity, info.uid_next), (3, Some(7), Some(40)));
        assert_eq!(info.highest_modseq, Some(715_194_045_007));
        assert!(!info.no_modseq);

        let (mut c, _) = scripted("* 0 EXISTS\r\n* OK [NOMODSEQ] pas de modseq\r\nF0001 OK done\r\n");
        let info = c.select("Archives").unwrap();
        assert_eq!(info.highest_modseq, None);
        assert!(info.no_modseq);
    }

    #[test]
    fn uid_set_parsing() {
        let set = UidSet::parse("1:5,9,12:10").unwrap();
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![1, 2, 3, 4, 5, 9, 10, 11, 12]);
        assert!(set.contains(4) && set.contains(11) && !set.contains(6));
        // Un grand intervalle n'est pas développé en mémoire.
        assert!(UidSet::parse("1:4000000000").unwrap().contains(3_999_999_999));
        assert_eq!(UidSet::parse("7").unwrap().iter().collect::<Vec<_>>(), vec![7]);
        assert!(UidSet::parse("1:*").is_none());
        assert!(UidSet::parse("").is_none());
    }

    #[test]
    fn fetch_modseq() {
        let item = FetchItem {
            seq: 1,
            items: vec![("UID".into(), Token::Atom("4".into())), ("MODSEQ".into(), Token::List(vec![Token::Atom("624140003".into())]))],
        };
        assert_eq!(item.modseq(), Some(624_140_003));
    }

    #[test]
    fn changed_since_with_vanished() {
        let (mut c, sent) = scripted(
            "* ENABLED QRESYNC\r\nF0001 OK enabled\r\n\
             * VANISHED (EARLIER) 1:5,9\r\n\
             * 3 FETCH (UID 12 FLAGS (\\Seen) MODSEQ (90))\r\n\
             * VANISHED 14\r\n\
             F0002 OK done\r\n",
        );
        assert_eq!(c.enable(&["QRESYNC"]).unwrap(), vec!["QRESYNC".to_string()]);
        assert!(c.qresync());
        let changes = c.uid_fetch_changed("1:*", "(UID FLAGS)", 80).unwrap();
        assert_eq!(changes.items.len(), 1);
        assert_eq!((changes.items[0].uid(), changes.items[0].modseq()), (Some(12), Some(90)));
        assert!(changes.items[0].has_flag("\\Seen"));
        assert_eq!(changes.vanished.iter().collect::<Vec<_>>(), vec![1, 2, 3, 4, 5, 9, 14]);
        let sent = String::from_utf8(sent.borrow().clone()).unwrap();
        assert_eq!(sent, "F0001 ENABLE QRESYNC\r\nF0002 UID FETCH 1:* (UID FLAGS) (CHANGEDSINCE 80 VANISHED)\r\n");
    }

    #[test]
    fn changed_since_without_qresync() {
        let (mut c, sent) = scripted("* 2 FETCH (UID 5 FLAGS () MODSEQ (12))\r\nF0001 OK done\r\n");
        let changes = c.uid_fetch_changed("3:*", "(UID FLAGS)", 10).unwrap();
        assert_eq!(changes.items.len(), 1);
        assert!(changes.vanished.is_empty());
        assert_eq!(String::from_utf8_lossy(&sent.borrow()), "F0001 UID FETCH 3:* (UID FLAGS) (CHANGEDSINCE 10)\r\n");
    }

    #[test]
    fn enable_only_when_advertised_and_before_select() {
        // Rien d'annoncé : aucune commande.
        let (mut c, sent) = scripted("");
        c.enable_sync_extensions().unwrap();
        assert!(sent.borrow().is_empty());

        // Annoncé : ENABLE une seule fois ; un refus n'est pas une erreur.
        let (mut c, sent) = scripted("F0001 BAD non\r\n");
        c.capabilities = vec!["ENABLE".into(), "CONDSTORE".into(), "QRESYNC".into()];
        c.enable_sync_extensions().unwrap();
        c.enable_sync_extensions().unwrap();
        assert_eq!(String::from_utf8_lossy(&sent.borrow()), "F0001 ENABLE CONDSTORE QRESYNC\r\n");
        assert!(!c.qresync() && c.condstore());

        // Dossier déjà sélectionné : ENABLE n'est plus permis.
        let (mut c, sent) = scripted("* 0 EXISTS\r\nF0001 OK done\r\n");
        c.capabilities = vec!["ENABLE".into(), "QRESYNC".into()];
        c.select("INBOX").unwrap();
        c.enable_sync_extensions().unwrap();
        assert_eq!(String::from_utf8_lossy(&sent.borrow()), "F0001 SELECT \"INBOX\"\r\n");
    }

    #[test]
    fn vanished_counts_as_a_change() {
        assert!(is_change(&parse_response(b"* VANISHED 3:4").unwrap()));
    }

    #[test]
    fn list_entry_names() {
        let e = ListEntry { name: "Clients/Soci&AOk-t&AOk-".into(), delimiter: Some('/'), attributes: vec!["\\HasNoChildren".into()] };
        assert_eq!(e.decoded_name(), "Clients/Société");
        assert_eq!(e.leaf_name(), "Société");
        assert_eq!(e.parent().as_deref(), Some("Clients"));
        assert!(e.selectable());
    }
}
