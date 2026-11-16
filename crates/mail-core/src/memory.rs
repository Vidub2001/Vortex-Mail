//! Backend en mémoire : sert aux données de démo et aux tests.

use std::collections::BTreeMap;

use crate::backend::MailBackend;
use crate::error::{MailError, Result};
use crate::mime::{self, ParsedMail};
use crate::model::{Account, AccountId, Folder, FolderId, FolderRole, Message, MessageId, MessageSummary, RawMessage};

#[derive(Debug, Clone)]
struct StoredMessage {
    folder_id: FolderId,
    raw: RawMessage,
    parsed: ParsedMail,
}

/// Magasin de messages en mémoire : démonstration et tests.
#[derive(Debug, Default)]
pub struct MemoryStore {
    accounts: Vec<Account>,
    folders: Vec<Folder>,
    messages: BTreeMap<MessageId, StoredMessage>,
    next_id: MessageId,
}

impl MemoryStore {
    /// Magasin vide.
    #[must_use]
    pub fn new() -> Self {
        Self { next_id: 1, ..Default::default() }
    }

    /// Ajoute un compte, avec sa couleur d'identification (#rrggbb).
    pub fn add_account(&mut self, id: &str, name: &str, email: &str, color: &str) {
        self.accounts.push(Account { id: id.to_string(), name: name.to_string(), email: email.to_string(), color: color.to_string() });
    }

    /// Ajoute un dossier. Les parents doivent être ajoutés avant leurs enfants :
    /// l'ordre d'insertion est l'ordre d'affichage.
    pub fn add_folder(&mut self, account_id: &str, path: &str, role: FolderRole) -> FolderId {
        let id = format!("{account_id}:{path}");
        let (parent_id, name) = match path.rsplit_once('/') {
            Some((parent, name)) => (Some(format!("{account_id}:{parent}")), name),
            None => (None, path),
        };
        let display = match role {
            FolderRole::Inbox => "Boîte de réception",
            FolderRole::Drafts => "Brouillons",
            FolderRole::Sent => "Éléments envoyés",
            FolderRole::Archive => "Archive",
            FolderRole::Trash => "Éléments supprimés",
            FolderRole::Junk => "Courrier indésirable",
            FolderRole::Custom => name,
        };
        self.folders.push(Folder {
            subscribed: true,
            id: id.clone(),
            account_id: account_id.to_string(),
            name: display.to_string(),
            path: path.to_string(),
            parent_id,
            depth: u32::try_from(path.matches('/').count()).unwrap_or(u32::MAX),
            role,
            unread: 0,
            total: 0,
        });
        id
    }

    /// Ajoute un message dans un dossier ; rend son identifiant.
    ///
    /// # Errors
    ///
    /// Dossier inconnu ([`MailError::NotFound`]).
    pub fn insert(&mut self, folder_id: &str, raw: RawMessage) -> Result<MessageId> {
        if !self.folders.iter().any(|f| f.id == folder_id) {
            return Err(MailError::NotFound(folder_id.to_string()));
        }
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        let parsed = mime::parse(&raw.bytes);
        self.messages.insert(id, StoredMessage { folder_id: folder_id.to_string(), raw, parsed });
        Ok(id)
    }

    /// Copie des comptes, dans l'ordre d'ajout.
    #[must_use]
    pub fn accounts(&self) -> Vec<Account> {
        self.accounts.clone()
    }

    /// Dossiers avec leurs compteurs à jour.
    #[must_use]
    pub fn folders(&self) -> Vec<Folder> {
        self.folders
            .iter()
            .map(|f| {
                let mut f = f.clone();
                for m in self.messages.values().filter(|m| m.folder_id == f.id) {
                    f.total += 1;
                    if !m.raw.flags.seen {
                        f.unread += 1;
                    }
                }
                f
            })
            .collect()
    }

    /// Messages d'un dossier, du plus récent au plus ancien.
    ///
    /// # Errors
    ///
    /// Dossier inconnu ([`MailError::NotFound`]).
    pub fn list_messages(&self, folder_id: &str) -> Result<Vec<MessageSummary>> {
        let folder = self.folders.iter().find(|f| f.id == folder_id).ok_or_else(|| MailError::NotFound(folder_id.to_string()))?;
        let mut list: Vec<MessageSummary> =
            self.messages.iter().filter(|(_, m)| m.folder_id == folder.id).map(|(id, m)| summary(*id, m, &folder.account_id)).collect();
        list.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
        Ok(list)
    }

    /// Message complet, pour l'affichage.
    ///
    /// # Errors
    ///
    /// Message inconnu ([`MailError::NotFound`]).
    pub fn message(&self, id: MessageId) -> Result<Message> {
        let m = self.get(id)?;
        let account = self.account_of_folder(&m.folder_id)?;
        Ok(Message {
            summary: summary(id, m, &account),
            to: m.parsed.to.clone(),
            body_html: m.parsed.body_html.clone(),
            to_list: crate::compose::parse_address_list(&m.parsed.to),
            cc_list: Vec::new(),
            reply_to: Vec::new(),
            message_id: String::new(),
            references: String::new(),
            attachments: Vec::new(),
            images_allowed: false,
            body_simple: String::new(),
            body_text: String::new(),
            signature: String::new(),
            encryption: String::new(),
            sender_check: String::new(),
        })
    }

    fn get(&self, id: MessageId) -> Result<&StoredMessage> {
        self.messages.get(&id).ok_or_else(|| MailError::NotFound(format!("message #{id}")))
    }

    fn account_of_folder(&self, folder_id: &str) -> Result<AccountId> {
        self.folders
            .iter()
            .find(|f| f.id == folder_id)
            .map(|f| f.account_id.clone())
            .ok_or_else(|| MailError::NotFound(folder_id.to_string()))
    }
}

fn summary(id: MessageId, m: &StoredMessage, account_id: &str) -> MessageSummary {
    MessageSummary {
        id,
        folder_id: m.folder_id.clone(),
        account_id: account_id.to_string(),
        from_name: m.parsed.from_name.clone(),
        from_email: m.parsed.from_email.clone(),
        subject: m.parsed.subject.clone(),
        preview: mime::preview(&m.parsed.body_html, 140),
        date: m.raw.internal_date,
        flags: m.raw.flags,
        has_attachments: m.parsed.has_attachments,
        uid: 0,
    }
}

impl MailBackend for MemoryStore {
    fn folder(&self, id: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    fn folder_by_role(&self, account_id: &str, role: FolderRole) -> Option<FolderId> {
        self.folders.iter().find(|f| f.account_id == account_id && f.role == role).map(|f| f.id.clone())
    }

    fn message_location(&self, id: MessageId) -> Result<FolderId> {
        self.get(id).map(|m| m.folder_id.clone())
    }

    fn native_move(&mut self, ids: &[MessageId], target: &str) -> Result<Vec<MessageId>> {
        if self.folder(target).is_none() {
            return Err(MailError::NotFound(target.to_string()));
        }
        for id in ids {
            self.messages.get_mut(id).ok_or_else(|| MailError::NotFound(format!("message #{id}")))?.folder_id = target.to_string();
        }
        // En IMAP, MOVE attribue de nouveaux UID (réponse COPYUID) ;
        // en mémoire, les identifiants sont conservés.
        Ok(ids.to_vec())
    }

    fn fetch_raw(&mut self, id: MessageId) -> Result<RawMessage> {
        self.get(id).map(|m| m.raw.clone())
    }

    fn append(&mut self, target: &str, raw: &RawMessage) -> Result<MessageId> {
        self.insert(target, raw.clone())
    }

    fn remove(&mut self, id: MessageId) -> Result<()> {
        self.messages.remove(&id).map(|_| ()).ok_or_else(|| MailError::NotFound(format!("message #{id}")))
    }

    fn find_message(&mut self, folder: &str, message_id: &str) -> Result<Option<MessageId>> {
        Ok(self
            .messages
            .iter()
            .filter(|(_, m)| m.folder_id == folder)
            .find(|(_, m)| crate::compose::header_value(&m.raw.bytes, "Message-ID").as_deref() == Some(message_id))
            .map(|(id, _)| *id))
    }
}
