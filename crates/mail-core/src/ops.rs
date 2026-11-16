//! Opérations sur les messages, dont le déplacement entre comptes.
//!
//! Même principe que Thunderbird : quand la source et la destination sont sur
//! le même compte, on délègue au serveur (`MOVE`) ; sinon, message par message,
//! on télécharge le brut, on l'`APPEND` dans la cible et on ne supprime la
//! source **qu'une fois la copie confirmée**. Au pire, un incident laisse un
//! doublon, jamais une perte.

use crate::backend::MailBackend;
use crate::error::{MailError, Result};
use crate::model::{FolderId, FolderRole, MessageId};

/// Avancement d'un déplacement journalisé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalState {
    /// Opération enregistrée, rien n'est encore fait.
    Pending,
    /// Copie présente dans la cible, source pas encore supprimée.
    Copied {
        /// Identifiant de la copie.
        new_id: MessageId,
    },
    /// Terminé.
    Done {
        /// Identifiant du message dans le dossier de destination.
        new_id: MessageId,
    },
    /// Abandonné, avec la raison.
    Failed(String),
}

/// Déplacement journalisé, repris s'il a été interrompu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    /// Message à déplacer.
    pub source_id: MessageId,
    /// Dossier de destination.
    pub target_folder: FolderId,
    /// `Message-ID` du message déplacé : permet, à la reprise, de savoir si la
    /// copie a déjà été déposée (et d'éviter un doublon).
    pub message_id: Option<String>,
    /// Étape atteinte : c'est elle qui dit quoi refaire à la reprise.
    pub state: JournalState,
}

/// Mémoire durable du journal (base `SQLite` de l'application) : ce qui y reste
/// au démarrage est une opération interrompue, à reprendre.
pub trait JournalStore: Send {
    /// Enregistre ou met à jour une opération en cours.
    fn save(&mut self, key: u64, entry: &JournalEntry);
    /// L'opération est terminée (ou n'a rien modifié) : plus rien à reprendre.
    fn remove(&mut self, key: u64);
}

/// Journal des déplacements inter-comptes.
///
/// Chaque étape est enregistrée avant d'être franchie. Avec un
/// [`JournalStore`], une opération interrompue (arrêt, coupure réseau) est
/// reprise par [`resume_moves`] : `Pending` → on vérifie si la copie existe
/// déjà avant de la refaire, `Copied` → on supprime seulement la source.
#[derive(Default)]
pub struct Journal {
    entries: Vec<JournalEntry>,
    keys: Vec<u64>,
    next_key: u64,
    store: Option<Box<dyn JournalStore>>,
}

impl std::fmt::Debug for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Journal").field("entries", &self.entries).field("persistent", &self.store.is_some()).finish_non_exhaustive()
    }
}

impl Journal {
    /// Journal persistant ; `unfinished` : opérations interrompues relues au démarrage.
    #[must_use]
    pub fn with_store(store: Box<dyn JournalStore>, unfinished: Vec<(u64, JournalEntry)>) -> Self {
        let next_key = unfinished.iter().map(|(k, _)| k + 1).max().unwrap_or(1);
        let (keys, entries) = unfinished.into_iter().unzip();
        Self { entries, keys, next_key, store: Some(store) }
    }

    fn persist(&mut self, index: usize) {
        let (Some(store), Some(entry), Some(&key)) = (self.store.as_mut(), self.entries.get(index), self.keys.get(index)) else {
            return;
        };
        match &entry.state {
            JournalState::Pending | JournalState::Copied { .. } => store.save(key, entry),
            JournalState::Done { .. } => store.remove(key),
            // Échec avant la copie : rien n'a changé, rien à reprendre. Un échec
            // après la copie garde l'état `Copied` enregistré : la suppression de
            // la source sera retentée.
            JournalState::Failed(_) => {}
        }
    }

    fn push(&mut self, entry: JournalEntry) -> usize {
        self.entries.push(entry);
        self.keys.push(self.next_key.max(1));
        self.next_key = self.next_key.max(1) + 1;
        let index = self.entries.len() - 1;
        self.persist(index);
        index
    }

    fn set(&mut self, index: usize, state: JournalState) {
        let copied_before = matches!(self.entries.get(index).map(|e| &e.state), Some(JournalState::Copied { .. }));
        if let Some(e) = self.entries.get_mut(index) {
            e.state = state;
        }
        let failed = matches!(self.entries.get(index).map(|e| &e.state), Some(JournalState::Failed(_)));
        if failed && !copied_before {
            if let (Some(store), Some(&key)) = (self.store.as_mut(), self.keys.get(index)) {
                store.remove(key);
            }
            return;
        }
        self.persist(index);
    }

    fn note_message_id(&mut self, index: usize, message_id: Option<String>) {
        if let Some(e) = self.entries.get_mut(index) {
            e.message_id = message_id;
        }
        self.persist(index);
    }

    /// Opérations du journal.
    #[must_use]
    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    /// Opérations à reprendre.
    pub fn unfinished(&self) -> impl Iterator<Item = &JournalEntry> {
        self.entries.iter().filter(|e| matches!(e.state, JournalState::Pending | JournalState::Copied { .. }))
    }

    /// Oublie les opérations terminées (le journal ne grossit pas indéfiniment).
    fn prune(&mut self) {
        let mut i = 0;
        while i < self.entries.len() {
            if matches!(self.entries[i].state, JournalState::Done { .. }) {
                self.entries.remove(i);
                self.keys.remove(i);
            } else {
                i += 1;
            }
        }
    }
}

/// Message déplacé.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "camelCase"))]
pub struct MovedMessage {
    /// Identifiant avant le déplacement.
    pub old_id: MessageId,
    /// Identifiant dans le dossier de destination.
    pub new_id: MessageId,
}

/// Message qui n'a pas pu être déplacé.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "camelCase"))]
pub struct FailedMove {
    /// Message resté à sa place.
    pub id: MessageId,
    /// Raison, lisible par l'utilisateur.
    pub reason: String,
}

/// Bilan d'un déplacement de plusieurs messages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "camelCase"))]
pub struct MoveReport {
    /// Messages déplacés.
    pub moved: Vec<MovedMessage>,
    /// Messages en échec.
    pub failed: Vec<FailedMove>,
    /// Nombre de messages déplacés par le serveur (même compte).
    pub same_account: u32,
    /// Nombre de messages copiés puis supprimés (entre comptes).
    pub cross_account: u32,
    /// Hors connexion : déplacements mis en attente, faits au retour du réseau.
    #[cfg_attr(feature = "serde", serde(default))]
    pub queued: u32,
}

impl MoveReport {
    fn merge(&mut self, other: MoveReport) {
        self.moved.extend(other.moved);
        self.failed.extend(other.failed);
        self.same_account += other.same_account;
        self.cross_account += other.cross_account;
        self.queued += other.queued;
    }
}

/// Déplace des messages (éventuellement issus de plusieurs dossiers et comptes)
/// vers un dossier de n'importe quel compte.
///
/// Un échec sur un message n'interrompt pas les autres : il est listé dans
/// [`MoveReport::failed`]. Seule une destination invalide fait échouer l'appel.
///
/// # Errors
///
/// Destination inexistante ([`MailError::InvalidTarget`]).
pub fn move_messages<B: MailBackend + ?Sized>(
    backend: &mut B,
    journal: &mut Journal,
    ids: &[MessageId],
    target: &str,
) -> Result<MoveReport> {
    let target_folder = backend.folder(target).cloned().ok_or_else(|| MailError::InvalidTarget(target.to_string()))?;

    let mut report = MoveReport::default();
    let mut same_account: Vec<MessageId> = Vec::new();

    for &id in ids {
        let source = match backend.message_location(id) {
            Ok(folder_id) => folder_id,
            Err(e) => {
                report.failed.push(FailedMove { id, reason: e.to_string() });
                continue;
            }
        };
        if source == target_folder.id {
            continue; // déjà au bon endroit
        }
        let Some(source_account) = backend.folder(&source).map(|f| f.account_id.clone()) else {
            report.failed.push(FailedMove { id, reason: MailError::NotFound(source).to_string() });
            continue;
        };

        if source_account == target_folder.account_id {
            same_account.push(id);
        } else {
            match move_across_accounts(backend, journal, id, &target_folder.id) {
                Ok(new_id) => {
                    report.moved.push(MovedMessage { old_id: id, new_id });
                    report.cross_account += 1;
                }
                Err(e) => report.failed.push(FailedMove { id, reason: e.to_string() }),
            }
        }
    }

    if !same_account.is_empty() {
        match backend.native_move(&same_account, &target_folder.id) {
            Ok(new_ids) => {
                for (old_id, new_id) in same_account.iter().copied().zip(new_ids) {
                    report.moved.push(MovedMessage { old_id, new_id });
                }
                report.same_account += u32::try_from(same_account.len()).unwrap_or(u32::MAX);
            }
            Err(e) => {
                let reason = e.to_string();
                report.failed.extend(same_account.into_iter().map(|id| FailedMove { id, reason: reason.clone() }));
            }
        }
    }

    Ok(report)
}

/// Copie fiable d'un message vers un autre compte, puis suppression de la source.
fn move_across_accounts<B: MailBackend + ?Sized>(backend: &mut B, journal: &mut Journal, id: MessageId, target: &str) -> Result<MessageId> {
    let entry =
        journal.push(JournalEntry { source_id: id, target_folder: target.to_string(), message_id: None, state: JournalState::Pending });

    // 1. Lecture du message brut avec ses flags et sa date d'origine.
    let raw = backend.fetch_raw(id).inspect_err(|e| {
        journal.set(entry, JournalState::Failed(e.to_string()));
    })?;
    // Noté avant la copie : à la reprise, il dira si la copie a déjà eu lieu.
    journal.note_message_id(entry, crate::compose::header_value(&raw.bytes, "Message-ID"));

    // 2. Dépôt dans le compte cible. Coupure pendant le dépôt : la copie a pu
    //    arriver. Si son `Message-ID` est connu, l'entrée reste « en cours » :
    //    la reprise la cherchera avant de la refaire, sans doublon (relecture
    //    complète n° 18).
    let new_id = backend.append(target, &raw).inspect_err(|e| {
        let unsure = matches!(e, MailError::Network(_)) && journal.entries.get(entry).is_some_and(|x| x.message_id.is_some());
        if !unsure {
            journal.set(entry, JournalState::Failed(e.to_string()));
        }
    })?;
    journal.set(entry, JournalState::Copied { new_id });

    // 3. Suppression de la source, uniquement maintenant que la copie existe.
    //    En cas d'échec, l'entrée reste « copiée » : la reprise (`resume_moves`)
    //    ne supprimera que la source, sans jamais recopier (relecture Y2).
    backend.remove(id)?;
    journal.set(entry, JournalState::Done { new_id });

    Ok(new_id)
}

/// Envoie chaque message vers le dossier d'un rôle donné **de son propre
/// compte** (Supprimer → Éléments supprimés, Archiver → Archive).
///
/// # Errors
///
/// Seulement si un déplacement entier échoue ; un message sans dossier de ce
/// rôle dans son compte est listé dans [`MoveReport::failed`].
pub fn move_to_role<B: MailBackend + ?Sized>(
    backend: &mut B,
    journal: &mut Journal,
    ids: &[MessageId],
    role: FolderRole,
) -> Result<MoveReport> {
    // Regroupe les messages par dossier cible, en conservant l'ordre.
    let mut groups: Vec<(FolderId, Vec<MessageId>)> = Vec::new();
    let mut report = MoveReport::default();

    for &id in ids {
        let target = backend
            .message_location(id)
            .and_then(|source| backend.folder(&source).map(|f| f.account_id.clone()).ok_or(MailError::NotFound(source)))
            .and_then(|account| {
                backend.folder_by_role(&account, role).ok_or_else(|| MailError::InvalidTarget(format!("{role:?} absent de {account}")))
            });
        match target {
            Ok(folder) => match groups.iter_mut().find(|(f, _)| *f == folder) {
                Some((_, list)) => list.push(id),
                None => groups.push((folder, vec![id])),
            },
            Err(e) => report.failed.push(FailedMove { id, reason: e.to_string() }),
        }
    }

    for (folder, list) in groups {
        report.merge(move_messages(backend, journal, &list, &folder)?);
    }
    Ok(report)
}

/// Reprend les déplacements inter-comptes interrompus (au démarrage, ou au
/// retour du réseau), sans perte ni doublon :
/// - copie déjà faite (`Copied`, ou `Message-ID` présent dans la cible) :
///   on supprime seulement la source ;
/// - sinon on refait le déplacement complet ;
/// - source introuvable et copie absente : rien à faire, l'entrée est oubliée.
///
/// Une opération qui échoue encore (réseau toujours absent) reste au journal.
pub fn resume_moves<B: MailBackend + ?Sized>(backend: &mut B, journal: &mut Journal) -> MoveReport {
    let mut report = MoveReport::default();
    let pending: Vec<usize> = (0..journal.entries.len())
        .filter(|&i| matches!(journal.entries[i].state, JournalState::Pending | JournalState::Copied { .. }))
        .collect();
    for index in pending {
        let entry = journal.entries[index].clone();
        let id = entry.source_id;
        let copy = match entry.state {
            JournalState::Copied { new_id } => Ok(Some(new_id)),
            _ => match entry.message_id.as_deref() {
                Some(mid) => backend.find_message(&entry.target_folder, mid),
                None => Ok(None),
            },
        };
        let result = match copy {
            Err(e) => Err(e),
            Ok(Some(new_id)) => {
                journal.set(index, JournalState::Copied { new_id });
                match backend.remove(id) {
                    Ok(()) | Err(MailError::NotFound(_)) => Ok(new_id),
                    Err(e) => Err(e),
                }
            }
            Ok(None) => match backend.fetch_raw(id) {
                Err(MailError::NotFound(_)) => {
                    // Source disparue, copie introuvable : rien à reprendre.
                    journal.set(index, JournalState::Failed("message introuvable".into()));
                    continue;
                }
                Err(e) => Err(e),
                Ok(raw) => backend.append(&entry.target_folder, &raw).and_then(|new_id| {
                    journal.set(index, JournalState::Copied { new_id });
                    backend.remove(id).map(|()| new_id)
                }),
            },
        };
        match result {
            Ok(new_id) => {
                journal.set(index, JournalState::Done { new_id });
                report.moved.push(MovedMessage { old_id: id, new_id });
                report.cross_account += 1;
            }
            Err(e) => {
                // Refus définitif avant toute copie : l'entrée est close, sinon
                // elle serait reprise en vain à chaque passage.
                if !matches!(e, MailError::Network(_)) && matches!(journal.entries[index].state, JournalState::Pending) {
                    journal.set(index, JournalState::Failed(e.to_string()));
                }
                report.failed.push(FailedMove { id, reason: e.to_string() });
            }
        }
    }
    journal.prune();
    report
}
