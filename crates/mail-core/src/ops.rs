//! Opérations sur les messages, dont le déplacement entre comptes.
//!
//! Même principe que Thunderbird : quand la source et la destination sont sur
//! le même compte, on délègue au serveur (`MOVE`) ; sinon, message par message,
//! on télécharge le brut, on l'`APPEND` dans la cible et on ne supprime la
//! source **qu'une fois la copie confirmée**. Au pire, un incident laisse un
//! doublon, jamais une perte.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::backend::MailBackend;
use crate::error::{MailError, Result};
use crate::model::{FolderId, FolderRole, MessageId, RawMessage};

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
    /// Opérations en cours dans un appel (déplacement ou reprise) : aucune
    /// autre ne les touche pendant ce temps (audit P4, journal partagé).
    busy: HashSet<u64>,
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
        Self { entries, keys, next_key, store: Some(store), busy: HashSet::new() }
    }

    fn index_of(&self, key: u64) -> Option<usize> {
        self.keys.iter().position(|&k| k == key)
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

    fn push(&mut self, entry: JournalEntry) -> u64 {
        let key = self.next_key.max(1);
        self.entries.push(entry);
        self.keys.push(key);
        self.next_key = key + 1;
        self.persist(self.entries.len() - 1);
        key
    }

    fn set_state(&mut self, key: u64, state: JournalState) {
        let Some(index) = self.index_of(key) else { return };
        let copied_before = matches!(self.entries[index].state, JournalState::Copied { .. });
        self.entries[index].state = state;
        if matches!(self.entries[index].state, JournalState::Failed(_)) && !copied_before {
            if let Some(store) = self.store.as_mut() {
                store.remove(key);
            }
            return;
        }
        self.persist(index);
    }

    fn set_message_id(&mut self, key: u64, message_id: Option<String>) {
        let Some(index) = self.index_of(key) else { return };
        self.entries[index].message_id = message_id;
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

    /// Oublie les opérations terminées ou abandonnées (le journal ne grossit
    /// pas indéfiniment ; audit B8 : les échecs restaient en mémoire).
    fn prune(&mut self) {
        let mut i = 0;
        while i < self.entries.len() {
            let finished = matches!(self.entries[i].state, JournalState::Done { .. } | JournalState::Failed(_));
            if finished && !self.busy.contains(&self.keys[i]) {
                self.entries.remove(i);
                self.keys.remove(i);
            } else {
                i += 1;
            }
        }
    }
}

/// Accès au journal pendant un déplacement : exclusif (`&mut Journal`) ou
/// partagé (`SharedJournal`), où le verrou n'est tenu que le temps d'une
/// écriture, jamais pendant le réseau (audit P4 : un long déplacement entre
/// comptes bloquait toute suppression et tout déplacement des autres comptes).
pub trait JournalAccess {
    /// Commence une opération et la réserve ; `None` si ce message a déjà un
    /// déplacement inachevé (en cours, ou à reprendre) : pas de double copie.
    fn begin(&mut self, entry: JournalEntry) -> Option<u64>;
    /// Nouvelle étape d'une opération.
    fn set(&mut self, key: u64, state: JournalState);
    /// `Message-ID` du message, noté avant la copie.
    fn note_message_id(&mut self, key: u64, message_id: Option<String>);
    /// Le `Message-ID` de l'opération est-il connu ?
    fn has_message_id(&mut self, key: u64) -> bool;
    /// Opérations à reprendre que personne d'autre ne traite, réservées.
    fn claim_unfinished(&mut self) -> Vec<(u64, JournalEntry)>;
    /// Fin du traitement d'une opération réservée.
    fn release(&mut self, key: u64);
    /// Oublie les opérations terminées ou abandonnées.
    fn prune(&mut self);
    /// Ce message a-t-il un déplacement inachevé ?
    fn is_moving(&mut self, source: MessageId) -> bool;
    /// Nombre d'opérations inachevées.
    fn unfinished_count(&mut self) -> usize;
}

impl JournalAccess for Journal {
    fn begin(&mut self, entry: JournalEntry) -> Option<u64> {
        if self.is_moving(entry.source_id) {
            return None;
        }
        let key = self.push(entry);
        self.busy.insert(key);
        Some(key)
    }

    fn set(&mut self, key: u64, state: JournalState) {
        self.set_state(key, state);
    }

    fn note_message_id(&mut self, key: u64, message_id: Option<String>) {
        self.set_message_id(key, message_id);
    }

    fn has_message_id(&mut self, key: u64) -> bool {
        self.index_of(key).is_some_and(|i| self.entries[i].message_id.is_some())
    }

    fn claim_unfinished(&mut self) -> Vec<(u64, JournalEntry)> {
        let claimed: Vec<(u64, JournalEntry)> = self
            .keys
            .iter()
            .zip(&self.entries)
            .filter(|(key, e)| matches!(e.state, JournalState::Pending | JournalState::Copied { .. }) && !self.busy.contains(*key))
            .map(|(&key, e)| (key, e.clone()))
            .collect();
        self.busy.extend(claimed.iter().map(|(key, _)| *key));
        claimed
    }

    fn release(&mut self, key: u64) {
        self.busy.remove(&key);
    }

    fn prune(&mut self) {
        Journal::prune(self);
    }

    fn is_moving(&mut self, source: MessageId) -> bool {
        self.unfinished().any(|e| e.source_id == source)
    }

    fn unfinished_count(&mut self) -> usize {
        self.unfinished().count()
    }
}

/// Journal partagé entre plusieurs déplacements simultanés : chaque accès
/// prend le verrou un instant (audit P4).
#[derive(Debug, Clone, Copy)]
pub struct SharedJournal<'a>(pub &'a Mutex<Journal>);

impl SharedJournal<'_> {
    fn lock(&self) -> MutexGuard<'_, Journal> {
        // Journal modifié une opération à la fois, chaque fois d'un bloc :
        // sûr après une panique (même règle que les verrous du routeur).
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl JournalAccess for SharedJournal<'_> {
    fn begin(&mut self, entry: JournalEntry) -> Option<u64> {
        self.lock().begin(entry)
    }
    fn set(&mut self, key: u64, state: JournalState) {
        self.lock().set(key, state);
    }
    fn note_message_id(&mut self, key: u64, message_id: Option<String>) {
        self.lock().note_message_id(key, message_id);
    }
    fn has_message_id(&mut self, key: u64) -> bool {
        self.lock().has_message_id(key)
    }
    fn claim_unfinished(&mut self) -> Vec<(u64, JournalEntry)> {
        self.lock().claim_unfinished()
    }
    fn release(&mut self, key: u64) {
        self.lock().release(key);
    }
    fn prune(&mut self) {
        JournalAccess::prune(&mut *self.lock());
    }
    fn is_moving(&mut self, source: MessageId) -> bool {
        self.lock().is_moving(source)
    }
    fn unfinished_count(&mut self) -> usize {
        self.lock().unfinished_count()
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
pub fn move_messages<B: MailBackend + ?Sized, J: JournalAccess + ?Sized>(
    backend: &mut B,
    journal: &mut J,
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

/// Message brut à copier vers un autre compte. Un message vide est refusé :
/// déposé dans la cible, il remplacerait l'original, supprimé ensuite (audit B5).
fn fetch_for_copy<B: MailBackend + ?Sized>(backend: &mut B, id: MessageId) -> Result<RawMessage> {
    let raw = backend.fetch_raw(id)?;
    if raw.bytes.is_empty() {
        let reason = crate::lang::pick(
            "contenu du message non reçu, déplacement annulé",
            "message content not received, move cancelled",
            "contenido del mensaje no recibido, traslado cancelado",
        );
        return Err(MailError::Backend(reason.into()));
    }
    Ok(raw)
}

/// Copie fiable d'un message vers un autre compte, puis suppression de la source.
fn move_across_accounts<B: MailBackend + ?Sized, J: JournalAccess + ?Sized>(
    backend: &mut B,
    journal: &mut J,
    id: MessageId,
    target: &str,
) -> Result<MessageId> {
    let entry = JournalEntry { source_id: id, target_folder: target.to_string(), message_id: None, state: JournalState::Pending };
    // Déjà en cours ou à reprendre (autre fenêtre, double clic, reprise) : pas
    // de seconde copie (audit P4 : les déplacements ne sont plus sérialisés).
    let Some(key) = journal.begin(entry) else {
        let reason = crate::lang::pick(
            "déplacement de ce message déjà en cours",
            "this message is already being moved",
            "este mensaje ya se está trasladando",
        );
        return Err(MailError::Backend(reason.into()));
    };
    let result = copy_then_remove(backend, journal, key, id, target);
    journal.release(key);
    result
}

fn copy_then_remove<B: MailBackend + ?Sized, J: JournalAccess + ?Sized>(
    backend: &mut B,
    journal: &mut J,
    key: u64,
    id: MessageId,
    target: &str,
) -> Result<MessageId> {
    // 1. Lecture du message brut avec ses flags et sa date d'origine.
    let raw = fetch_for_copy(backend, id).inspect_err(|e| {
        journal.set(key, JournalState::Failed(e.to_string()));
    })?;
    // Noté avant la copie : à la reprise, il dira si la copie a déjà eu lieu.
    journal.note_message_id(key, crate::compose::header_value(&raw.bytes, "Message-ID"));

    // 2. Dépôt dans le compte cible. Coupure pendant le dépôt : la copie a pu
    //    arriver. Si son `Message-ID` est connu, l'entrée reste « en cours » :
    //    la reprise la cherchera avant de la refaire, sans doublon (relecture
    //    complète n° 18).
    let new_id = backend.append(target, &raw).inspect_err(|e| {
        let unsure = matches!(e, MailError::Network(_)) && journal.has_message_id(key);
        if !unsure {
            journal.set(key, JournalState::Failed(e.to_string()));
        }
    })?;
    journal.set(key, JournalState::Copied { new_id });

    // 3. Suppression de la source, uniquement maintenant que la copie existe.
    //    En cas d'échec, l'entrée reste « copiée » : la reprise (`resume_moves`)
    //    ne supprimera que la source, sans jamais recopier (relecture Y2).
    backend.remove(id)?;
    journal.set(key, JournalState::Done { new_id });

    Ok(new_id)
}

/// Envoie chaque message vers le dossier d'un rôle donné **de son propre
/// compte** (Supprimer → Éléments supprimés, Archiver → Archive).
///
/// # Errors
///
/// Seulement si un déplacement entier échoue ; un message sans dossier de ce
/// rôle dans son compte est listé dans [`MoveReport::failed`].
pub fn move_to_role<B: MailBackend + ?Sized, J: JournalAccess + ?Sized>(
    backend: &mut B,
    journal: &mut J,
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
pub fn resume_moves<B: MailBackend + ?Sized, J: JournalAccess + ?Sized>(backend: &mut B, journal: &mut J) -> MoveReport {
    let mut report = MoveReport::default();
    // Réservées : une autre reprise lancée en même temps ne les refait pas.
    for (key, entry) in journal.claim_unfinished() {
        let id = entry.source_id;
        let mut copied = matches!(entry.state, JournalState::Copied { .. });
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
                journal.set(key, JournalState::Copied { new_id });
                copied = true;
                match backend.remove(id) {
                    Ok(()) | Err(MailError::NotFound(_)) => Ok(new_id),
                    Err(e) => Err(e),
                }
            }
            Ok(None) => match fetch_for_copy(backend, id) {
                Err(MailError::NotFound(_)) => {
                    // Source disparue, copie introuvable : rien à reprendre.
                    journal.set(key, JournalState::Failed("message introuvable".into()));
                    journal.release(key);
                    continue;
                }
                Err(e) => Err(e),
                Ok(raw) => backend.append(&entry.target_folder, &raw).and_then(|new_id| {
                    journal.set(key, JournalState::Copied { new_id });
                    copied = true;
                    backend.remove(id).map(|()| new_id)
                }),
            },
        };
        match result {
            Ok(new_id) => {
                journal.set(key, JournalState::Done { new_id });
                report.moved.push(MovedMessage { old_id: id, new_id });
                report.cross_account += 1;
            }
            Err(e) => {
                // Refus définitif avant toute copie : l'entrée est close, sinon
                // elle serait reprise en vain à chaque passage.
                if !matches!(e, MailError::Network(_)) && !copied {
                    journal.set(key, JournalState::Failed(e.to_string()));
                }
                report.failed.push(FailedMove { id, reason: e.to_string() });
            }
        }
        journal.release(key);
    }
    journal.prune();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p4_journal_partage_sans_double_deplacement() {
        let shared = Mutex::new(Journal::default());
        let entry = JournalEntry { source_id: 1, target_folder: "b:INBOX".into(), message_id: None, state: JournalState::Pending };
        let key = SharedJournal(&shared).begin(entry.clone()).expect("premier déplacement");
        assert!(SharedJournal(&shared).begin(entry.clone()).is_none(), "même message : refusé pendant le premier");
        assert!(SharedJournal(&shared).claim_unfinished().is_empty(), "une reprise ne touche pas un déplacement en cours");
        SharedJournal(&shared).release(key);
        // Interrompu (coupure) : la reprise le réserve une seule fois.
        assert_eq!(SharedJournal(&shared).claim_unfinished().len(), 1);
        assert!(SharedJournal(&shared).claim_unfinished().is_empty(), "déjà réservé par la première reprise");
        assert!(SharedJournal(&shared).begin(entry).is_none(), "toujours à reprendre : pas de nouvelle copie");
        SharedJournal(&shared).set(key, JournalState::Done { new_id: 2 });
        SharedJournal(&shared).release(key);
        JournalAccess::prune(&mut SharedJournal(&shared));
        assert_eq!(SharedJournal(&shared).unfinished_count(), 0);
        assert_eq!(shared.lock().unwrap().entries().len(), 0, "opération finie oubliée");
    }

    #[test]
    fn b8_echecs_oublies_avec_les_operations_finies() {
        let mut journal = Journal::default();
        let entry = |state| JournalEntry { source_id: 1, target_folder: "b:INBOX".into(), message_id: None, state };
        journal.push(entry(JournalState::Done { new_id: 2 }));
        journal.push(entry(JournalState::Failed("refusé".into())));
        journal.push(entry(JournalState::Pending));
        journal.prune();
        assert_eq!(journal.entries.len(), 1, "seule l'opération à reprendre reste");
        assert_eq!(journal.unfinished().count(), 1);
    }
}
