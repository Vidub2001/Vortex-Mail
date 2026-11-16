//! Le trait [`MailBackend`], interface entre les opérations et un magasin de messages.

use crate::model::{Folder, FolderId, FolderRole, MessageId, RawMessage};
use crate::Result;

/// Ce qu'un « magasin » de messages doit savoir faire pour que les opérations
/// de [`crate::ops`] fonctionnent. Implémenté aujourd'hui par
/// [`crate::MemoryStore`], demain par un backend IMAP multi-comptes.
///
/// Correspondance IMAP prévue :
/// - `native_move`  → `UID MOVE` (ou `UID COPY` + `\Deleted` + `UID EXPUNGE`)
/// - `fetch_raw`    → `UID FETCH n (BODY.PEEK[] FLAGS INTERNALDATE)`
/// - `append`       → `APPEND "dossier" (flags) "date" {octets}` + `APPENDUID`
/// - `remove`       → `UID STORE n +FLAGS (\Deleted)` puis `UID EXPUNGE n`
pub trait MailBackend {
    /// Dossier d'identifiant `id`.
    fn folder(&self, id: &str) -> Option<&Folder>;
    /// Dossier qui tient le rôle `role` dans le compte `account_id`.
    fn folder_by_role(&self, account_id: &str, role: FolderRole) -> Option<FolderId>;
    /// Dossier où se trouve un message.
    ///
    /// # Errors
    ///
    /// Message inconnu ([`crate::MailError::NotFound`]).
    fn message_location(&self, id: MessageId) -> Result<FolderId>;

    /// Déplacement à l'intérieur d'un même compte (le serveur fait le travail).
    /// Renvoie les nouveaux identifiants, dans le même ordre que `ids`.
    ///
    /// # Errors
    ///
    /// Message ou dossier inconnu, ou refus du serveur.
    fn native_move(&mut self, ids: &[MessageId], target: &str) -> Result<Vec<MessageId>>;

    /// Contenu complet d'un message, avec ses drapeaux et sa date.
    ///
    /// # Errors
    ///
    /// Message inconnu, ou erreur du serveur.
    fn fetch_raw(&mut self, id: MessageId) -> Result<RawMessage>;
    /// Dépose un message dans un dossier ; rend son nouvel identifiant.
    ///
    /// # Errors
    ///
    /// Dossier inconnu, ou refus du serveur.
    fn append(&mut self, target: &str, raw: &RawMessage) -> Result<MessageId>;
    /// Supprime définitivement un message.
    ///
    /// # Errors
    ///
    /// Message inconnu, ou refus du serveur.
    fn remove(&mut self, id: MessageId) -> Result<()>;

    /// Message de `folder` portant cet en-tête `Message-ID`, s'il existe
    /// (reprise d'un déplacement interrompu). Par défaut : inconnu.
    ///
    /// # Errors
    ///
    /// Dossier inconnu, ou erreur du serveur pendant la recherche.
    fn find_message(&mut self, _folder: &str, _message_id: &str) -> Result<Option<MessageId>> {
        Ok(None)
    }
}
