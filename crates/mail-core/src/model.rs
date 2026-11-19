//! Types partagés entre le cœur, la couche Tauri et le frontend.
//! Avec la feature `serde`, ils sont sérialisés en camelCase pour TypeScript.

/// Identifiant de compte, choisi à sa création.
pub type AccountId = String;
/// Identifiant de dossier : `"<compte>:<chemin IMAP>"`, ex. `"pro:Clients/Client A"`.
pub type FolderId = String;
/// Identifiant de message, attribué par le magasin.
pub type MessageId = u64;

/// Rôle d'un dossier (attributs spéciaux IMAP, RFC 6154).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "camelCase"))]
pub enum FolderRole {
    /// Boîte de réception.
    Inbox,
    /// Brouillons.
    Drafts,
    /// Éléments envoyés.
    Sent,
    /// Archives.
    Archive,
    /// Corbeille.
    Trash,
    /// Courrier indésirable.
    Junk,
    /// Dossier créé par l'utilisateur.
    Custom,
}

/// Compte de messagerie.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "camelCase"))]
pub struct Account {
    /// Identifiant choisi à la création du compte, repris dans les `FolderId`.
    pub id: AccountId,
    /// Nom donné au compte dans l'interface (« Pro », « Perso »…).
    pub name: String,
    /// Adresse électronique du compte.
    pub email: String,
    /// Couleur d'identification du compte dans l'interface (#rrggbb).
    pub color: String,
}

/// Dossier d'un compte.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "camelCase"))]
pub struct Folder {
    /// Identifiant (`compte:chemin`).
    pub id: FolderId,
    /// Compte du dossier.
    pub account_id: AccountId,
    /// Nom affiché (dernier segment du chemin).
    pub name: String,
    /// Chemin complet côté serveur, séparateur `/`.
    pub path: String,
    /// Dossier parent, absent à la racine.
    pub parent_id: Option<FolderId>,
    /// Profondeur dans l'arborescence (0 à la racine).
    pub depth: u32,
    /// Rôle (réception, envoyés, corbeille…), `Custom` pour un dossier ordinaire.
    pub role: FolderRole,
    /// Messages non lus.
    pub unread: u32,
    /// Messages au total.
    pub total: u32,
    /// Abonné côté serveur (LSUB). Les dossiers non abonnés n'apparaissent
    /// que dans la vue détaillée par compte, comme dans Thunderbird.
    #[cfg_attr(feature = "serde", serde(default = "yes"))]
    pub subscribed: bool,
}

#[cfg(feature = "serde")]
fn yes() -> bool {
    true
}

/// Drapeaux IMAP d'un message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "camelCase"))]
pub struct Flags {
    /// `\Seen`
    pub seen: bool,
    /// `\Flagged`
    pub flagged: bool,
    /// `\Answered`
    pub answered: bool,
}

/// Message brut tel que l'échange IMAP le manipule :
/// `FETCH BODY.PEEK[] FLAGS INTERNALDATE` d'un côté, `APPEND` de l'autre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMessage {
    /// Contenu RFC 822 complet, en-têtes compris.
    pub bytes: Vec<u8>,
    /// Lu, suivi, répondu.
    pub flags: Flags,
    /// Date de réception côté serveur (timestamp Unix, secondes).
    pub internal_date: i64,
}

/// Ligne d'un message dans la liste.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "camelCase"))]
pub struct MessageSummary {
    /// Identifiant attribué par le magasin ; change si le message est déplacé.
    pub id: MessageId,
    /// Dossier qui contient le message.
    pub folder_id: FolderId,
    /// Compte du dossier.
    pub account_id: AccountId,
    /// Nom affiché de l'expéditeur.
    pub from_name: String,
    /// Adresse de l'expéditeur.
    pub from_email: String,
    /// Objet décodé (RFC 2047).
    pub subject: String,
    /// Début du texte, pour l'aperçu.
    pub preview: String,
    /// Timestamp Unix (secondes).
    pub date: i64,
    /// Lu, suivi, répondu.
    pub flags: Flags,
    /// Vrai si le message a des pièces jointes.
    pub has_attachments: bool,
    /// UID IMAP dans son dossier (0 : inconnu). Distingue deux messages par
    /// ailleurs identiques (règles, relecture complète n° 50).
    pub uid: u32,
}

/// Message complet, pour l'affichage.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "camelCase"))]
pub struct Message {
    /// Ce qu'en montre la liste.
    #[cfg_attr(feature = "serde", serde(flatten))]
    pub summary: MessageSummary,
    /// Destinataires, pour l'affichage.
    pub to: String,
    /// Corps en HTML.
    pub body_html: String,
    /// Adresses des en-têtes `To`, `Cc` et `Reply-To` (pour Répondre / Répondre à tous).
    pub to_list: Vec<String>,
    /// Adresses de l'en-tête `Cc`.
    pub cc_list: Vec<String>,
    /// Adresses de l'en-tête `Reply-To`.
    pub reply_to: Vec<String>,
    /// `Message-ID` et `References`, pour rattacher la réponse au fil de discussion.
    pub message_id: String,
    /// En-tête `References`.
    pub references: String,
    /// Pièces jointes (hors images intégrées au corps).
    pub attachments: Vec<AttachmentInfo>,
    /// Images distantes autorisées (et intégrées au cache) pour ce message.
    pub images_allowed: bool,
    /// Corps nettoyé (liste blanche) pour la citation et les brouillons ;
    /// vide dans le cœur métier, calculé par l'application à la demande.
    #[cfg_attr(feature = "serde", serde(default))]
    pub body_simple: String,
    /// Version texte du corps (mode d'affichage « Texte ») ; vide dans le
    /// cœur métier, calculée par l'application à la demande.
    #[cfg_attr(feature = "serde", serde(default))]
    pub body_text: String,
    /// Signature S/MIME vérifiée, en JSON (lot D1) ; vide si le message n'est
    /// pas signé, `?` s'il n'a pas encore été examiné ; rempli par l'application.
    #[cfg_attr(feature = "serde", serde(default))]
    pub signature: String,
    /// Message chiffré en S/MIME (lot D3a) : vide s'il ne l'est pas, `ok` une
    /// fois déchiffré (en mémoire seulement), sinon pourquoi il ne l'a pas été ;
    /// rempli par l'application.
    #[cfg_attr(feature = "serde", serde(default))]
    pub encryption: String,
    /// Expéditeur que le serveur de réception n'a pas pu authentifier (lot G1) :
    /// vide, `dmarc` ou `spf` ; rempli par l'application.
    #[cfg_attr(feature = "serde", serde(default))]
    pub sender_check: String,
}

/// Pièce jointe d'un message reçu ; le contenu est téléchargé à la demande.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "camelCase"))]
pub struct AttachmentInfo {
    /// Position parmi les pièces jointes du message (référence pour le téléchargement).
    pub index: u32,
    /// Nom du fichier.
    pub name: String,
    /// Type MIME annoncé.
    pub content_type: String,
    /// Taille décodée, en octets.
    pub size: u64,
}
