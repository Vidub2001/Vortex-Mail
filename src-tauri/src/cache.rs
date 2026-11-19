//! Cache local SQLite : arborescence, en-têtes des messages, corps déjà lus.
//!
//! L'interface lit d'abord ici (affichage immédiat, hors ligne) ; la
//! synchronisation avec les serveurs met ensuite le cache à jour.
//!
//! Le cache possède aussi le registre des identifiants de messages vus par
//! l'interface (`MessageId` ⇄ compte/dossier/UID), pour que les lectures du
//! cache n'aient jamais besoin du verrou des connexions réseau.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use mail_core::search::SearchQuery;
use mail_core::{Account, AttachmentInfo, Flags, Folder, FolderRole, Message, MessageId, MessageSummary};
use rusqlite::{params, Connection, OptionalExtension};

/// Les identifiants de messages commencent ici (au-delà de ceux de la démo).
const ID_BASE: MessageId = 1 << 40;

/// Durée pendant laquelle le dossier d'origine d'un message supprimé reste
/// noté (« Restaurer ») ; au-delà, il revient dans la boîte de réception.
pub(crate) const TRASH_ORIGIN_DAYS: i64 = 180;

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS folders (
    account   TEXT    NOT NULL,
    path      TEXT    NOT NULL,
    name      TEXT    NOT NULL,
    parent    TEXT,
    depth     INTEGER NOT NULL,
    role      TEXT    NOT NULL,
    position  INTEGER NOT NULL,
    unread    INTEGER NOT NULL,
    total     INTEGER NOT NULL,
    PRIMARY KEY (account, path)
);

CREATE TABLE IF NOT EXISTS folder_state (
    account       TEXT    NOT NULL,
    path          TEXT    NOT NULL,
    uidvalidity   INTEGER NOT NULL,
    highestmodseq INTEGER,
    exists_count  INTEGER,
    PRIMARY KEY (account, path)
);

CREATE TABLE IF NOT EXISTS messages (
    account     TEXT    NOT NULL,
    path        TEXT    NOT NULL,
    uid         INTEGER NOT NULL,
    from_name   TEXT    NOT NULL,
    from_email  TEXT    NOT NULL,
    subject     TEXT    NOT NULL,
    preview     TEXT    NOT NULL,
    date        INTEGER NOT NULL,
    seen        INTEGER NOT NULL,
    flagged     INTEGER NOT NULL,
    answered    INTEGER NOT NULL,
    attachments INTEGER NOT NULL,
    PRIMARY KEY (account, path, uid)
);
CREATE INDEX IF NOT EXISTS messages_by_date ON messages (account, path, date DESC);

CREATE TABLE IF NOT EXISTS bodies (
    account TEXT    NOT NULL,
    path    TEXT    NOT NULL,
    uid     INTEGER NOT NULL,
    to_addr TEXT    NOT NULL,
    html    TEXT    NOT NULL,
    PRIMARY KEY (account, path, uid)
);

-- Déplacements entre comptes en cours (reprise après un arrêt ou une coupure).
CREATE TABLE IF NOT EXISTS move_journal (
    key        INTEGER PRIMARY KEY,
    account    TEXT    NOT NULL,
    mailbox    TEXT    NOT NULL,
    uid        INTEGER NOT NULL,
    target     TEXT    NOT NULL,
    message_id TEXT    NOT NULL,
    copied     INTEGER NOT NULL
);

-- Actions faites hors connexion, rejouées au retour du réseau.
CREATE TABLE IF NOT EXISTS pending_actions (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    kind    TEXT    NOT NULL,
    account TEXT    NOT NULL,
    mailbox TEXT    NOT NULL,
    uids    TEXT    NOT NULL,
    arg     TEXT    NOT NULL,
    value   INTEGER NOT NULL
);

-- Copies de messages envoyés à déposer dans « Éléments envoyés » quand le
-- serveur IMAP ne répondait pas au moment de l'envoi (relecture complète n° 11).
CREATE TABLE IF NOT EXISTS pending_sent (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    account TEXT    NOT NULL,
    mailbox TEXT    NOT NULL,
    date    INTEGER NOT NULL,
    raw     BLOB    NOT NULL
);

-- Contacts et agenda : carnets / agendas de chaque compte, et leur contenu.
CREATE TABLE IF NOT EXISTS pim_collections (
    account TEXT NOT NULL,
    url     TEXT NOT NULL,
    kind    TEXT NOT NULL,
    name    TEXT NOT NULL,
    color   TEXT NOT NULL,
    PRIMARY KEY (account, url)
);

-- `kind` : contacts ou calendar ; `data` : JSON (Contact, ou liste d'Event).
CREATE TABLE IF NOT EXISTS pim_items (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    account    TEXT NOT NULL,
    collection TEXT NOT NULL,
    url        TEXT NOT NULL,
    etag       TEXT NOT NULL,
    raw        TEXT NOT NULL,
    data       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS pim_items_by_collection ON pim_items (account, collection);

-- Copies de secours des brouillons jamais arrivés sur le serveur (lot G4 :
-- dans le cache chiffré, plus dans un fichier ni dans le stockage de la fenêtre).
-- `key` : fenêtre de rédaction (`compose-…`) ou brouillon de la fenêtre principale.
CREATE TABLE IF NOT EXISTS draft_backups (
    key  TEXT PRIMARY KEY,
    data TEXT NOT NULL
);

-- Réglages qui nomment des correspondants (règles, signatures) : dans le
-- cache chiffré plutôt que dans le stockage de la fenêtre, en clair sur le
-- disque (audit S8, lot J12b).
CREATE TABLE IF NOT EXISTS prefs (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Adresses collectées pour la complétion (lot J13) : destinataires des
-- messages envoyés, par compte d'envoi (clé : adresse en minuscules ; retirées
-- avec le compte). `sent` compte les messages des dossiers « Éléments
-- envoyés » gardés dans le cache ; un envoi de Vortex Mail note l'adresse tout
-- de suite, sa copie dans « Envoyés » la comptera.
CREATE TABLE IF NOT EXISTS collected_addresses (
    account TEXT    NOT NULL,
    key     TEXT    NOT NULL,
    email   TEXT    NOT NULL,
    name    TEXT    NOT NULL,
    sent    INTEGER NOT NULL,
    last    INTEGER NOT NULL,
    PRIMARY KEY (account, key)
);
-- Adresses écartées de la complétion par l'utilisateur.
CREATE TABLE IF NOT EXISTS forgotten_addresses (
    key TEXT PRIMARY KEY
);

-- Réponses données aux invitations (lot H1), pour les montrer à la relecture :
-- par compte, rendez-vous (`uid`) et occurrence (`recurrence`, vide pour la série) ;
-- `sequence` : version de l'invitation à laquelle on a répondu.
CREATE TABLE IF NOT EXISTS invitation_answers (
    account    TEXT    NOT NULL,
    uid        TEXT    NOT NULL,
    recurrence TEXT    NOT NULL,
    answer     TEXT    NOT NULL,
    sequence   INTEGER NOT NULL,
    PRIMARY KEY (account, uid, recurrence)
);

-- Dossier d'origine des messages envoyés dans « Éléments supprimés » par
-- Vortex Mail, pour « Restaurer » : par compte et `Message-ID` (l'UID change
-- avec le dossier) ; un même `Message-ID` peut venir de plusieurs dossiers
-- (message envoyé à soi-même : « Éléments envoyés » et boîte de réception) ;
-- `deleted_at` en secondes, pour oublier les plus anciens.
CREATE TABLE IF NOT EXISTS trash_origins (
    account    TEXT    NOT NULL,
    message_id TEXT    NOT NULL,
    mailbox    TEXT    NOT NULL,
    deleted_at INTEGER NOT NULL,
    PRIMARY KEY (account, message_id, mailbox)
);

CREATE TABLE IF NOT EXISTS accounts (
    id       TEXT    PRIMARY KEY,
    name     TEXT    NOT NULL,
    email    TEXT    NOT NULL,
    color    TEXT    NOT NULL,
    position INTEGER NOT NULL
);
";

/// Déplacement inter-comptes enregistré au journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRow {
    pub key: u64,
    pub source: ImapRef,
    pub target: String,
    pub message_id: Option<String>,
    pub copied: bool,
}

/// Copie d'un message envoyé, à déposer dans « Éléments envoyés » au retour du
/// serveur IMAP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSent {
    /// Numéro de la ligne (0 avant l'enregistrement).
    pub id: i64,
    pub account: String,
    pub mailbox: String,
    /// Date de l'envoi (secondes Unix), gardée comme date de réception du dépôt.
    pub date: i64,
    /// Message tel qu'envoyé, sans `Bcc`.
    pub raw: Vec<u8>,
}

/// Action faite hors connexion, en attente du réseau.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAction {
    pub id: i64,
    /// `flag` (arg = `\Seen` ou `\Flagged`, value = posé ou retiré) ou `move` (arg = dossier cible).
    pub kind: String,
    pub account: String,
    pub mailbox: String,
    pub uids: Vec<u32>,
    pub arg: String,
    pub value: bool,
}

/// Carnet d'adresses ou agenda d'un compte (`account` = `local` : sur cet ordinateur).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PimCollection {
    pub account: String,
    pub url: String,
    pub kind: String,
    pub name: String,
    pub color: String,
}

/// Contact ou ressource d'agenda en cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PimItem {
    pub id: String,
    pub url: String,
    pub etag: String,
    /// vCard ou iCalendar d'origine.
    pub raw: String,
    /// JSON prêt pour l'interface.
    pub data: String,
}

/// Emplacement d'un message sur un serveur.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImapRef {
    pub account: String,
    pub mailbox: String,
    pub uid: u32,
}

/// Colonnes ajoutées à `bodies` après la première version (migration automatique).
const BODY_EXTRA_COLUMNS: [&str; 8] = ["to_list", "cc_list", "reply_to", "message_id", "refs", "files", "signature", "sender_check"];

/// Colonne `signature` d'un message examiné et non signé en S/MIME ; vide :
/// pas encore examiné (corps gardé avant le lot D1).
pub(crate) const UNSIGNED: &str = "-";

/// `Message::signature` d'un message pas encore examiné : l'interface demande
/// alors `check_signature`.
pub(crate) const NOT_EXAMINED: &str = "?";

/// Colonne `signature` d'un message chiffré en S/MIME (lot D3a) : son corps
/// n'est pas gardé en clair, seulement le message chiffré (colonne
/// `ciphertext`), déchiffré en mémoire à chaque ouverture.
pub(crate) const ENCRYPTED: &str = "encrypted";

/// Informations du corps utiles pour répondre (listes séparées par des virgules).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BodyExtra {
    pub to_list: Vec<String>,
    pub cc_list: Vec<String>,
    pub reply_to: Vec<String>,
    pub message_id: String,
    pub references: String,
    pub attachments: Vec<AttachmentInfo>,
    /// Lu seulement : posé par `allow_images`, remis à zéro si le corps est relu.
    pub images_allowed: bool,
    /// Signature S/MIME vérifiée au téléchargement (JSON de `smime::SignatureInfo`),
    /// [`UNSIGNED`], [`ENCRYPTED`] ou vide (lot D1).
    pub signature: String,
    /// Message chiffré tel que reçu, pour [`ENCRYPTED`] seulement (lot D3a) ;
    /// écrit seulement, relu par [`Cache::ciphertext`].
    pub ciphertext: Vec<u8>,
    /// Expéditeur non authentifié par le serveur de réception (lot G1) : vide,
    /// ou la raison (`authres::DMARC_FAIL`, `authres::SPF_FAIL`).
    pub sender_check: String,
}

fn join(list: &[String]) -> String {
    list.join(", ")
}

fn split(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

/// En-tête d'un message tel que stocké dans le cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub uid: u32,
    pub from_name: String,
    pub from_email: String,
    pub subject: String,
    pub preview: String,
    pub date: i64,
    pub flags: Flags,
    pub has_attachments: bool,
}

type Res<T> = Result<T, String>;

#[expect(clippy::needless_pass_by_value, reason = "utilisée avec map_err, qui passe l'erreur par valeur")]
fn sql(e: rusqlite::Error) -> String {
    tr!("cache local : {e}", e = e)
}

// ------------------------------------------------------------------ chiffrement (lot G4)

/// Clé du cache chiffré (lot G4) : 32 octets tirés au hasard, gardés dans le
/// trousseau du système. Jamais affichée (`Debug` la masque).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CacheKey([u8; 32]);

impl std::fmt::Debug for CacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CacheKey(..)")
    }
}

impl CacheKey {
    /// Nouvelle clé tirée par le générateur du système.
    ///
    /// # Errors
    /// Générateur aléatoire du système indisponible.
    pub(crate) fn random() -> Res<Self> {
        crate::oauth::random_bytes::<32>().map(Self)
    }

    /// La clé en hexadécimal (64 caractères), telle que gardée dans le trousseau.
    pub(crate) fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            let _infallible = write!(out, "{byte:02x}");
        }
        out
    }

    /// Relit une clé gardée en hexadécimal ; `None` si elle est abîmée.
    pub(crate) fn from_hex(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let mut key = [0u8; 32];
        for (byte, pair) in key.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
            *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        Some(Self(key))
    }

    /// Clé brute pour `SQLCipher` (`x'…'`) : pas de dérivation PBKDF2, la clé est
    /// déjà aléatoire, donc pas de seconde d'attente au démarrage.
    fn sqlcipher(&self) -> String {
        format!("x'{}'", self.to_hex())
    }
}

/// Pourquoi le cache chiffré ne s'ouvre pas.
#[derive(Debug)]
pub(crate) enum OpenError {
    /// Fichier chiffré avec une autre clé (clé du trousseau perdue), ou abîmé.
    Unreadable,
    /// Autre erreur (disque plein, droits…).
    Failed(String),
}

/// `path` suivi de `suffix` (`cache.sqlite-wal`, `cache.sqlite-chiffrement`…).
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn remove_if_present(path: &Path) -> Res<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(tr!("chiffrement du cache local : {e}", e = e)),
        _ => Ok(()),
    }
}

/// Fichier SQLite en clair ? Son en-tête est « SQLite format 3 » ; celui d'un
/// fichier chiffré par `SQLCipher` est tiré au hasard.
fn is_plain(path: &Path) -> bool {
    let mut head = [0u8; 16];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head == *b"SQLite format 3\0"
}

/// Chiffre un cache encore en clair (versions d'avant le lot G4) : copie
/// chiffrée à côté (`sqlcipher_export`), puis remplacement du fichier. Une
/// interruption laisse soit l'ancien fichier en clair, repris au démarrage
/// suivant, soit le nouveau, jamais un mélange.
fn encrypt_plain(path: &Path, key: &CacheKey) -> Res<()> {
    let target = sibling(path, "-chiffrement");
    remove_if_present(&target)?;
    {
        let plain = Connection::open(path).map_err(sql)?;
        let version: i64 = plain.query_row("PRAGMA user_version", params![], |r| r.get(0)).map_err(sql)?;
        let name = target.to_str().ok_or_else(|| tr!("chiffrement du cache local : {e}", e = target.display()))?;
        plain.execute("ATTACH DATABASE ?1 AS chiffre KEY ?2", params![name, key.sqlcipher()]).map_err(sql)?;
        plain.query_row("SELECT sqlcipher_export('chiffre')", params![], |_| Ok(())).map_err(sql)?;
        plain.execute_batch(&format!("PRAGMA chiffre.user_version = {version}; DETACH DATABASE chiffre;")).map_err(sql)?;
    }
    // Ancienne base fermée : son journal (déjà repris par la copie) part avec elle.
    for suffix in ["-wal", "-shm"] {
        remove_if_present(&sibling(path, suffix))?;
    }
    std::fs::rename(&target, path).map_err(|e| tr!("chiffrement du cache local : {e}", e = e))
}

/// Entrée du trousseau qui garde la clé du cache.
const KEY_ENTRY: &str = "cache/key";

fn key_label() -> &'static str {
    tr!("Vortex Mail — clé du cache local")
}

/// La clé du cache gardée dans le trousseau, créée au premier lancement ; `None`
/// (erreur notée) si le trousseau ne la garde pas : le cache reste alors en clair.
fn profile_key() -> Option<CacheKey> {
    match crate::secrets::read(KEY_ENTRY, key_label()) {
        Ok(Some(hex)) => {
            if let Some(key) = CacheKey::from_hex(&hex) {
                return Some(key);
            }
        }
        Ok(None) => {}
        Err(e) => {
            crate::diag::report(&tr!("clé du cache indisponible : {e} ; cache ouvert sans chiffrement", e = e));
            return None;
        }
    }
    let key = match CacheKey::random() {
        Ok(key) => key,
        Err(e) => {
            crate::diag::report(&e);
            return None;
        }
    };
    if let Err(e) = crate::secrets::write(KEY_ENTRY, key_label(), &key.to_hex()) {
        crate::diag::report(&tr!("clé du cache indisponible : {e} ; cache ouvert sans chiffrement", e = e));
        return None;
    }
    // Relue avant de servir : une clé que le trousseau ne rendrait pas au
    // prochain démarrage laisserait un cache illisible.
    match crate::secrets::read(KEY_ENTRY, key_label()) {
        Ok(Some(hex)) if CacheKey::from_hex(&hex).as_ref() == Some(&key) => Some(key),
        _ => {
            crate::diag::report(tr!("clé du cache mal relue ; cache ouvert sans chiffrement"));
            None
        }
    }
}

/// Ouvre le cache du profil (lot G4) : chiffré par `SQLCipher`, clé dans le
/// trousseau (dans le coffre en mode portable, lot G4b) ; un cache encore en
/// clair est chiffré au passage. Sans trousseau utilisable, le cache reste en
/// clair comme avant. Un cache chiffré avec une clé perdue (trousseau
/// réinitialisé) est mis de côté, jamais effacé, et un cache neuf le remplace :
/// le courrier se resynchronise.
///
/// # Errors
/// Fichier inutilisable (disque, droits) : l'appelant se replie sur un cache en mémoire.
pub(crate) fn open_profile(path: &Path) -> Res<Cache> {
    let Some(key) = profile_key() else { return Cache::open(path) };
    match Cache::open_encrypted(path, &key) {
        Ok(cache) => Ok(cache),
        Err(OpenError::Failed(e)) => Err(e),
        Err(OpenError::Unreadable) => {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let aside = sibling(path, &format!("-illisible-{stamp}"));
            std::fs::rename(path, &aside).map_err(|e| tr!("chiffrement du cache local : {e}", e = e))?;
            for suffix in ["-wal", "-shm"] {
                remove_if_present(&sibling(path, suffix))?;
            }
            crate::diag::report(&tr!(
                "cache local chiffré avec une clé perdue : mis de côté ({file}), nouveau cache créé",
                file = aside.display()
            ));
            Cache::open_encrypted(path, &key).map_err(|e| match e {
                OpenError::Failed(e) => e,
                OpenError::Unreadable => tr!("cache local illisible").to_string(),
            })
        }
    }
}

fn role_to_str(role: FolderRole) -> &'static str {
    match role {
        FolderRole::Inbox => "inbox",
        FolderRole::Drafts => "drafts",
        FolderRole::Sent => "sent",
        FolderRole::Archive => "archive",
        FolderRole::Trash => "trash",
        FolderRole::Junk => "junk",
        FolderRole::Custom => "custom",
    }
}

fn role_from_str(s: &str) -> FolderRole {
    match s {
        "inbox" => FolderRole::Inbox,
        "drafts" => FolderRole::Drafts,
        "sent" => FolderRole::Sent,
        "archive" => FolderRole::Archive,
        "trash" => FolderRole::Trash,
        "junk" => FolderRole::Junk,
        _ => FolderRole::Custom,
    }
}

pub struct Cache {
    conn: Connection,
    ids: HashMap<MessageId, ImapRef>,
    reverse: HashMap<ImapRef, MessageId>,
    next_id: MessageId,
    revisions: Revisions,
}

/// Version de la liste de chaque dossier (audit P1) : augmentée à chaque
/// écriture qui change ce que `list` rend (en-têtes, drapeaux, suppressions).
/// L'interface la renvoie à la synchronisation suivante et ne relit la liste
/// que si elle a changé. Départ à l'heure en millisecondes : un cache rouvert
/// (mode portable) ne redonne jamais une version déjà vue.
#[derive(Debug)]
struct Revisions {
    counter: u64,
    /// Version de tous les dossiers (compte supprimé, renommage, arborescence).
    all: u64,
    folders: HashMap<(String, String), u64>,
    /// Version des carnets, agendas et de leur contenu (complétion des
    /// adresses gardée en mémoire tant qu'elle ne change pas, lot J10).
    pim: u64,
    /// Version des adresses collectées (lot J13) : corps d'un message envoyé
    /// gardé, envoi, adresse oubliée.
    addresses: u64,
}

impl Revisions {
    fn starting_now() -> Self {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_millis());
        let start = u64::try_from(now).unwrap_or(1);
        Self { counter: start, all: start, folders: HashMap::new(), pim: start, addresses: start }
    }

    fn touch(&mut self, account: &str, path: &str) {
        self.counter = self.counter.saturating_add(1);
        self.folders.insert((account.to_string(), path.to_string()), self.counter);
    }

    fn touch_all(&mut self) {
        self.counter = self.counter.saturating_add(1);
        self.all = self.counter;
    }

    fn touch_pim(&mut self) {
        self.counter = self.counter.saturating_add(1);
        self.pim = self.counter;
    }

    fn touch_addresses(&mut self) {
        self.counter = self.counter.saturating_add(1);
        self.addresses = self.counter;
    }

    fn of(&self, account: &str, path: &str) -> u64 {
        let folder = self.folders.get(&(account.to_string(), path.to_string())).copied().unwrap_or(0);
        folder.max(self.all)
    }
}

/// N'affiche pas le contenu de la base (messages, adresses).
impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache").field("known_messages", &self.ids.len()).finish_non_exhaustive()
    }
}

/// Décode les aperçus restés en base64 dans le cache (voir
/// `mimeparse::decode_if_base64`).
fn repair_base64_previews(conn: &Connection) -> Res<()> {
    let broken: Vec<(String, String, u32, String)> = {
        let mut stmt = conn.prepare("SELECT account, path, uid, preview FROM messages").map_err(sql)?;
        let rows = stmt.query_map(params![], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(sql)?;
        rows.filter_map(Result::ok)
            .filter_map(|(a, p, u, preview): (String, String, u32, String)| {
                crate::mimeparse::decode_if_base64(&preview).map(|text| (a, p, u, mail_core::mime::preview(&text, 140)))
            })
            .collect()
    };
    for (account, path, uid, preview) in broken {
        conn.execute(
            "UPDATE messages SET preview = ?4 WHERE account = ?1 AND path = ?2 AND uid = ?3",
            params![account, path, uid, preview],
        )
        .map_err(sql)?;
    }
    Ok(())
}

/// Adresse collectée pour la complétion (lot J13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedAddress {
    pub email: String,
    pub name: String,
    /// Messages envoyés à cette adresse trouvés dans le cache.
    pub sent: u32,
    /// Date du dernier envoi (secondes depuis 1970).
    pub last: i64,
}

/// Destinataires d'un message envoyé : (nom, adresse). `first` est le premier
/// destinataire tel que gardé (`Nom <adresse>`) : seul son nom est connu.
fn recipients_of(first: &str, to: &str, cc: &str) -> Vec<(String, String)> {
    let named = first.trim().strip_suffix('>').and_then(|f| f.rsplit_once('<')).map(|(name, email)| (name.trim(), email.trim()));
    split(to)
        .into_iter()
        .chain(split(cc))
        .filter(|email| email.contains('@'))
        .map(|email| {
            let name = named.filter(|(_, e)| e.eq_ignore_ascii_case(&email)).map_or("", |(n, _)| n);
            (name.trim_matches('"').to_string(), email)
        })
        .collect()
}

/// Ajoute des destinataires aux adresses collectées. `counted` : ils viennent
/// d'un message de « Envoyés » nouvellement gardé (compté, sauf adresse
/// écartée) ; sinon d'un envoi de Vortex Mail (noté, et de nouveau proposé
/// s'il avait été écarté : l'utilisateur vient d'y écrire).
fn collect_addresses(conn: &Connection, account: &str, recipients: &[(String, String)], when: i64, counted: bool) -> Res<()> {
    for (name, email) in recipients {
        let key = email.to_lowercase();
        if counted {
            let forgotten =
                conn.prepare_cached("SELECT 1 FROM forgotten_addresses WHERE key = ?1").map_err(sql)?.exists(params![key]).map_err(sql)?;
            if forgotten {
                continue;
            }
        } else {
            conn.prepare_cached("DELETE FROM forgotten_addresses WHERE key = ?1").map_err(sql)?.execute(params![key]).map_err(sql)?;
        }
        conn.prepare_cached(
            "INSERT INTO collected_addresses (account, key, email, name, sent, last) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(account, key) DO UPDATE SET
               name = CASE WHEN excluded.name = '' THEN collected_addresses.name ELSE excluded.name END,
               sent = collected_addresses.sent + excluded.sent,
               last = MAX(collected_addresses.last, excluded.last)",
        )
        .map_err(sql)?
        .execute(params![account, key, email, name, i64::from(counted), when])
        .map_err(sql)?;
    }
    Ok(())
}

/// Reprise de l'historique (lot J13) : destinataires des messages de
/// « Éléments envoyés » déjà gardés dans le cache, une fois pour toutes.
fn collect_sent_history(conn: &Connection) -> Res<()> {
    let sent: Vec<(String, String, String, String, i64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT b.account, b.to_addr, b.to_list, b.cc_list, m.date FROM bodies b
                 JOIN messages m ON m.account = b.account AND m.path = b.path AND m.uid = b.uid
                 JOIN folders f ON f.account = b.account AND f.path = b.path
                 WHERE f.role = 'sent'",
            )
            .map_err(sql)?;
        let rows = stmt.query_map(params![], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)?
    };
    let tx = conn.unchecked_transaction().map_err(sql)?;
    for (account, first, to, cc, date) in sent {
        collect_addresses(&tx, &account, &recipients_of(&first, &to, &cc), date, true)?;
    }
    tx.commit().map_err(sql)
}

impl Cache {
    pub fn open(path: &Path) -> Res<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| tr!("création de {path} impossible : {e}", path = dir.display(), e = e))?;
        }
        Self::init(Connection::open(path).map_err(sql)?)
    }

    /// Ouvre le cache chiffré par `key` (lot G4) ; un cache encore en clair est
    /// chiffré au passage (`encrypt_plain`).
    ///
    /// # Errors
    /// `Unreadable` si le fichier est chiffré avec une autre clé ou abîmé ;
    /// `Failed` pour toute autre erreur.
    pub(crate) fn open_encrypted(path: &Path, key: &CacheKey) -> Result<Self, OpenError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| OpenError::Failed(tr!("création de {path} impossible : {e}", path = dir.display(), e = e)))?;
        }
        if is_plain(path) {
            encrypt_plain(path, key).map_err(OpenError::Failed)?;
        }
        let conn = Connection::open(path).map_err(|e| OpenError::Failed(sql(e)))?;
        // La clé avant toute autre instruction ; fichiers temporaires en mémoire,
        // jamais en clair sur le disque.
        conn.execute_batch(&format!("PRAGMA key = \"{}\"; PRAGMA temp_store = MEMORY;", key.sqlcipher()))
            .map_err(|e| OpenError::Failed(sql(e)))?;
        match conn.query_row("SELECT count(*) FROM sqlite_master", params![], |r| r.get::<_, i64>(0)) {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::NotADatabase => return Err(OpenError::Unreadable),
            Err(e) => return Err(OpenError::Failed(sql(e))),
        }
        Self::init(conn).map_err(OpenError::Failed)
    }

    /// Cache éphémère (tests, ou repli si le fichier est inutilisable).
    pub fn in_memory() -> Res<Self> {
        Self::init(Connection::open_in_memory().map_err(sql)?)
    }

    fn init(conn: Connection) -> Res<Self> {
        conn.execute_batch(SCHEMA).map_err(sql)?;
        // Requêtes préparées gardées (audit P7) : de quoi tenir les plus fréquentes.
        conn.set_prepared_statement_cache_capacity(64);
        // Migration : ajoute les colonnes manquantes d'une base plus ancienne.
        let existing: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(bodies)").map_err(sql)?;
            let rows = stmt.query_map(params![], |r| r.get::<_, String>(1)).map_err(sql)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql)?
        };
        for column in BODY_EXTRA_COLUMNS {
            if !existing.iter().any(|c| c == column) {
                conn.execute_batch(&format!("ALTER TABLE bodies ADD COLUMN {column} TEXT NOT NULL DEFAULT ''")).map_err(sql)?;
            }
        }
        let folder_columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(folders)").map_err(sql)?;
            let rows = stmt.query_map(params![], |r| r.get::<_, String>(1)).map_err(sql)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql)?
        };
        let body_columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(bodies)").map_err(sql)?;
            let rows = stmt.query_map(params![], |r| r.get::<_, String>(1)).map_err(sql)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql)?
        };
        if !body_columns.iter().any(|c| c == "images") {
            conn.execute_batch("ALTER TABLE bodies ADD COLUMN images INTEGER NOT NULL DEFAULT 0").map_err(sql)?;
        }
        if !body_columns.iter().any(|c| c == "ciphertext") {
            conn.execute_batch("ALTER TABLE bodies ADD COLUMN ciphertext BLOB").map_err(sql)?;
        }
        if !folder_columns.iter().any(|c| c == "subscribed") {
            conn.execute_batch("ALTER TABLE folders ADD COLUMN subscribed INTEGER NOT NULL DEFAULT 1").map_err(sql)?;
        }
        // Synchronisation incrémentale (CONDSTORE) : NULL = inconnu, d'où une
        // synchronisation complète au premier passage sur une base ancienne.
        let state_columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(folder_state)").map_err(sql)?;
            let rows = stmt.query_map(params![], |r| r.get::<_, String>(1)).map_err(sql)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql)?
        };
        for column in ["highestmodseq", "exists_count"] {
            if !state_columns.iter().any(|c| c == column) {
                conn.execute_batch(&format!("ALTER TABLE folder_state ADD COLUMN {column} INTEGER")).map_err(sql)?;
            }
        }
        // Version 1 : l'objet vide n'est plus enregistré en toutes lettres
        // (« (sans objet) », traduit à l'affichage).
        let version: i64 = conn.query_row("PRAGMA user_version", params![], |r| r.get(0)).map_err(sql)?;
        if version < 1 {
            conn.execute_batch("UPDATE messages SET subject = '' WHERE subject = '(sans objet)'; PRAGMA user_version = 1;").map_err(sql)?;
        }
        // Version 2 : aperçus restés en base64 (début d'une partie base64 que
        // `mail-parser` ne décodait pas), décodés une fois pour toutes.
        if version < 2 {
            repair_base64_previews(&conn)?;
            conn.execute_batch("PRAGMA user_version = 2;").map_err(sql)?;
        }
        // Version 4 : adresses collectées pour la complétion, par compte
        // (lot J13), à partir des messages envoyés déjà gardés. La table d'une
        // version 3 (sans compte) est refaite.
        if version < 4 {
            conn.execute_batch("DROP TABLE IF EXISTS collected_addresses;").map_err(sql)?;
            conn.execute_batch(SCHEMA).map_err(sql)?;
            collect_sent_history(&conn)?;
            conn.execute_batch("PRAGMA user_version = 4;").map_err(sql)?;
        }
        Ok(Self { conn, ids: HashMap::new(), reverse: HashMap::new(), next_id: ID_BASE, revisions: Revisions::starting_now() })
    }

    // ------------------------------------------------------------ identifiants

    pub fn id_for(&mut self, r: ImapRef) -> MessageId {
        if let Some(id) = self.reverse.get(&r) {
            return *id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.ids.insert(id, r.clone());
        self.reverse.insert(r, id);
        id
    }

    /// Identifiant provisoire, non relié à un message (serveur sans UIDPLUS).
    /// Il est réservé : aucun message ne le recevra ensuite (relecture Y1 : un
    /// brouillon et un vrai message partageaient le même numéro, et remplacer
    /// le brouillon supprimait le message).
    pub fn fresh_id(&mut self) -> MessageId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn imap_ref(&self, id: MessageId) -> Option<ImapRef> {
        self.ids.get(&id).cloned()
    }

    pub fn forget(&mut self, id: MessageId) {
        if let Some(r) = self.ids.remove(&id) {
            self.reverse.remove(&r);
        }
    }

    // ------------------------------------------------------------ comptes

    /// Liste des comptes affichés (copie, pour lire sans verrou réseau).
    pub fn set_accounts(&mut self, accounts: &[Account]) -> Res<()> {
        let tx = self.conn.savepoint().map_err(sql)?;
        tx.execute("DELETE FROM accounts", params![]).map_err(sql)?;
        for (i, a) in accounts.iter().enumerate() {
            tx.execute(
                "INSERT INTO accounts (id, name, email, color, position) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![a.id, a.name, a.email, a.color, i64::try_from(i).unwrap_or(i64::MAX)],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)
    }

    pub fn accounts(&self) -> Res<Vec<Account>> {
        let mut stmt = self.conn.prepare("SELECT id, name, email, color FROM accounts ORDER BY position").map_err(sql)?;
        let rows = stmt
            .query_map(params![], |r| Ok(Account { id: r.get(0)?, name: r.get(1)?, email: r.get(2)?, color: r.get(3)? }))
            .map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    /// Efface tout ce qui concerne un compte supprimé, ou dont le serveur a
    /// changé : y compris les actions en attente et les déplacements
    /// interrompus, qui désignent des messages de l'ancien serveur et ne
    /// doivent jamais être rejoués sur le nouveau (relecture Y6).
    pub fn delete_account(&mut self, account: &str) -> Res<()> {
        self.revisions.touch_all();
        let tx = self.conn.savepoint().map_err(sql)?;
        // Adresses collectées depuis ce compte (lot J13) : celles écrites
        // depuis un autre compte restent proposées.
        for table in [
            "folders",
            "folder_state",
            "messages",
            "bodies",
            "pending_actions",
            "move_journal",
            "pending_sent",
            "trash_origins",
            "collected_addresses",
        ] {
            tx.execute(&format!("DELETE FROM {table} WHERE account = ?1"), params![account]).map_err(sql)?;
        }
        // Déplacements vers ce compte depuis un autre : la cible n'existe plus.
        let prefix = format!("{account}:");
        tx.execute("DELETE FROM move_journal WHERE substr(target, 1, length(?1)) = ?1", params![prefix]).map_err(sql)?;
        tx.commit().map_err(sql)?;
        self.ids.retain(|_, r| r.account != account);
        self.reverse.retain(|r, _| r.account != account);
        Ok(())
    }

    // ------------------------------------------------------------ dossiers

    /// Remplace l'arborescence d'un compte et purge les dossiers disparus.
    pub fn save_folders(&mut self, account: &str, folders: &[Folder]) -> Res<()> {
        let tx = self.conn.savepoint().map_err(sql)?;
        tx.execute("DELETE FROM folders WHERE account = ?1", params![account]).map_err(sql)?;
        for (i, f) in folders.iter().enumerate() {
            let parent: Option<String> = f.parent_id.as_deref().and_then(|p| p.split_once(':')).map(|(_, path)| path.to_string());
            tx.execute(
                "INSERT INTO folders (account, path, name, parent, depth, role, position, unread, total, subscribed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    account,
                    f.path,
                    f.name,
                    parent,
                    f.depth,
                    role_to_str(f.role),
                    i64::try_from(i).unwrap_or(i64::MAX),
                    f.unread,
                    f.total,
                    f.subscribed
                ],
            )
            .map_err(sql)?;
        }
        let mut removed = 0;
        for table in ["folder_state", "messages", "bodies"] {
            removed += tx
                .execute(
                    &format!(
                        "DELETE FROM {table} WHERE account = ?1
                         AND path NOT IN (SELECT path FROM folders WHERE account = ?1)"
                    ),
                    params![account],
                )
                .map_err(sql)?;
        }
        tx.commit().map_err(sql)?;
        // Seulement si un dossier disparu avait des messages : relire
        // l'arborescence à chaque synchronisation ne rend pas les listes « changées ».
        if removed > 0 {
            self.revisions.touch_all();
        }
        Ok(())
    }

    pub fn folders(&self, account: &str) -> Res<Vec<Folder>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT path, name, parent, depth, role, unread, total, subscribed
                 FROM folders WHERE account = ?1 ORDER BY position",
            )
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![account], |r| {
                let path: String = r.get(0)?;
                let parent: Option<String> = r.get(2)?;
                let role = role_from_str(&r.get::<_, String>(4)?);
                let stored: String = r.get(1)?;
                Ok(Folder {
                    id: format!("{account}:{path}"),
                    account_id: account.to_string(),
                    // Dossier spécial : nommé dans la langue de l'interface au moment de la
                    // lecture, pas dans celle de la dernière synchronisation (lot E3).
                    name: crate::router::role_name(role).map_or(stored, str::to_owned),
                    parent_id: parent.map(|p| format!("{account}:{p}")),
                    path,
                    depth: r.get(3)?,
                    role,
                    unread: r.get(5)?,
                    total: r.get(6)?,
                    subscribed: r.get(7)?,
                })
            })
            .map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    /// Arborescence de tous les comptes, dans l'ordre des comptes.
    pub fn all_folders(&self) -> Res<Vec<Folder>> {
        let mut out = Vec::new();
        for a in self.accounts()? {
            out.extend(self.folders(&a.id)?);
        }
        Ok(out)
    }

    /// Ajuste le compteur de non-lus après un changement local.
    pub fn adjust_unread(&mut self, account: &str, path: &str, delta: i64) -> Res<()> {
        self.conn
            .execute("UPDATE folders SET unread = MAX(0, unread + ?3) WHERE account = ?1 AND path = ?2", params![account, path, delta])
            .map_err(sql)?;
        Ok(())
    }

    // ------------------------------------------------------------ état d'un dossier

    pub fn uid_validity(&self, account: &str, path: &str) -> Res<Option<u32>> {
        self.conn
            .query_row("SELECT uidvalidity FROM folder_state WHERE account = ?1 AND path = ?2", params![account, path], |r| {
                r.get::<_, u32>(0)
            })
            .optional()
            .map_err(sql)
    }

    /// Enregistre l'UIDVALIDITY ; s'il change, le MODSEQ mémorisé ne vaut plus rien.
    pub fn set_uid_validity(&mut self, account: &str, path: &str, value: u32) -> Res<()> {
        self.conn
            .execute(
                "INSERT INTO folder_state (account, path, uidvalidity) VALUES (?1, ?2, ?3)
                 ON CONFLICT (account, path) DO UPDATE SET
                     highestmodseq = CASE WHEN uidvalidity = excluded.uidvalidity THEN highestmodseq END,
                     exists_count  = CASE WHEN uidvalidity = excluded.uidvalidity THEN exists_count END,
                     uidvalidity   = excluded.uidvalidity",
                params![account, path, value],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// État de la dernière synchronisation réussie (RFC 7162) : HIGHESTMODSEQ
    /// et nombre de messages du dossier à ce moment-là. `None` : inconnu.
    pub fn modseq_state(&self, account: &str, path: &str) -> Res<Option<(u64, Option<u32>)>> {
        let row = self
            .conn
            .query_row(
                "SELECT highestmodseq, exists_count FROM folder_state WHERE account = ?1 AND path = ?2",
                params![account, path],
                |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<u32>>(1)?)),
            )
            .optional()
            .map_err(sql)?;
        Ok(row.and_then(|(modseq, exists)| Some((u64::try_from(modseq?).ok()?, exists))))
    }

    /// Mémorise l'état d'une synchronisation réussie (`None` : à refaire en entier).
    /// Sans effet si l'UIDVALIDITY du dossier n'est pas encore enregistré.
    pub fn set_modseq_state(&mut self, account: &str, path: &str, modseq: Option<u64>, exists: u32) -> Res<()> {
        let modseq: Option<i64> = modseq.and_then(|m| i64::try_from(m).ok());
        self.conn
            .execute(
                "UPDATE folder_state SET highestmodseq = ?3, exists_count = ?4 WHERE account = ?1 AND path = ?2",
                params![account, path, modseq, exists],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// Vide le contenu d'un dossier (UIDVALIDITY changé : les UID ne valent plus rien).
    pub fn reset_folder(&mut self, account: &str, path: &str) -> Res<()> {
        self.revisions.touch(account, path);
        let tx = self.conn.savepoint().map_err(sql)?;
        for table in ["messages", "bodies"] {
            tx.execute(&format!("DELETE FROM {table} WHERE account = ?1 AND path = ?2"), params![account, path]).map_err(sql)?;
        }
        // Dossier renuméroté : les actions en attente et les déplacements
        // interrompus portent des UID qui désignent désormais d'autres messages
        // (relecture complète n° 2). Un déplacement déjà copié laisse au pire un
        // doublon, jamais une suppression du mauvais message.
        for table in ["pending_actions", "move_journal"] {
            tx.execute(&format!("DELETE FROM {table} WHERE account = ?1 AND mailbox = ?2"), params![account, path]).map_err(sql)?;
        }
        // Cache vidé : la prochaine synchronisation repart de zéro.
        tx.execute(
            "UPDATE folder_state SET highestmodseq = NULL, exists_count = NULL WHERE account = ?1 AND path = ?2",
            params![account, path],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        self.ids.retain(|_, r| !(r.account == account && r.mailbox == path));
        self.reverse.retain(|r, _| !(r.account == account && r.mailbox == path));
        Ok(())
    }

    /// Dossier renommé ou déplacé sur le serveur : ses messages (et ceux de ses
    /// sous-dossiers) suivent dans le cache, sans tout retélécharger.
    pub fn rename_path(&mut self, account: &str, old: &str, new: &str, delimiter: char) -> Res<()> {
        self.revisions.touch_all();
        let prefix = format!("{old}{delimiter}");
        let tail = i64::try_from(old.chars().count()).unwrap_or(i64::MAX).saturating_add(1);
        let prefix_len = i64::try_from(prefix.chars().count()).unwrap_or(i64::MAX);
        let tx = self.conn.savepoint().map_err(sql)?;
        for (table, column) in [
            ("messages", "path"),
            ("bodies", "path"),
            ("folder_state", "path"),
            ("folders", "path"),
            ("move_journal", "mailbox"),
            ("pending_actions", "mailbox"),
            // Audit B6 : « Restaurer », copies d'envoi en attente, dossier parent.
            ("trash_origins", "mailbox"),
            ("pending_sent", "mailbox"),
            ("folders", "parent"),
        ] {
            tx.execute(
                &format!(
                    "UPDATE {table} SET {column} = ?3 || substr({column}, ?4)
                     WHERE account = ?1 AND ({column} = ?2 OR substr({column}, 1, ?5) = ?6)"
                ),
                params![account, old, new, tail, prefix_len, prefix],
            )
            .map_err(sql)?;
        }
        // Dossiers cibles notés « compte:dossier » : déplacements interrompus
        // (même venus d'un autre compte) et déplacements en attente (audit B6).
        let folder_id = format!("{account}:{old}");
        let folder_prefix = format!("{folder_id}{delimiter}");
        let id_tail = i64::try_from(folder_id.chars().count()).unwrap_or(i64::MAX).saturating_add(1);
        let id_prefix_len = i64::try_from(folder_prefix.chars().count()).unwrap_or(i64::MAX);
        for (table, column, only) in [("move_journal", "target", ""), ("pending_actions", "arg", " AND kind = 'move'")] {
            tx.execute(
                &format!(
                    "UPDATE {table} SET {column} = ?1 || substr({column}, ?2)
                     WHERE ({column} = ?3 OR substr({column}, 1, ?4) = ?5){only}"
                ),
                params![format!("{account}:{new}"), id_tail, folder_id, id_prefix_len, folder_prefix],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)?;
        let moved = |mailbox: &str| -> Option<String> {
            if mailbox == old {
                Some(new.to_string())
            } else {
                mailbox.strip_prefix(prefix.as_str()).map(|rest| format!("{new}{delimiter}{rest}"))
            }
        };
        let mut renamed = Vec::new();
        for (id, r) in &self.ids {
            if r.account == account {
                if let Some(mailbox) = moved(&r.mailbox) {
                    renamed.push((*id, r.clone(), ImapRef { mailbox, ..r.clone() }));
                }
            }
        }
        for (id, before, after) in renamed {
            self.reverse.remove(&before);
            self.reverse.insert(after.clone(), id);
            self.ids.insert(id, after);
        }
        Ok(())
    }

    // ------------------------------------------------------------ messages

    pub fn cached_uids(&self, account: &str, path: &str) -> Res<Vec<u32>> {
        let mut stmt = self.conn.prepare("SELECT uid FROM messages WHERE account = ?1 AND path = ?2 ORDER BY uid").map_err(sql)?;
        let rows = stmt.query_map(params![account, path], |r| r.get::<_, u32>(0)).map_err(sql)?;
        rows.collect::<Result<Vec<u32>, _>>().map_err(sql)
    }

    pub fn upsert_headers(&mut self, account: &str, path: &str, headers: &[Header]) -> Res<()> {
        if headers.is_empty() {
            return Ok(());
        }
        self.revisions.touch(account, path);
        let tx = self.conn.savepoint().map_err(sql)?;
        {
            // Requête préparée une fois pour toute la tranche (audit P7).
            let mut insert = tx
                .prepare_cached(
                    "INSERT OR REPLACE INTO messages
                       (account, path, uid, from_name, from_email, subject, preview, date,
                        seen, flagged, answered, attachments)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                )
                .map_err(sql)?;
            for h in headers {
                insert
                    .execute(params![
                        account,
                        path,
                        h.uid,
                        h.from_name,
                        h.from_email,
                        h.subject,
                        h.preview,
                        h.date,
                        h.flags.seen,
                        h.flags.flagged,
                        h.flags.answered,
                        h.has_attachments
                    ])
                    .map_err(sql)?;
            }
        }
        tx.commit().map_err(sql)
    }

    /// Drapeaux relus sur le serveur : seuls les messages dont l'état a changé
    /// sont réécrits (le plus souvent : aucun), sans relire tout le dossier
    /// pour trois messages changés (audit P7).
    pub fn update_flags(&mut self, account: &str, path: &str, flags: &[(u32, Flags)]) -> Res<()> {
        if flags.is_empty() {
            return Ok(());
        }
        let tx = self.conn.savepoint().map_err(sql)?;
        let mut changed = 0;
        {
            let mut update = tx
                .prepare_cached(
                    "UPDATE messages SET seen = ?4, flagged = ?5, answered = ?6
                     WHERE account = ?1 AND path = ?2 AND uid = ?3
                       AND (seen IS NOT ?4 OR flagged IS NOT ?5 OR answered IS NOT ?6)",
                )
                .map_err(sql)?;
            for (uid, f) in flags {
                changed += update.execute(params![account, path, uid, f.seen, f.flagged, f.answered]).map_err(sql)?;
            }
        }
        tx.commit().map_err(sql)?;
        if changed > 0 {
            self.revisions.touch(account, path);
        }
        Ok(())
    }

    /// Pose ou retire un flag localement (`\Seen`, `\Answered` ou
    /// `\Flagged`) ; un autre (`$Forwarded`…) n'est pas gardé dans le cache.
    pub fn set_flag(&mut self, account: &str, path: &str, uid: u32, flag: &str, on: bool) -> Res<()> {
        let column = if flag.eq_ignore_ascii_case("\\Seen") {
            "seen"
        } else if flag.eq_ignore_ascii_case("\\Answered") {
            "answered"
        } else if flag.eq_ignore_ascii_case("\\Flagged") {
            "flagged"
        } else {
            return Ok(());
        };
        let changed = self
            .conn
            .execute(
                &format!("UPDATE messages SET {column} = ?4 WHERE account = ?1 AND path = ?2 AND uid = ?3 AND {column} != ?4"),
                params![account, path, uid, on],
            )
            .map_err(sql)?;
        if changed > 0 {
            self.revisions.touch(account, path);
        }
        Ok(())
    }

    pub fn is_seen(&self, account: &str, path: &str, uid: u32) -> Res<Option<bool>> {
        self.conn
            .query_row("SELECT seen FROM messages WHERE account = ?1 AND path = ?2 AND uid = ?3", params![account, path, uid], |r| {
                r.get::<_, bool>(0)
            })
            .optional()
            .map_err(sql)
    }

    /// Plusieurs suppressions en une seule transaction.
    pub fn delete_messages(&mut self, account: &str, path: &str, uids: &[u32]) -> Res<()> {
        if uids.is_empty() {
            return Ok(());
        }
        self.revisions.touch(account, path);
        let tx = self.conn.savepoint().map_err(sql)?;
        {
            let mut messages = tx.prepare_cached("DELETE FROM messages WHERE account = ?1 AND path = ?2 AND uid = ?3").map_err(sql)?;
            let mut bodies = tx.prepare_cached("DELETE FROM bodies WHERE account = ?1 AND path = ?2 AND uid = ?3").map_err(sql)?;
            for uid in uids {
                messages.execute(params![account, path, uid]).map_err(sql)?;
                bodies.execute(params![account, path, uid]).map_err(sql)?;
            }
        }
        tx.commit().map_err(sql)?;
        for uid in uids {
            let r = ImapRef { account: account.to_string(), mailbox: path.to_string(), uid: *uid };
            if let Some(id) = self.reverse.remove(&r) {
                self.ids.remove(&id);
            }
        }
        Ok(())
    }

    pub fn delete_message(&mut self, account: &str, path: &str, uid: u32) -> Res<()> {
        self.revisions.touch(account, path);
        let tx = self.conn.savepoint().map_err(sql)?;
        for table in ["messages", "bodies"] {
            tx.execute(&format!("DELETE FROM {table} WHERE account = ?1 AND path = ?2 AND uid = ?3"), params![account, path, uid])
                .map_err(sql)?;
        }
        tx.commit().map_err(sql)?;
        let r = ImapRef { account: account.to_string(), mailbox: path.to_string(), uid };
        if let Some(id) = self.reverse.remove(&r) {
            self.ids.remove(&id);
        }
        Ok(())
    }

    pub fn header(&self, account: &str, path: &str, uid: u32) -> Res<Option<Header>> {
        self.conn
            .query_row(
                "SELECT uid, from_name, from_email, subject, preview, date, seen, flagged, answered, attachments
                 FROM messages WHERE account = ?1 AND path = ?2 AND uid = ?3",
                params![account, path, uid],
                |r| {
                    Ok(Header {
                        uid: r.get(0)?,
                        from_name: r.get(1)?,
                        from_email: r.get(2)?,
                        subject: r.get(3)?,
                        preview: r.get(4)?,
                        date: r.get(5)?,
                        flags: Flags { seen: r.get(6)?, flagged: r.get(7)?, answered: r.get(8)? },
                        has_attachments: r.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(sql)
    }

    /// Exécute `write` en une seule transaction (audit P7 : une synchronisation
    /// écrivait en cinq ou six transactions) ; tout est annulé si elle échoue.
    /// Les écritures de `Cache` y deviennent des points de sauvegarde
    /// (`savepoint`), qui s'imbriquent.
    ///
    /// # Errors
    ///
    /// Erreur de `write`, ou de la base (début ou validation).
    pub fn atomically<T>(&mut self, write: impl FnOnce(&mut Self) -> Res<T>) -> Res<T> {
        // Transaction restée ouverte par une panique (verrou empoisonné, puis
        // repris) : annulée avant d'en commencer une autre.
        if !self.conn.is_autocommit() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
        self.conn.execute_batch("BEGIN IMMEDIATE").map_err(sql)?;
        match write(self) {
            Ok(value) => {
                self.conn.execute_batch("COMMIT").map_err(sql)?;
                Ok(value)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// Version de la liste du dossier (voir `Revisions`) : égale à celle
    /// d'une lecture précédente si `list` rendrait la même chose.
    pub fn revision(&self, account: &str, path: &str) -> u64 {
        self.revisions.of(account, path)
    }

    /// Messages d'un dossier, du plus récent au plus ancien, avec leurs identifiants.
    pub fn list(&mut self, account: &str, path: &str) -> Res<Vec<MessageSummary>> {
        let headers: Vec<Header> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT uid, from_name, from_email, subject, preview, date, seen, flagged, answered, attachments
                     FROM messages WHERE account = ?1 AND path = ?2 ORDER BY date DESC, uid DESC",
                )
                .map_err(sql)?;
            let rows = stmt
                .query_map(params![account, path], |r| {
                    Ok(Header {
                        uid: r.get(0)?,
                        from_name: r.get(1)?,
                        from_email: r.get(2)?,
                        subject: r.get(3)?,
                        preview: r.get(4)?,
                        date: r.get(5)?,
                        flags: Flags { seen: r.get(6)?, flagged: r.get(7)?, answered: r.get(8)? },
                        has_attachments: r.get(9)?,
                    })
                })
                .map_err(sql)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql)?
        };
        Ok(headers.into_iter().map(|h| self.summary(account, path, h)).collect())
    }

    /// Message complet si son corps est déjà en cache (lecture hors ligne).
    pub fn message(&self, id: MessageId) -> Res<Option<Message>> {
        let Some(r) = self.imap_ref(id) else { return Ok(None) };
        let header = self.header(&r.account, &r.mailbox, r.uid)?;
        let body = self.body(&r.account, &r.mailbox, r.uid)?;
        let (Some(h), Some((to, html))) = (header, body) else { return Ok(None) };
        let extra = self.body_extra(&r.account, &r.mailbox, r.uid)?;
        // Corps mis en cache par une version antérieure : on le relira du serveur.
        if extra.message_id.is_empty() {
            return Ok(None);
        }
        // Message chiffré gardé par les lots D1 et D2 comme une pièce `smime.p7m` :
        // à examiner de nouveau, pour le déchiffrer (lot D3a).
        let old_encrypted =
            extra.signature == UNSIGNED && extra.attachments.iter().any(|a| a.content_type.to_ascii_lowercase().contains("pkcs7-mime"));
        Ok(Some(Message {
            summary: MessageSummary {
                id,
                folder_id: format!("{}:{}", r.account, r.mailbox),
                account_id: r.account,
                from_name: h.from_name,
                from_email: h.from_email,
                subject: h.subject,
                preview: h.preview,
                date: h.date,
                flags: h.flags,
                has_attachments: h.has_attachments,
                uid: r.uid,
            },
            to,
            body_html: html,
            to_list: extra.to_list,
            cc_list: extra.cc_list,
            reply_to: extra.reply_to,
            message_id: if extra.message_id == "-" { String::new() } else { extra.message_id },
            references: extra.references,
            // Partie `smime.p7s` des corps gardés au début du lot D1 : montrée
            // par la barre de signature, pas comme pièce jointe.
            attachments: if matches!(extra.signature.as_str(), "" | UNSIGNED) {
                extra.attachments
            } else {
                extra.attachments.into_iter().filter(|a| !crate::smime::is_signature_part(&a.content_type)).collect()
            },
            images_allowed: extra.images_allowed,
            body_simple: String::new(),
            body_text: String::new(),
            signature: match extra.signature.as_str() {
                _ if old_encrypted => NOT_EXAMINED.to_string(),
                UNSIGNED => String::new(),
                "" => NOT_EXAMINED.to_string(),
                _ => extra.signature,
            },
            encryption: String::new(),
            sender_check: extra.sender_check,
        }))
    }

    /// Recherche locale (hors connexion), plus récents d'abord (lot E7 : critères
    /// de [`SearchQuery`]). Les mots sont cherchés dans l'objet, l'expéditeur et
    /// l'aperçu ; `à:` dans les destinataires des messages déjà ouverts, seuls
    /// connus du cache. Les dates sont des jours à l'heure locale.
    pub fn search(&mut self, query: &SearchQuery, limit: usize) -> Res<Vec<MessageSummary>> {
        // Un mot : dans l'objet, le nom ou l'adresse de l'expéditeur, ou l'aperçu
        // (chaque `?` reçoit la même valeur).
        const TEXT: &str = "(m.subject LIKE ? ESCAPE '\\' OR m.from_name LIKE ? ESCAPE '\\' OR m.from_email LIKE ? ESCAPE '\\' OR m.preview LIKE ? ESCAPE '\\')";
        let like = |text: &str| format!("%{}%", text.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let local_midnight = |day: mail_core::search::Date| {
            let utc = day.utc_midnight();
            utc - i64::from(crate::tz::local_offset(utc))
        };
        let mut conditions: Vec<String> = Vec::new();
        let mut values: Vec<rusqlite::types::Value> = Vec::new();
        let mut add = |condition: &str, value: rusqlite::types::Value, values: &mut Vec<rusqlite::types::Value>| {
            values.push(value);
            conditions.push(condition.replace('?', &format!("?{}", values.len())));
        };
        for word in &query.words {
            add(TEXT, like(word).into(), &mut values);
        }
        for word in &query.excluded {
            add(&format!("NOT {TEXT}"), like(word).into(), &mut values);
        }
        for who in &query.from {
            add("(m.from_name LIKE ? ESCAPE '\\' OR m.from_email LIKE ? ESCAPE '\\')", like(who).into(), &mut values);
        }
        for who in &query.to {
            add(
                "EXISTS (SELECT 1 FROM bodies b WHERE b.account = m.account AND b.path = m.path AND b.uid = m.uid
                         AND (b.to_addr LIKE ? ESCAPE '\\' OR b.cc_list LIKE ? ESCAPE '\\'))",
                like(who).into(),
                &mut values,
            );
        }
        for text in &query.subject {
            add("m.subject LIKE ? ESCAPE '\\'", like(text).into(), &mut values);
        }
        if let Some(day) = query.before {
            add("m.date < ?", local_midnight(day).into(), &mut values);
        }
        if let Some(day) = query.after {
            add("m.date >= ?", local_midnight(day).into(), &mut values);
        }
        if let Some(unread) = query.unread {
            add("m.seen = ?", i64::from(!unread).into(), &mut values);
        }
        if query.flagged {
            add("m.flagged = ?", 1_i64.into(), &mut values);
        }
        if query.attachment {
            add("m.attachments = ?", 1_i64.into(), &mut values);
        }
        if conditions.is_empty() {
            return Ok(Vec::new());
        }
        values.push(i64::try_from(limit).unwrap_or(i64::MAX).into());
        let sql_text = format!(
            "SELECT m.account, m.path, m.uid, m.from_name, m.from_email, m.subject, m.preview, m.date, m.seen, m.flagged,
                    m.answered, m.attachments
             FROM messages m WHERE {} ORDER BY m.date DESC LIMIT ?{}",
            conditions.join(" AND "),
            values.len()
        );
        let rows: Vec<(String, String, Header)> = {
            let mut stmt = self.conn.prepare(&sql_text).map_err(sql)?;
            let mapped = stmt
                .query_map(rusqlite::params_from_iter(values.iter()), |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        Header {
                            uid: r.get(2)?,
                            from_name: r.get(3)?,
                            from_email: r.get(4)?,
                            subject: r.get(5)?,
                            preview: r.get(6)?,
                            date: r.get(7)?,
                            flags: Flags { seen: r.get(8)?, flagged: r.get(9)?, answered: r.get(10)? },
                            has_attachments: r.get(11)?,
                        },
                    ))
                })
                .map_err(sql)?;
            mapped.collect::<Result<Vec<_>, _>>().map_err(sql)?
        };
        Ok(rows.into_iter().map(|(account, path, h)| self.summary(&account, &path, h)).collect())
    }

    /// Ligne de liste pour un en-tête, avec son identifiant.
    pub fn summary(&mut self, account: &str, path: &str, h: Header) -> MessageSummary {
        let id = self.id_for(ImapRef { account: account.to_string(), mailbox: path.to_string(), uid: h.uid });
        MessageSummary {
            id,
            folder_id: format!("{account}:{path}"),
            account_id: account.to_string(),
            from_name: h.from_name,
            from_email: h.from_email,
            subject: h.subject,
            preview: h.preview,
            date: h.date,
            flags: h.flags,
            has_attachments: h.has_attachments,
            uid: h.uid,
        }
    }

    // ------------------------------------------------------------ journal des déplacements

    pub fn journal_save(&mut self, row: &JournalRow) -> Res<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO move_journal (key, account, mailbox, uid, target, message_id, copied)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    i64::try_from(row.key).unwrap_or(i64::MAX),
                    row.source.account,
                    row.source.mailbox,
                    row.source.uid,
                    row.target,
                    row.message_id.clone().unwrap_or_default(),
                    row.copied
                ],
            )
            .map_err(sql)?;
        Ok(())
    }

    pub fn journal_remove(&mut self, key: u64) -> Res<()> {
        self.conn.execute("DELETE FROM move_journal WHERE key = ?1", params![i64::try_from(key).unwrap_or(i64::MAX)]).map_err(sql)?;
        Ok(())
    }

    pub fn journal_rows(&self) -> Res<Vec<JournalRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, account, mailbox, uid, target, message_id, copied FROM move_journal ORDER BY key")
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![], |r| {
                let message_id: String = r.get(5)?;
                Ok(JournalRow {
                    key: u64::try_from(r.get::<_, i64>(0)?).unwrap_or_default(),
                    source: ImapRef { account: r.get(1)?, mailbox: r.get(2)?, uid: r.get(3)? },
                    target: r.get(4)?,
                    message_id: Some(message_id).filter(|m| !m.is_empty()),
                    copied: r.get(6)?,
                })
            })
            .map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    // ------------------------------------------------------------ actions hors connexion

    pub fn pending_push(&mut self, a: &PendingAction) -> Res<()> {
        let uids: Vec<String> = a.uids.iter().map(u32::to_string).collect();
        self.conn
            .execute(
                "INSERT INTO pending_actions (kind, account, mailbox, uids, arg, value)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![a.kind, a.account, a.mailbox, uids.join(","), a.arg, a.value],
            )
            .map_err(sql)?;
        Ok(())
    }

    pub fn pending_actions(&self) -> Res<Vec<PendingAction>> {
        let mut stmt =
            self.conn.prepare("SELECT id, kind, account, mailbox, uids, arg, value FROM pending_actions ORDER BY id").map_err(sql)?;
        let rows = stmt
            .query_map(params![], |r| {
                let uids: String = r.get(4)?;
                Ok(PendingAction {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    account: r.get(2)?,
                    mailbox: r.get(3)?,
                    uids: uids.split(',').filter_map(|u| u.parse().ok()).collect(),
                    arg: r.get(5)?,
                    value: r.get(6)?,
                })
            })
            .map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    pub fn pending_remove(&mut self, id: i64) -> Res<()> {
        self.conn.execute("DELETE FROM pending_actions WHERE id = ?1", params![id]).map_err(sql)?;
        Ok(())
    }

    /// Met en attente la copie d'un message envoyé (dossier « Éléments envoyés »).
    pub fn sent_push(&mut self, copy: &PendingSent) -> Res<()> {
        self.conn
            .execute(
                "INSERT INTO pending_sent (account, mailbox, date, raw) VALUES (?1, ?2, ?3, ?4)",
                params![copy.account, copy.mailbox, copy.date, copy.raw],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// Copies de messages envoyés en attente, dans l'ordre d'envoi.
    pub fn sent_pending(&self) -> Res<Vec<PendingSent>> {
        let mut stmt = self.conn.prepare("SELECT id, account, mailbox, date, raw FROM pending_sent ORDER BY id").map_err(sql)?;
        let rows = stmt
            .query_map(params![], |r| {
                Ok(PendingSent { id: r.get(0)?, account: r.get(1)?, mailbox: r.get(2)?, date: r.get(3)?, raw: r.get(4)? })
            })
            .map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    pub fn sent_remove(&mut self, id: i64) -> Res<()> {
        self.conn.execute("DELETE FROM pending_sent WHERE id = ?1", params![id]).map_err(sql)?;
        Ok(())
    }

    /// Enregistre (ou remplace) la copie de secours d'un brouillon (lot G4).
    pub fn backup_put(&mut self, key: &str, data: &str) -> Res<()> {
        self.conn.execute("INSERT OR REPLACE INTO draft_backups (key, data) VALUES (?1, ?2)", params![key, data]).map_err(sql)?;
        Ok(())
    }

    /// Réglages enregistrés parmi `keys` (audit S8).
    pub fn prefs(&self, keys: &[&str]) -> Res<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare_cached("SELECT value FROM prefs WHERE key = ?1").map_err(sql)?;
        let mut out = Vec::new();
        for key in keys {
            if let Some(value) = stmt.query_row(params![key], |r| r.get::<_, String>(0)).optional().map_err(sql)? {
                out.push(((*key).to_string(), value));
            }
        }
        Ok(out)
    }

    /// Enregistre un réglage, ou le retire (`None`).
    pub fn pref_set(&mut self, key: &str, value: Option<&str>) -> Res<()> {
        match value {
            Some(v) => self.conn.execute("INSERT OR REPLACE INTO prefs (key, value) VALUES (?1, ?2)", params![key, v]),
            None => self.conn.execute("DELETE FROM prefs WHERE key = ?1", params![key]),
        }
        .map_err(sql)?;
        Ok(())
    }

    /// Retire une copie de secours ; `true` si elle existait.
    pub fn backup_remove(&mut self, key: &str) -> Res<bool> {
        Ok(self.conn.execute("DELETE FROM draft_backups WHERE key = ?1", params![key]).map_err(sql)? > 0)
    }

    /// Garde la réponse donnée à une invitation (lot H1).
    pub fn answer_put(&mut self, account: &str, uid: &str, recurrence: &str, answer: &str, sequence: i64) -> Res<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO invitation_answers (account, uid, recurrence, answer, sequence) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![account, uid, recurrence, answer, sequence],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// Réponse donnée à cette version de l'invitation ou à une plus récente ;
    /// `None` si l'organisateur l'a modifiée depuis (nouvelle réponse attendue).
    pub fn answer_for(&self, account: &str, uid: &str, recurrence: &str, sequence: i64) -> Res<Option<String>> {
        self.conn
            .query_row(
                "SELECT answer FROM invitation_answers WHERE account = ?1 AND uid = ?2 AND recurrence = ?3 AND sequence >= ?4",
                params![account, uid, recurrence, sequence],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql)
    }

    /// Note le dossier d'origine de messages envoyés dans « Éléments supprimés »
    /// (par leur `Message-ID`), et oublie les notes de plus de
    /// [`TRASH_ORIGIN_DAYS`] jours.
    pub fn trash_origins_put(&mut self, account: &str, mailbox: &str, message_ids: &[String], now: i64) -> Res<()> {
        let tx = self.conn.savepoint().map_err(sql)?;
        for message_id in message_ids {
            tx.execute(
                "INSERT OR REPLACE INTO trash_origins (account, message_id, mailbox, deleted_at) VALUES (?1, ?2, ?3, ?4)",
                params![account, message_id, mailbox, now],
            )
            .map_err(sql)?;
        }
        let oldest = now.saturating_sub(TRASH_ORIGIN_DAYS.saturating_mul(86_400));
        tx.execute("DELETE FROM trash_origins WHERE deleted_at < ?1", params![oldest]).map_err(sql)?;
        tx.commit().map_err(sql)
    }

    /// Dossiers d'origine notés pour ce `Message-ID`, du plus récent au plus ancien.
    pub fn trash_origins(&self, account: &str, message_id: &str) -> Res<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT mailbox FROM trash_origins WHERE account = ?1 AND message_id = ?2 ORDER BY deleted_at DESC, mailbox")
            .map_err(sql)?;
        let rows = stmt.query_map(params![account, message_id], |r| r.get(0)).map_err(sql)?;
        rows.collect::<Result<Vec<String>, _>>().map_err(sql)
    }

    /// Oublie le dossier d'origine de messages restaurés : (`Message-ID`, dossier).
    pub fn trash_origins_forget(&mut self, account: &str, restored: &[(String, String)]) -> Res<()> {
        let tx = self.conn.savepoint().map_err(sql)?;
        for (message_id, mailbox) in restored {
            tx.execute(
                "DELETE FROM trash_origins WHERE account = ?1 AND message_id = ?2 AND mailbox = ?3",
                params![account, message_id, mailbox],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)
    }

    /// Copies de secours restantes : (clé, copie).
    pub fn backups(&self) -> Res<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT key, data FROM draft_backups ORDER BY key").map_err(sql)?;
        let rows = stmt.query_map(params![], |r| Ok((r.get(0)?, r.get(1)?))).map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    /// Nombre de messages concernés par des actions en attente.
    pub fn pending_count(&self) -> Res<u32> {
        // Compté par SQLite (UID séparés par des virgules), sans relire ni
        // découper chaque action (audit P7).
        let total: i64 = self
            .conn
            .prepare_cached(
                "SELECT (SELECT COALESCE(SUM(CASE WHEN uids = '' THEN 0
                                                  ELSE length(uids) - length(replace(uids, ',', '')) + 1 END), 0)
                         FROM pending_actions)
                      + (SELECT COUNT(*) FROM pending_sent)",
            )
            .map_err(sql)?
            .query_row(params![], |r| r.get(0))
            .map_err(sql)?;
        Ok(u32::try_from(total).unwrap_or(u32::MAX))
    }

    // ------------------------------------------------------------ contacts et agenda

    /// Remplace la liste des carnets / agendas d'un compte.
    pub fn pim_set_collections(&mut self, account: &str, kind: &str, list: &[PimCollection]) -> Res<()> {
        self.revisions.touch_pim();
        let tx = self.conn.savepoint().map_err(sql)?;
        tx.execute("DELETE FROM pim_collections WHERE account = ?1 AND kind = ?2", params![account, kind]).map_err(sql)?;
        for c in list {
            tx.execute(
                "INSERT OR REPLACE INTO pim_collections (account, url, kind, name, color) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![account, c.url, kind, c.name, c.color],
            )
            .map_err(sql)?;
        }
        // Contenu des collections disparues.
        tx.execute(
            "DELETE FROM pim_items WHERE account = ?1 AND kind = ?2
             AND collection NOT IN (SELECT url FROM pim_collections WHERE account = ?1 AND kind = ?2)",
            params![account, kind],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }

    /// Version des carnets, agendas et de leur contenu : change à chaque
    /// écriture (`pim_*`), jamais deux fois la même, même après réouverture.
    #[must_use]
    pub fn pim_revision(&self) -> u64 {
        self.revisions.pim
    }

    /// Version des adresses collectées (lot J13) ; la suppression d'un compte
    /// la change aussi.
    pub fn addresses_revision(&self) -> u64 {
        self.revisions.addresses.max(self.revisions.all)
    }

    /// Adresses collectées pour la complétion (lot J13), tous comptes réunis :
    /// une adresse utilisée depuis deux comptes compte leurs deux envois.
    pub fn collected_addresses(&self) -> Res<Vec<CollectedAddress>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT MAX(email), MAX(name), SUM(sent), MAX(last) FROM collected_addresses GROUP BY key")
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![], |r| Ok(CollectedAddress { email: r.get(0)?, name: r.get(1)?, sent: r.get(2)?, last: r.get(3)? }))
            .map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    /// Note les destinataires d'un envoi de Vortex Mail depuis `account` :
    /// (nom, adresse). Une adresse écartée à laquelle on écrit de nouveau revient.
    pub fn note_sent_to(&mut self, account: &str, recipients: &[(String, String)], when: i64) -> Res<()> {
        if recipients.is_empty() {
            return Ok(());
        }
        self.atomically(|c| collect_addresses(&c.conn, account, recipients, when, false))?;
        self.revisions.touch_addresses();
        Ok(())
    }

    /// Écarte une adresse de la complétion (elle reste dans les messages) ;
    /// ses prochains messages trouvés dans « Envoyés » ne la ramènent pas.
    pub fn forget_address(&mut self, email: &str) -> Res<()> {
        let key = email.trim().to_lowercase();
        self.atomically(|c| {
            c.conn.execute("INSERT OR IGNORE INTO forgotten_addresses (key) VALUES (?1)", params![key]).map_err(sql)?;
            c.conn.execute("DELETE FROM collected_addresses WHERE key = ?1", params![key]).map_err(sql)?;
            Ok(())
        })?;
        self.revisions.touch_addresses();
        Ok(())
    }

    pub fn pim_collections(&self) -> Res<Vec<PimCollection>> {
        let mut stmt =
            self.conn.prepare("SELECT account, url, kind, name, color FROM pim_collections ORDER BY account, name").map_err(sql)?;
        let rows = stmt
            .query_map(params![], |r| {
                Ok(PimCollection { account: r.get(0)?, url: r.get(1)?, kind: r.get(2)?, name: r.get(3)?, color: r.get(4)? })
            })
            .map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    /// Contenu d'une collection tel que relu sur le serveur (remplace l'ancien).
    pub fn pim_replace_items(&mut self, account: &str, kind: &str, collection: &str, items: &[PimItem]) -> Res<()> {
        self.revisions.touch_pim();
        let tx = self.conn.savepoint().map_err(sql)?;
        tx.execute("DELETE FROM pim_items WHERE account = ?1 AND kind = ?2 AND collection = ?3", params![account, kind, collection])
            .map_err(sql)?;
        for i in items {
            tx.execute(
                "INSERT OR REPLACE INTO pim_items (id, kind, account, collection, url, etag, raw, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![i.id, kind, account, collection, i.url, i.etag, i.raw, i.data],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)
    }

    /// Remplace tous les éléments d'un compte en une seule transaction : un
    /// import de sauvegarde interrompu ne vide pas « Sur cet ordinateur » (C2).
    ///
    /// # Errors
    ///
    /// Erreur de la base du cache (la transaction est alors annulée).
    pub fn pim_replace_account(&mut self, account: &str, items: &[(String, String, PimItem)]) -> Res<()> {
        self.revisions.touch_pim();
        let tx = self.conn.savepoint().map_err(sql)?;
        tx.execute("DELETE FROM pim_items WHERE account = ?1", params![account]).map_err(sql)?;
        for (kind, collection, i) in items {
            tx.execute(
                "INSERT OR REPLACE INTO pim_items (id, kind, account, collection, url, etag, raw, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![i.id, kind, account, collection, i.url, i.etag, i.raw, i.data],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)
    }

    pub fn pim_put(&mut self, kind: &str, account: &str, collection: &str, item: &PimItem) -> Res<()> {
        self.revisions.touch_pim();
        self.conn
            .execute(
                "INSERT OR REPLACE INTO pim_items (id, kind, account, collection, url, etag, raw, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![item.id, kind, account, collection, item.url, item.etag, item.raw, item.data],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// Éléments d'un compte (sauvegarde de « Sur cet ordinateur ») : (genre, collection, élément).
    pub fn pim_items_of(&self, account: &str) -> Res<Vec<(String, String, PimItem)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT kind, collection, id, url, etag, raw, data FROM pim_items WHERE account = ?1 ORDER BY id")
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![account], |r| {
                Ok((r.get(0)?, r.get(1)?, PimItem { id: r.get(2)?, url: r.get(3)?, etag: r.get(4)?, raw: r.get(5)?, data: r.get(6)? }))
            })
            .map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    pub fn pim_delete(&mut self, id: &str) -> Res<()> {
        self.revisions.touch_pim();
        self.conn.execute("DELETE FROM pim_items WHERE id = ?1", params![id]).map_err(sql)?;
        Ok(())
    }

    /// Un élément : (compte, collection, contenu).
    pub fn pim_get(&self, id: &str) -> Res<Option<(String, String, PimItem)>> {
        self.conn
            .query_row("SELECT account, collection, id, url, etag, raw, data FROM pim_items WHERE id = ?1", params![id], |r| {
                Ok((r.get(0)?, r.get(1)?, PimItem { id: r.get(2)?, url: r.get(3)?, etag: r.get(4)?, raw: r.get(5)?, data: r.get(6)? }))
            })
            .optional()
            .map_err(sql)
    }

    /// Contenus JSON d'un type (`contacts` ou `calendar`).
    pub fn pim_data(&self, kind: &str) -> Res<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT data FROM pim_items WHERE kind = ?1").map_err(sql)?;
        let rows = stmt.query_map(params![kind], |r| r.get::<_, String>(0)).map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    /// Images distantes autorisées : corps réécrit (images intégrées) et marqué.
    pub fn allow_images(&mut self, id: MessageId, html: &str) -> Res<()> {
        let r = self.imap_ref(id).ok_or_else(|| tr!("message #{id} inconnu", id = id))?;
        self.conn
            .execute(
                "UPDATE bodies SET html = ?4, images = 1 WHERE account = ?1 AND path = ?2 AND uid = ?3",
                params![r.account, r.mailbox, r.uid, html],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// Message chiffré tel que reçu (lot D3a), s'il est gardé.
    pub fn ciphertext(&self, id: MessageId) -> Res<Option<Vec<u8>>> {
        let Some(r) = self.imap_ref(id) else { return Ok(None) };
        self.conn
            .query_row(
                "SELECT ciphertext FROM bodies WHERE account = ?1 AND path = ?2 AND uid = ?3",
                params![r.account, r.mailbox, r.uid],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(sql)
    }

    /// Signature S/MIME examinée de nouveau (JSON, ou [`UNSIGNED`]), pour un
    /// corps gardé avant le lot D1 dont le message n'est pas signé.
    pub fn set_signature(&mut self, id: MessageId, signature: &str) -> Res<()> {
        let r = self.imap_ref(id).ok_or_else(|| tr!("message #{id} inconnu", id = id))?;
        self.conn
            .execute(
                "UPDATE bodies SET signature = ?4 WHERE account = ?1 AND path = ?2 AND uid = ?3",
                params![r.account, r.mailbox, r.uid, signature],
            )
            .map_err(sql)?;
        Ok(())
    }

    // ------------------------------------------------------------ hors connexion

    /// Messages en cache dont le corps n'est pas encore téléchargé (plus récents d'abord).
    pub fn uids_without_body(&self, account: &str, path: &str, limit: usize) -> Res<Vec<u32>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT m.uid FROM messages m WHERE m.account = ?1 AND m.path = ?2
                 AND NOT EXISTS (SELECT 1 FROM bodies b
                                 WHERE b.account = m.account AND b.path = m.path AND b.uid = m.uid)
                 ORDER BY m.uid DESC LIMIT ?3",
            )
            .map_err(sql)?;
        let rows = stmt.query_map(params![account, path, i64::try_from(limit).unwrap_or(i64::MAX)], |r| r.get::<_, u32>(0)).map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    pub fn count_without_body(&self, account: &str, path: &str) -> Res<u32> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM messages m WHERE m.account = ?1 AND m.path = ?2
                 AND NOT EXISTS (SELECT 1 FROM bodies b
                                 WHERE b.account = m.account AND b.path = m.path AND b.uid = m.uid)",
                params![account, path],
                |r| r.get(0),
            )
            .map_err(sql)
    }

    fn body_extra(&self, account: &str, path: &str, uid: u32) -> Res<BodyExtra> {
        self.conn
            .query_row(
                "SELECT to_list, cc_list, reply_to, message_id, refs, files, images, signature, sender_check FROM bodies
                 WHERE account = ?1 AND path = ?2 AND uid = ?3",
                params![account, path, uid],
                |r| {
                    let (to, cc, reply): (String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
                    let files: String = r.get(5)?;
                    let attachments = serde_json::from_str::<Vec<AttachmentInfo>>(&files).ok();
                    Ok(BodyExtra {
                        to_list: split(&to),
                        cc_list: split(&cc),
                        reply_to: split(&reply),
                        // Liste des pièces jointes absente (ancienne version) : à relire.
                        message_id: if attachments.is_some() { r.get(3)? } else { String::new() },
                        references: r.get(4)?,
                        attachments: attachments.unwrap_or_default(),
                        images_allowed: r.get(6)?,
                        signature: r.get(7)?,
                        ciphertext: Vec::new(),
                        sender_check: r.get(8)?,
                    })
                },
            )
            .optional()
            .map(Option::unwrap_or_default)
            .map_err(sql)
    }

    // ------------------------------------------------------------ corps

    /// Corps déjà téléchargé : (destinataires, HTML affichable).
    pub fn body(&self, account: &str, path: &str, uid: u32) -> Res<Option<(String, String)>> {
        self.conn
            .query_row("SELECT to_addr, html FROM bodies WHERE account = ?1 AND path = ?2 AND uid = ?3", params![account, path, uid], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(sql)
    }

    pub fn save_body(&mut self, account: &str, path: &str, uid: u32, to: &str, html: &str, extra: &BodyExtra) -> Res<()> {
        let known = self
            .conn
            .prepare_cached("SELECT 1 FROM bodies WHERE account = ?1 AND path = ?2 AND uid = ?3")
            .map_err(sql)?
            .exists(params![account, path, uid])
            .map_err(sql)?;
        // « - » : message sans Message-ID (distinct d'une ligne d'avant la migration).
        let message_id = if extra.message_id.is_empty() { "-" } else { extra.message_id.as_str() };
        self.conn
            .execute(
                "INSERT OR REPLACE INTO bodies
                   (account, path, uid, to_addr, html, to_list, cc_list, reply_to, message_id, refs, files, signature, ciphertext, sender_check)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    account,
                    path,
                    uid,
                    to,
                    html,
                    join(&extra.to_list),
                    join(&extra.cc_list),
                    join(&extra.reply_to),
                    message_id,
                    extra.references,
                    serde_json::to_string(&extra.attachments).unwrap_or_else(|_| "[]".into()),
                    extra.signature,
                    (!extra.ciphertext.is_empty()).then_some(&extra.ciphertext),
                    extra.sender_check
                ],
            )
            .map_err(sql)?;
        // Message envoyé gardé pour la première fois : ses destinataires
        // entrent dans la complétion, comptés une fois (lot J13).
        if !known {
            let sent: Option<i64> = self
                .conn
                .prepare_cached(
                    "SELECT m.date FROM folders f JOIN messages m ON m.account = f.account AND m.path = f.path
                     WHERE f.account = ?1 AND f.path = ?2 AND f.role = 'sent' AND m.uid = ?3",
                )
                .map_err(sql)?
                .query_row(params![account, path, uid], |r| r.get(0))
                .optional()
                .map_err(sql)?;
            if let Some(date) = sent {
                let recipients = recipients_of(to, &join(&extra.to_list), &join(&extra.cc_list));
                collect_addresses(&self.conn, account, &recipients, date, true)?;
                self.revisions.touch_addresses();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(uid: u32, date: i64, seen: bool) -> Header {
        Header {
            uid,
            from_name: "Marie".into(),
            from_email: "marie@example.com".into(),
            subject: format!("Sujet {uid}"),
            preview: "Bonjour".into(),
            date,
            flags: Flags { seen, flagged: false, answered: false },
            has_attachments: uid == 2,
        }
    }

    #[test]
    fn s8_regles_et_signatures_dans_le_cache() {
        let mut c = Cache::in_memory().unwrap();
        assert_eq!(c.prefs(&["vortex.rules"]).unwrap(), Vec::<(String, String)>::new());
        c.pref_set("vortex.rules", Some("[{\"name\":\"Factures\"}]")).unwrap();
        c.pref_set("vortex.mail", Some("{}")).unwrap();
        c.pref_set("vortex.mail", Some("{\"signatures\":{}}")).unwrap();
        let got = c.prefs(&["vortex.mail", "vortex.rules", "vortex.rules.moved"]).unwrap();
        assert_eq!(
            got,
            vec![
                ("vortex.mail".to_string(), "{\"signatures\":{}}".to_string()),
                ("vortex.rules".into(), "[{\"name\":\"Factures\"}]".into())
            ],
            "la dernière valeur, et seulement les clés présentes"
        );
        c.pref_set("vortex.rules", None).unwrap();
        assert_eq!(c.prefs(&["vortex.rules"]).unwrap().len(), 0, "retirée");
    }

    #[test]
    fn p7_une_transaction_pour_la_synchronisation() {
        let mut c = Cache::in_memory().unwrap();
        c.upsert_headers("a", "INBOX", &[header(1, 10, false), header(2, 20, false)]).unwrap();
        // Échec en cours de route : rien de ce qui précède n'est gardé.
        let failed: Res<()> = c.atomically(|c| {
            c.upsert_headers("a", "INBOX", &[header(3, 30, false)])?;
            c.delete_messages("a", "INBOX", &[1])?;
            Err("coupure".into())
        });
        assert!(failed.is_err());
        assert_eq!(c.cached_uids("a", "INBOX").unwrap(), vec![1, 2], "rien d'écrit");
        // Réussite : tout est gardé, et une transaction suivante reste possible.
        c.atomically(|c| {
            c.upsert_headers("a", "INBOX", &[header(3, 30, false)])?;
            c.update_flags("a", "INBOX", &[(2, Flags { seen: true, ..Flags::default() })])
        })
        .unwrap();
        assert_eq!(c.cached_uids("a", "INBOX").unwrap(), vec![1, 2, 3]);
        assert!(c.header("a", "INBOX", 2).unwrap().unwrap().flags.seen);
        // Transaction laissée ouverte (panique pendant l'écriture) : annulée au passage suivant.
        c.conn.execute_batch("BEGIN").unwrap();
        c.atomically(|c| c.delete_messages("a", "INBOX", &[3])).unwrap();
        assert_eq!(c.cached_uids("a", "INBOX").unwrap(), vec![1, 2]);
    }

    #[test]
    fn p7_drapeaux_reecrits_seulement_s_ils_changent() {
        let mut c = Cache::in_memory().unwrap();
        c.upsert_headers("a", "INBOX", &[header(1, 10, false), header(2, 20, true)]).unwrap();
        let before = c.revision("a", "INBOX");
        let same = [(1, Flags::default()), (2, Flags { seen: true, ..Flags::default() }), (9, Flags::default())];
        c.update_flags("a", "INBOX", &same).unwrap();
        assert_eq!(c.revision("a", "INBOX"), before, "rien de changé : même version de la liste");
        c.update_flags("a", "INBOX", &[(1, Flags { flagged: true, ..Flags::default() })]).unwrap();
        assert!(c.revision("a", "INBOX") > before);
        assert!(c.header("a", "INBOX", 1).unwrap().unwrap().flags.flagged);
    }

    #[test]
    fn p7_actions_en_attente_comptees_par_sqlite() {
        let c = Cache::in_memory().unwrap();
        c.conn
            .execute_batch(
                "INSERT INTO pending_actions (kind, account, mailbox, uids, arg, value) VALUES
                   ('flag', 'a', 'INBOX', '1,2,3', '\\Seen', 1), ('move', 'a', 'INBOX', '7', 'Archive', 0),
                   ('flag', 'a', 'INBOX', '', '\\Seen', 1);",
            )
            .unwrap();
        assert_eq!(c.pending_count().unwrap(), 4);
    }

    #[test]
    fn p1_version_de_la_liste() {
        let mut c = Cache::in_memory().unwrap();
        let start = c.revision("a", "INBOX");
        c.upsert_headers("a", "INBOX", &[header(1, 10, false)]).unwrap();
        let after_new = c.revision("a", "INBOX");
        assert!(after_new > start, "nouveau message : liste changée");
        assert_eq!(c.revision("a", "Archive"), start, "autre dossier inchangé");

        // Rien de neuf : la version ne bouge pas (synchronisation sans nouvelles).
        c.upsert_headers("a", "INBOX", &[]).unwrap();
        c.update_flags("a", "INBOX", &[(1, Flags { seen: false, flagged: false, answered: false })]).unwrap();
        c.delete_messages("a", "INBOX", &[]).unwrap();
        c.set_flag("a", "INBOX", 1, "\\Seen", false).unwrap();
        assert_eq!(c.revision("a", "INBOX"), after_new, "aucune écriture réelle");

        c.set_flag("a", "INBOX", 1, "\\Seen", true).unwrap();
        let after_seen = c.revision("a", "INBOX");
        assert!(after_seen > after_new, "lu : liste changée");
        c.delete_messages("a", "INBOX", &[1]).unwrap();
        assert!(c.revision("a", "INBOX") > after_seen, "supprimé : liste changée");
        c.delete_account("b").unwrap();
        assert!(c.revision("a", "Archive") > start, "compte supprimé : toutes les listes à relire");
    }

    #[test]
    #[ignore = "mesure à la main : cargo test -p vortexmail j6_mesure -- --ignored --nocapture"]
    fn j6_mesure_grande_boite() {
        // Boîte synthétique de 30 000 messages : ce que coûtait chaque cycle
        // avant le lot J6 (lecture et JSON de toute la liste, deux fois),
        // et ce qu'il coûte quand elle n'a pas changé (une version comparée).
        let mut c = Cache::in_memory().unwrap();
        let headers: Vec<Header> = (1..=30_000).map(|uid| header(uid, i64::from(uid), uid % 3 == 0)).collect();
        c.upsert_headers("a", "INBOX", &headers).unwrap();
        let started = std::time::Instant::now();
        let list = c.list("a", "INBOX").unwrap();
        let read = started.elapsed();
        let started = std::time::Instant::now();
        let json = serde_json::to_vec(&list).unwrap();
        let serialized = started.elapsed();
        let started = std::time::Instant::now();
        let revision = c.revision("a", "INBOX");
        let compared = started.elapsed();
        println!(
            "30 000 messages : lecture {read:?}, JSON {serialized:?} ({} Ko) ; liste inchangée : version {revision} lue en {compared:?}",
            json.len() / 1024
        );
        assert_eq!(list.len(), 30_000);
    }

    #[test]
    fn dossier_d_origine_des_messages_supprimes() {
        let mut c = Cache::in_memory().unwrap();
        let ids = vec!["<a@x>".to_string(), "<b@x>".to_string()];
        c.trash_origins_put("a", "Clients", &ids, 1_000_000_000).unwrap();
        assert_eq!(c.trash_origins("a", "<a@x>").unwrap(), ["Clients"]);
        assert_eq!(c.trash_origins("b", "<a@x>").unwrap(), Vec::<String>::new(), "autre compte");
        // Même Message-ID supprimé d'un second dossier : les deux restent notés, le plus récent d'abord.
        c.trash_origins_put("a", "Sent", &ids[..1], 1_000_000_100).unwrap();
        assert_eq!(c.trash_origins("a", "<a@x>").unwrap(), ["Sent", "Clients"]);
        c.trash_origins_forget("a", &[("<a@x>".to_string(), "Sent".to_string())]).unwrap();
        assert_eq!(c.trash_origins("a", "<a@x>").unwrap(), ["Clients"]);
        // Les notes trop anciennes sont oubliées.
        let later = 1_000_000_000 + (TRASH_ORIGIN_DAYS + 1) * 86_400;
        c.trash_origins_put("a", "INBOX", &["<c@x>".to_string()], later).unwrap();
        assert_eq!(c.trash_origins("a", "<b@x>").unwrap(), Vec::<String>::new());
        assert_eq!(c.trash_origins("a", "<c@x>").unwrap(), ["INBOX"]);
        c.delete_account("a").unwrap();
        assert_eq!(c.trash_origins("a", "<c@x>").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn apercus_restes_en_base64_decodes() {
        let mut c = Cache::in_memory().unwrap();
        let mut broken = header(1, 10, false);
        broken.preview = "Qm9uam91ciwgdm9pY2kgbGUgbWVzc2FnZSB0cmFuc2bDqXLDqS4N CkJvbmpvdXIsIHZvaWNpIGxl…".into();
        c.upsert_headers("a", "INBOX", &[broken, header(2, 20, false)]).unwrap();
        repair_base64_previews(&c.conn).unwrap();
        let fixed = c.header("a", "INBOX", 1).unwrap().unwrap();
        assert!(fixed.preview.starts_with("Bonjour, voici le message transf\u{e9}r\u{e9}."), "{}", fixed.preview);
        assert_eq!(c.header("a", "INBOX", 2).unwrap().unwrap().preview, "Bonjour");
    }

    fn folder(account: &str, path: &str, role: FolderRole, parent: Option<&str>, depth: u32) -> Folder {
        Folder {
            id: format!("{account}:{path}"),
            account_id: account.into(),
            name: path.rsplit('/').next().unwrap().into(),
            path: path.into(),
            parent_id: parent.map(|p| format!("{account}:{p}")),
            depth,
            role,
            unread: 1,
            total: 3,
            subscribed: path != "Clients",
        }
    }

    #[test]
    fn folders_roundtrip_and_cleanup() {
        let mut c = Cache::in_memory().unwrap();
        c.set_accounts(&[Account { id: "a".into(), name: "A".into(), email: "a@x".into(), color: "#000".into() }]).unwrap();
        let mut tree = vec![
            folder("a", "INBOX", FolderRole::Inbox, None, 0),
            folder("a", "Clients", FolderRole::Custom, None, 0),
            folder("a", "Clients/Société", FolderRole::Custom, Some("Clients"), 1),
        ];
        tree[0].name = "Boîte de réception".into();
        c.save_folders("a", &tree).unwrap();
        assert_eq!(c.all_folders().unwrap(), tree);

        c.upsert_headers("a", "Clients/Société", &[header(1, 10, true)]).unwrap();
        // Le dossier disparaît du serveur : ses messages quittent le cache.
        c.save_folders("a", &tree[..2]).unwrap();
        assert_eq!(c.cached_uids("a", "Clients/Société").unwrap().len(), 0);
    }

    #[test]
    fn import_interrompu_garde_les_contacts_locaux() {
        // C2 : l'écriture des éléments « Sur cet ordinateur » échoue à mi-chemin.
        let mut c = Cache::in_memory().unwrap();
        let item = |id: &str| PimItem {
            id: id.into(),
            url: String::new(),
            etag: String::new(),
            raw: format!("BEGIN:VCARD {id}"),
            data: String::new(),
        };
        let before = vec![("contacts".to_string(), "local:contacts".to_string(), item("local|marie"))];
        c.pim_replace_account("local", &before).unwrap();
        c.conn
            .execute_batch(
                "CREATE TRIGGER panne BEFORE INSERT ON pim_items WHEN NEW.id = 'local|casse'
                 BEGIN SELECT RAISE(ABORT, 'disque plein'); END;",
            )
            .unwrap();
        let imported = vec![
            ("contacts".to_string(), "local:contacts".to_string(), item("local|sophie")),
            ("contacts".to_string(), "local:contacts".to_string(), item("local|casse")),
        ];
        assert!(c.pim_replace_account("local", &imported).is_err());
        let ids: Vec<String> = c.pim_items_of("local").unwrap().into_iter().map(|(_, _, i)| i.id).collect();
        assert_eq!(ids, ["local|marie"], "rien de l'import, rien de perdu");
    }

    #[test]
    fn noms_des_dossiers_speciaux_dans_la_langue_de_l_interface() {
        // Lot E3 : synchronisé en anglais, relu en français (langue des tests).
        let mut c = Cache::in_memory().unwrap();
        c.set_accounts(&[Account { id: "a".into(), name: "A".into(), email: "a@x".into(), color: "#000".into() }]).unwrap();
        let mut inbox = folder("a", "INBOX", FolderRole::Inbox, None, 0);
        inbox.name = "Inbox".into();
        let mut sent = folder("a", "Sent", FolderRole::Sent, None, 0);
        sent.name = "Sent Items".into();
        c.save_folders("a", &[inbox, sent, folder("a", "Clients", FolderRole::Custom, None, 0)]).unwrap();
        let names: Vec<String> = c.all_folders().unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["Boîte de réception", "Éléments envoyés", "Clients"], "dossier personnel : nom du serveur gardé");
    }

    #[test]
    fn messages_sync_cycle() {
        let mut c = Cache::in_memory().unwrap();
        c.upsert_headers("a", "INBOX", &[header(3, 30, false), header(1, 10, true), header(2, 20, true)]).unwrap();
        let list = c.list("a", "INBOX").unwrap();
        assert_eq!(list.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Sujet 3", "Sujet 2", "Sujet 1"]);
        let id3 = list[0].id;
        assert_eq!(c.list("a", "INBOX").unwrap()[0].id, id3, "identifiant stable");

        c.update_flags("a", "INBOX", &[(3, Flags { seen: true, flagged: true, answered: false })]).unwrap();
        assert_eq!(c.is_seen("a", "INBOX", 3).unwrap(), Some(true));

        // UID 2 supprimé sur le serveur.
        c.delete_messages("a", "INBOX", &[2]).unwrap();
        assert_eq!(c.cached_uids("a", "INBOX").unwrap(), vec![1, 3]);

        let extra = BodyExtra {
            to_list: vec!["moi@x.fr".into(), "toi@y.fr".into()],
            cc_list: vec!["lui@z.fr".into()],
            reply_to: vec![],
            message_id: "<m3@x.fr>".into(),
            references: "<m1@x.fr> <m2@x.fr>".into(),
            images_allowed: false,
            attachments: vec![AttachmentInfo { index: 1, name: "devis.pdf".into(), content_type: "application/pdf".into(), size: 1234 }],
            signature: UNSIGNED.into(),
            ciphertext: Vec::new(),
            sender_check: "dmarc".into(),
        };
        c.save_body("a", "INBOX", 3, "moi@x", "<p>corps</p>", &extra).unwrap();
        assert_eq!(c.message(id3).unwrap().unwrap().sender_check, "dmarc", "lot G1 : gardé avec le corps");
        assert_eq!(c.body("a", "INBOX", 3).unwrap().unwrap().1, "<p>corps</p>");
        let m = c.message(id3).unwrap().expect("lisible hors ligne");
        assert_eq!((m.summary.subject.as_str(), m.to.as_str()), ("Sujet 3", "moi@x"));
        assert!(m.summary.flags.flagged);
        assert_eq!(m.to_list, extra.to_list);
        assert_eq!(m.cc_list, extra.cc_list);
        assert_eq!(m.message_id, "<m3@x.fr>");
        assert_eq!(m.attachments, extra.attachments);
        assert!(!m.images_allowed);
        assert_eq!(m.signature, "", "examiné, non signé");
        c.set_signature(id3, "").unwrap();
        assert_eq!(c.message(id3).unwrap().unwrap().signature, NOT_EXAMINED, "corps d'avant le lot D1");
        c.set_signature(id3, r#"{"verdict":"verified"}"#).unwrap();
        assert_eq!(c.message(id3).unwrap().unwrap().signature, r#"{"verdict":"verified"}"#);
        assert_eq!(c.uids_without_body("a", "INBOX", 10).unwrap(), vec![1]);
        assert_eq!(c.count_without_body("a", "INBOX").unwrap(), 1);
        c.allow_images(id3, "<p>avec images</p>").unwrap();
        let m = c.message(id3).unwrap().unwrap();
        assert!(m.images_allowed);
        assert_eq!(m.body_html, "<p>avec images</p>");
        c.set_flag("a", "INBOX", 3, "\\Answered", true).unwrap();
        assert!(c.header("a", "INBOX", 3).unwrap().unwrap().flags.answered);
        // Message chiffré (lot D3a) : seul le chiffré est gardé, et relu à part.
        assert_eq!(c.ciphertext(id3).unwrap(), None);
        let encrypted = BodyExtra { signature: ENCRYPTED.into(), ciphertext: b"CMS".to_vec(), ..extra.clone() };
        c.save_body("a", "INBOX", 3, "moi@x", "", &encrypted).unwrap();
        assert_eq!(c.ciphertext(id3).unwrap().as_deref(), Some(&b"CMS"[..]));
        assert_eq!(c.message(id3).unwrap().unwrap().signature, ENCRYPTED);
        // Gardé comme pièce `smime.p7m` avant le lot D3a : à examiner de nouveau.
        let old = BodyExtra {
            attachments: vec![AttachmentInfo {
                index: 1,
                name: "smime.p7m".into(),
                content_type: "application/pkcs7-mime".into(),
                size: 9,
            }],
            ..extra
        };
        c.save_body("a", "INBOX", 3, "moi@x", "", &old).unwrap();
        assert_eq!(c.message(id3).unwrap().unwrap().signature, NOT_EXAMINED);
        assert_eq!(c.ciphertext(id3).unwrap(), None);
        c.delete_message("a", "INBOX", 3).unwrap();
        assert!(c.body("a", "INBOX", 3).unwrap().is_none());
        assert!(c.imap_ref(id3).is_none());
    }

    #[test]
    fn pim_items() {
        let mut c = Cache::in_memory().unwrap();
        let book = PimCollection {
            account: "a".into(),
            url: "https://d/ab/".into(),
            kind: "contacts".into(),
            name: "Contacts".into(),
            color: String::new(),
        };
        c.pim_set_collections("a", "contacts", std::slice::from_ref(&book)).unwrap();
        let item =
            PimItem { id: "a|x".into(), url: "https://d/ab/x.vcf".into(), etag: "1".into(), raw: "BEGIN:VCARD".into(), data: "{}".into() };
        c.pim_replace_items("a", "contacts", &book.url, std::slice::from_ref(&item)).unwrap();
        assert_eq!(c.pim_collections().unwrap(), vec![book]);
        assert_eq!(c.pim_data("contacts").unwrap(), vec!["{}"]);
        assert_eq!(c.pim_get("a|x").unwrap().unwrap().2, item);
        c.pim_set_collections("a", "contacts", &[]).unwrap();
        assert!(c.pim_data("contacts").unwrap().is_empty(), "carnet disparu : contenu retiré");
    }

    /// Cache avec une boîte de réception et un dossier « Envoyés ».
    fn with_sent_folder() -> Cache {
        let mut c = Cache::in_memory().unwrap();
        c.set_accounts(&[Account { id: "a".into(), name: "A".into(), email: "a@x".into(), color: "#000".into() }]).unwrap();
        c.save_folders("a", &[folder("a", "INBOX", FolderRole::Inbox, None, 0), folder("a", "Sent", FolderRole::Sent, None, 0)]).unwrap();
        c
    }

    fn sent_extra(to: &[&str], cc: &[&str]) -> BodyExtra {
        BodyExtra {
            to_list: to.iter().map(ToString::to_string).collect(),
            cc_list: cc.iter().map(ToString::to_string).collect(),
            ..BodyExtra::default()
        }
    }

    #[test]
    fn j13_destinataires_des_messages_envoyes_collectes() {
        let mut c = with_sent_folder();
        c.upsert_headers("a", "Sent", &[header(1, 100, true), header(2, 200, true)]).unwrap();
        c.upsert_headers("a", "INBOX", &[header(3, 300, true)]).unwrap();
        let extra = sent_extra(&["paul@y.fr", "lea@z.fr"], &["Marc@W.fr"]);
        c.save_body("a", "Sent", 1, "Paul Martin <paul@y.fr>", "<p>1</p>", &extra).unwrap();
        // Même corps gardé une seconde fois (message relu) : compté une seule fois.
        c.save_body("a", "Sent", 1, "Paul Martin <paul@y.fr>", "<p>1</p>", &extra).unwrap();
        c.save_body("a", "Sent", 2, "paul@y.fr", "<p>2</p>", &sent_extra(&["paul@y.fr"], &[])).unwrap();
        // Message reçu : ses destinataires (nous) ne sont pas collectés.
        c.save_body("a", "INBOX", 3, "a@x", "<p>3</p>", &sent_extra(&["a@x", "autre@v.fr"], &[])).unwrap();
        let mut found = c.collected_addresses().unwrap();
        found.sort_by(|x, y| x.email.cmp(&y.email));
        let summary: Vec<(&str, &str, u32, i64)> = found.iter().map(|a| (a.email.as_str(), a.name.as_str(), a.sent, a.last)).collect();
        assert_eq!(summary, [("Marc@W.fr", "", 1, 100), ("lea@z.fr", "", 1, 100), ("paul@y.fr", "Paul Martin", 2, 200)]);
    }

    #[test]
    fn j13_envoi_note_et_adresse_ecartee() {
        let mut c = with_sent_folder();
        let revision = c.addresses_revision();
        c.note_sent_to("a", &[("Léa".into(), "lea@z.fr".into())], 500).unwrap();
        assert!(c.addresses_revision() > revision, "la complétion relit la liste");
        assert_eq!(c.collected_addresses().unwrap().first().map(|a| (a.name.as_str(), a.sent, a.last)), Some(("Léa", 0, 500)));
        // Écartée : retirée, et sa copie trouvée ensuite dans « Envoyés » ne la ramène pas.
        c.forget_address(" LEA@z.fr ").unwrap();
        assert_eq!(c.collected_addresses().unwrap(), Vec::<CollectedAddress>::new());
        c.upsert_headers("a", "Sent", &[header(1, 600, true)]).unwrap();
        c.save_body("a", "Sent", 1, "lea@z.fr", "<p>1</p>", &sent_extra(&["lea@z.fr"], &[])).unwrap();
        assert_eq!(c.collected_addresses().unwrap(), Vec::<CollectedAddress>::new(), "écartée malgré la copie envoyée");
        // L'utilisateur lui écrit de nouveau : elle revient.
        c.note_sent_to("a", &[(String::new(), "lea@z.fr".into())], 700).unwrap();
        assert_eq!(c.collected_addresses().unwrap().len(), 1, "un nouvel envoi la ramène");
    }

    #[test]
    fn j13_adresses_retirees_avec_leur_compte() {
        let mut c = with_sent_folder();
        c.note_sent_to("a", &[("Paul".into(), "paul@y.fr".into()), (String::new(), "lea@z.fr".into())], 100).unwrap();
        c.note_sent_to("b", &[(String::new(), "Paul@Y.fr".into())], 200).unwrap();
        let summary = |c: &Cache| {
            let mut found: Vec<(String, String, i64)> =
                c.collected_addresses().unwrap().into_iter().map(|a| (a.email.to_lowercase(), a.name, a.last)).collect();
            found.sort();
            found
        };
        assert_eq!(
            summary(&c),
            [("lea@z.fr".to_string(), String::new(), 100), ("paul@y.fr".to_string(), "Paul".to_string(), 200)],
            "une adresse des deux comptes n'apparaît qu'une fois"
        );
        let revision = c.addresses_revision();
        c.delete_account("a").unwrap();
        assert!(c.addresses_revision() > revision, "la complétion relit la liste");
        assert_eq!(summary(&c), [("paul@y.fr".to_string(), String::new(), 200)], "écrite aussi depuis « b » : gardée");
        c.delete_account("b").unwrap();
        assert_eq!(c.collected_addresses().unwrap(), Vec::<CollectedAddress>::new(), "plus aucun compte : plus rien");
    }

    #[test]
    fn j13_historique_repris_a_la_migration() {
        let dir = std::env::temp_dir().join(format!("vortex-j13-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cache.db");
        {
            let mut c = Cache::open(&path).unwrap();
            c.set_accounts(&[Account { id: "a".into(), name: "A".into(), email: "a@x".into(), color: "#000".into() }]).unwrap();
            c.save_folders("a", &[folder("a", "Sent", FolderRole::Sent, None, 0)]).unwrap();
            c.upsert_headers("a", "Sent", &[header(1, 100, true)]).unwrap();
            c.save_body("a", "Sent", 1, "\"Dupont, Marie\" <m@x.fr>", "<p>1</p>", &sent_extra(&["m@x.fr"], &[])).unwrap();
            // Cache de la version 3 : table sans compte, à refaire.
            c.conn
                .execute_batch(
                    "DROP TABLE collected_addresses;
                     CREATE TABLE collected_addresses (key TEXT PRIMARY KEY, email TEXT, name TEXT, sent INTEGER, last INTEGER);
                     PRAGMA user_version = 3;",
                )
                .unwrap();
        }
        let c = Cache::open(&path).unwrap();
        let found = c.collected_addresses().unwrap();
        assert_eq!(found.iter().map(|a| (a.email.as_str(), a.name.as_str(), a.sent)).collect::<Vec<_>>(), [("m@x.fr", "Dupont, Marie", 1)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_items_are_replaced_in_one_block() {
        let mut c = Cache::in_memory().unwrap();
        let item =
            |id: &str| PimItem { id: id.into(), url: String::new(), etag: String::new(), raw: "BEGIN:VCARD".into(), data: "{}".into() };
        c.pim_put("contacts", "local", "local:contacts", &item("local|a")).unwrap();
        c.pim_put("calendar", "local", "local:calendar", &item("local|b")).unwrap();
        c.pim_put("contacts", "autre", "https://d/ab/", &item("autre|x")).unwrap();
        c.pim_replace_account("local", &[("contacts".into(), "local:contacts".into(), item("local|c"))]).unwrap();
        let ids: Vec<String> = c.pim_items_of("local").unwrap().into_iter().map(|(_, _, i)| i.id).collect();
        assert_eq!(ids, vec!["local|c"]);
        assert!(c.pim_get("autre|x").unwrap().is_some(), "les autres comptes ne sont pas touchés");
    }

    #[test]
    fn renamed_folders_keep_their_messages() {
        let mut c = Cache::in_memory().unwrap();
        c.upsert_headers("a", "Clients", &[header(1, 10, true)]).unwrap();
        c.upsert_headers("a", "Clients/Client A", &[header(2, 20, true)]).unwrap();
        c.upsert_headers("a", "ClientsVIP", &[header(3, 30, true)]).unwrap();
        let id = c.list("a", "Clients/Client A").unwrap()[0].id;
        c.rename_path("a", "Clients", "Archives/Clients", '/').unwrap();
        assert_eq!(c.cached_uids("a", "Archives/Clients").unwrap(), vec![1]);
        assert_eq!(c.cached_uids("a", "Archives/Clients/Client A").unwrap(), vec![2]);
        assert_eq!(c.cached_uids("a", "ClientsVIP").unwrap(), vec![3], "nom voisin intact");
        assert_eq!(c.imap_ref(id).unwrap().mailbox, "Archives/Clients/Client A");
    }

    #[test]
    fn b6_renommage_suivi_par_restaurer_et_les_actions_en_attente() {
        // Un message supprimé, un déplacement interrompu ou en attente vers un
        // dossier, une copie d'envoi en attente : tous suivent son renommage.
        let mut c = Cache::in_memory().unwrap();
        c.trash_origins_put("a", "Clients/Client A", &["<m1@x.fr>".into()], 0).unwrap();
        c.sent_push(&PendingSent { id: 0, account: "a".into(), mailbox: "Clients".into(), date: 0, raw: b"x".to_vec() }).unwrap();
        let source = ImapRef { account: "b".into(), mailbox: "INBOX".into(), uid: 4 };
        c.journal_save(&JournalRow { key: 1, source, target: "a:Clients/Client A".into(), message_id: None, copied: false }).unwrap();
        let action = |arg: &str| PendingAction {
            id: 0,
            kind: "move".into(),
            account: "a".into(),
            mailbox: "INBOX".into(),
            uids: vec![4],
            arg: arg.into(),
            value: false,
        };
        c.pending_push(&action("a:Clients")).unwrap();
        c.pending_push(&action("a:ClientsVIP")).unwrap();

        c.rename_path("a", "Clients", "Archives/Clients", '/').unwrap();

        assert_eq!(c.trash_origins("a", "<m1@x.fr>").unwrap(), ["Archives/Clients/Client A"]);
        assert_eq!(c.sent_pending().unwrap()[0].mailbox, "Archives/Clients");
        assert_eq!(c.journal_rows().unwrap()[0].target, "a:Archives/Clients/Client A");
        let args: Vec<String> = c.pending_actions().unwrap().into_iter().map(|a| a.arg).collect();
        assert_eq!(args, ["a:Archives/Clients", "a:ClientsVIP"], "nom voisin intact");
    }

    #[test]
    fn identifiant_provisoire_jamais_reattribue() {
        let mut c = Cache::in_memory().unwrap();
        let provisional = c.fresh_id();
        let real = c.id_for(ImapRef { account: "a".into(), mailbox: "Drafts".into(), uid: 7 });
        assert_ne!(provisional, real, "Y1 : le brouillon et le message ne partagent pas de numéro");
        assert!(c.imap_ref(provisional).is_none());
    }

    #[test]
    fn changement_de_serveur_efface_les_actions_en_attente() {
        let mut c = Cache::in_memory().unwrap();
        let source = ImapRef { account: "a".into(), mailbox: "INBOX".into(), uid: 4 };
        let row = JournalRow { key: 1, source, target: "b:INBOX".into(), message_id: None, copied: true };
        c.journal_save(&row).unwrap();
        let other = ImapRef { account: "b".into(), mailbox: "INBOX".into(), uid: 2 };
        c.journal_save(&JournalRow { key: 2, source: other.clone(), target: "a:Archive".into(), ..row.clone() }).unwrap();
        c.journal_save(&JournalRow { key: 3, source: other, target: "c:INBOX".into(), ..row }).unwrap();
        let action = PendingAction {
            id: 0,
            kind: "move".into(),
            account: "a".into(),
            mailbox: "INBOX".into(),
            uids: vec![4],
            arg: "a:Trash".into(),
            value: true,
        };
        c.pending_push(&action).unwrap();
        c.delete_account("a").unwrap();
        assert_eq!(c.pending_count().unwrap(), 0, "Y6 : actions de l'ancien serveur effacées");
        let keys: Vec<u64> = c.journal_rows().unwrap().iter().map(|r| r.key).collect();
        assert_eq!(keys, vec![3], "seuls les déplacements sans rapport avec le compte restent");
    }

    #[test]
    fn journal_pending_and_search() {
        let mut c = Cache::in_memory().unwrap();
        let source = ImapRef { account: "a".into(), mailbox: "INBOX".into(), uid: 4 };
        let row = JournalRow { key: 3, source, target: "b:INBOX".into(), message_id: Some("<x@y>".into()), copied: false };
        c.journal_save(&row).unwrap();
        c.journal_save(&JournalRow { copied: true, ..row.clone() }).unwrap();
        assert_eq!(c.journal_rows().unwrap(), vec![JournalRow { copied: true, ..row }]);
        c.journal_remove(3).unwrap();
        assert_eq!(c.journal_rows().unwrap().len(), 0);

        let action = PendingAction {
            id: 0,
            kind: "flag".into(),
            account: "a".into(),
            mailbox: "INBOX".into(),
            uids: vec![4, 9],
            arg: "\\Seen".into(),
            value: true,
        };
        c.pending_push(&action).unwrap();
        let stored = c.pending_actions().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].uids, vec![4, 9]);
        assert_eq!(c.pending_count().unwrap(), 2);
        c.pending_remove(stored[0].id).unwrap();
        assert_eq!(c.pending_count().unwrap(), 0);

        c.upsert_headers("a", "INBOX", &[header(1, 10, false)]).unwrap();
        let q = |text: &str| SearchQuery::parse(text).unwrap();
        let found = c.search(&q("sujet 1"), 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].folder_id, "a:INBOX");
        assert_eq!(c.search(&q("100%"), 10).unwrap().len(), 0);
    }

    #[test]
    fn e7_recherche_par_criteres() {
        let mut c = Cache::in_memory().unwrap();
        let mut later = header(3, 2_000_000_000, false);
        later.flags.flagged = true;
        c.upsert_headers("a", "INBOX", &[header(1, 10, false), header(2, 20, true), later]).unwrap();
        let uids = |c: &mut Cache, text: &str| -> Vec<u32> {
            let mut found: Vec<u32> = c.search(&SearchQuery::parse(text).unwrap(), 10).unwrap().iter().map(|m| m.uid).collect();
            found.sort_unstable();
            found
        };
        assert_eq!(uids(&mut c, "est:nonlu"), [1, 3]);
        assert_eq!(uids(&mut c, "est:lu"), [2]);
        assert_eq!(uids(&mut c, "est:suivi"), [3]);
        assert_eq!(uids(&mut c, "avec:pj"), [2]);
        assert_eq!(uids(&mut c, "objet:\"sujet 3\""), [3]);
        assert_eq!(uids(&mut c, "de:marie est:nonlu -\"sujet 1\""), [3]);
        assert_eq!(uids(&mut c, "de:paul"), Vec::<u32>::new());
        assert_eq!(uids(&mut c, "avant:01/01/2000"), [1, 2]);
        assert_eq!(uids(&mut c, "après:2030-01-01"), [3]);
        assert_eq!(uids(&mut c, ""), Vec::<u32>::new(), "sans critère : rien");
        // Les caractères spéciaux de LIKE sont cherchés tels quels.
        assert_eq!(uids(&mut c, "de:_"), Vec::<u32>::new());
    }

    #[test]
    fn uidvalidity_reset() {
        let mut c = Cache::in_memory().unwrap();
        assert_eq!(c.uid_validity("a", "INBOX").unwrap(), None);
        c.set_uid_validity("a", "INBOX", 7).unwrap();
        c.upsert_headers("a", "INBOX", &[header(1, 10, true)]).unwrap();
        c.reset_folder("a", "INBOX").unwrap();
        assert_eq!(c.cached_uids("a", "INBOX").unwrap().len(), 0);
        assert_eq!(c.uid_validity("a", "INBOX").unwrap(), Some(7));
    }

    #[test]
    fn copie_envoyee_en_attente() {
        // Relecture complète n° 11 : la copie d'un message envoyé attend le
        // retour du serveur IMAP, et compte parmi les actions en attente.
        let mut c = Cache::in_memory().unwrap();
        let copy =
            PendingSent { id: 0, account: "a".into(), mailbox: "Sent".into(), date: 1_800_000_000, raw: b"Subject: x\r\n\r\nok".to_vec() };
        c.sent_push(&copy).unwrap();
        assert_eq!(c.pending_count().unwrap(), 1, "comptée en attente");
        let stored = c.sent_pending().unwrap();
        assert_eq!(stored.len(), 1, "une copie");
        assert_eq!((stored[0].mailbox.as_str(), stored[0].raw.as_slice()), ("Sent", copy.raw.as_slice()), "contenu gardé tel quel");
        c.sent_remove(stored[0].id).unwrap();
        assert_eq!(c.pending_count().unwrap(), 0, "retirée après le dépôt");
    }

    #[test]
    fn dossier_renumerote_sans_action_en_attente() {
        // Relecture complète n° 2 : après une renumérotation, une action en
        // attente ou un déplacement interrompu viserait d'autres messages.
        let mut c = Cache::in_memory().unwrap();
        let action = |mailbox: &str| PendingAction {
            id: 0,
            kind: "flag".into(),
            account: "a".into(),
            mailbox: mailbox.into(),
            uids: vec![42],
            arg: "\\Seen".into(),
            value: true,
        };
        c.pending_push(&action("INBOX")).unwrap();
        c.pending_push(&action("Archives")).unwrap();
        let source = ImapRef { account: "a".into(), mailbox: "INBOX".into(), uid: 42 };
        c.journal_save(&JournalRow { key: 1, source, target: "b:INBOX".into(), message_id: None, copied: true }).unwrap();
        c.reset_folder("a", "INBOX").unwrap();
        let left: Vec<String> = c.pending_actions().unwrap().into_iter().map(|a| a.mailbox).collect();
        assert_eq!(left, ["Archives"], "seules les actions des autres dossiers restent");
        assert!(c.journal_rows().unwrap().is_empty(), "déplacement interrompu oublié avec les anciens UID");
    }

    #[test]
    fn modseq_state_follows_uidvalidity() {
        let mut c = Cache::in_memory().unwrap();
        // Sans UIDVALIDITY enregistré : rien n'est mémorisé.
        c.set_modseq_state("a", "INBOX", Some(10), 3).unwrap();
        assert_eq!(c.modseq_state("a", "INBOX").unwrap(), None);

        c.set_uid_validity("a", "INBOX", 7).unwrap();
        assert_eq!(c.modseq_state("a", "INBOX").unwrap(), None);
        c.set_modseq_state("a", "INBOX", Some(715_194_045_007), 3).unwrap();
        assert_eq!(c.modseq_state("a", "INBOX").unwrap(), Some((715_194_045_007, Some(3))));

        // Même UIDVALIDITY : conservé ; autre UIDVALIDITY : oublié.
        c.set_uid_validity("a", "INBOX", 7).unwrap();
        assert_eq!(c.modseq_state("a", "INBOX").unwrap(), Some((715_194_045_007, Some(3))));
        c.set_uid_validity("a", "INBOX", 8).unwrap();
        assert_eq!(c.modseq_state("a", "INBOX").unwrap(), None);
        assert_eq!(c.uid_validity("a", "INBOX").unwrap(), Some(8));

        // Dossier vidé : oublié aussi.
        c.set_modseq_state("a", "INBOX", Some(20), 0).unwrap();
        c.reset_folder("a", "INBOX").unwrap();
        assert_eq!(c.modseq_state("a", "INBOX").unwrap(), None);

        // `None` efface.
        c.set_modseq_state("a", "INBOX", Some(30), 1).unwrap();
        c.set_modseq_state("a", "INBOX", None, 1).unwrap();
        assert_eq!(c.modseq_state("a", "INBOX").unwrap(), None);
    }

    /// Dossier de test propre au test `name`.
    fn g4_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vx-g4-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("cache.sqlite")
    }

    /// Contenu des fichiers de la base (principal et journal), pour y chercher du texte en clair.
    fn g4_bytes(path: &Path) -> Vec<u8> {
        let mut all = std::fs::read(path).unwrap();
        all.extend(std::fs::read(sibling(path, "-wal")).unwrap_or_default());
        all
    }

    fn g4_contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn h1_reponse_gardee_par_version() {
        let mut c = Cache::in_memory().unwrap();
        c.answer_put("a", "uid-1", "", "accepted", 2).unwrap();
        assert_eq!(c.answer_for("a", "uid-1", "", 2).unwrap().as_deref(), Some("accepted"), "même version");
        assert_eq!(c.answer_for("a", "uid-1", "", 1).unwrap().as_deref(), Some("accepted"), "version plus ancienne");
        assert_eq!(c.answer_for("a", "uid-1", "", 3).unwrap(), None, "invitation modifiée depuis");
        assert_eq!(c.answer_for("b", "uid-1", "", 2).unwrap(), None, "autre compte");
        assert_eq!(c.answer_for("a", "uid-1", "20261127T100000", 2).unwrap(), None, "autre occurrence");
        c.answer_put("a", "uid-1", "", "declined", 2).unwrap();
        assert_eq!(c.answer_for("a", "uid-1", "", 2).unwrap().as_deref(), Some("declined"), "réponse changée");
    }

    #[test]
    fn g4_copies_de_secours() {
        let mut c = Cache::in_memory().unwrap();
        c.backup_put("compose-1", "{\"a\":1}").unwrap();
        c.backup_put("principale:7", "{}").unwrap();
        c.backup_put("compose-1", "{\"a\":2}").unwrap();
        assert_eq!(
            c.backups().unwrap(),
            vec![("compose-1".to_string(), "{\"a\":2}".to_string()), ("principale:7".to_string(), "{}".to_string())],
            "une copie par clé, la dernière"
        );
        assert!(c.backup_remove("compose-1").unwrap(), "retirée");
        assert!(!c.backup_remove("compose-1").unwrap(), "déjà retirée");
    }

    #[test]
    fn g4_cle_relue() {
        let key = CacheKey::random().unwrap();
        assert_eq!(CacheKey::from_hex(&key.to_hex()), Some(key.clone()), "aller-retour hexadécimal");
        assert_eq!(key.to_hex().len(), 64, "64 caractères");
        assert!(CacheKey::from_hex("abc").is_none(), "trop courte");
        assert!(CacheKey::from_hex(&"zz".repeat(32)).is_none(), "pas hexadécimale");
        assert_eq!(format!("{key:?}"), "CacheKey(..)", "jamais affichée");
    }

    #[test]
    fn g4_cache_chiffre() {
        let path = g4_file("chiffre");
        let key = CacheKey::random().unwrap();
        {
            let mut c = Cache::open_encrypted(&path, &key).unwrap();
            c.upsert_headers("a", "INBOX", &[header(7, 70, false)]).unwrap();
        }
        assert!(!is_plain(&path), "en-tête chiffré");
        assert!(!g4_contains(&g4_bytes(&path), b"Sujet 7"), "objet absent du fichier");
        let c = Cache::open_encrypted(&path, &key).unwrap();
        assert_eq!(c.cached_uids("a", "INBOX").unwrap(), vec![7], "relu avec la clé");
        drop(c);
        let other = CacheKey::random().unwrap();
        assert!(matches!(Cache::open_encrypted(&path, &other), Err(OpenError::Unreadable)), "autre clé : illisible");
        assert!(Cache::open(&path).is_err(), "sans clé : illisible");
    }

    #[test]
    fn g4_ancien_cache_chiffre_au_passage() {
        let path = g4_file("ancien");
        {
            let mut c = Cache::open(&path).unwrap();
            c.upsert_headers("a", "INBOX", &[header(3, 30, true)]).unwrap();
        }
        assert!(is_plain(&path), "cache d'avant le lot G4 : en clair");
        let key = CacheKey::random().unwrap();
        let c = Cache::open_encrypted(&path, &key).unwrap();
        assert_eq!(c.cached_uids("a", "INBOX").unwrap(), vec![3], "messages repris");
        let version: i64 = c.conn.query_row("PRAGMA user_version", params![], |r| r.get(0)).unwrap();
        assert_eq!(version, 4, "version du schéma reprise (4 depuis le lot J13)");
        drop(c);
        assert!(!is_plain(&path), "fichier remplacé par sa copie chiffrée");
        assert!(!g4_contains(&g4_bytes(&path), b"Sujet 3"), "plus rien en clair");
        assert!(!sibling(&path, "-chiffrement").exists(), "copie provisoire remplacée");
    }
}
