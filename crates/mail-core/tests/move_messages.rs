//! Déplacements de messages, dans un compte et entre comptes, reprise après interruption comprise.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mail_core::{
    move_messages, move_to_role, resume_moves, Flags, FolderId, FolderRole, Journal, JournalEntry, JournalState, JournalStore, MailBackend,
    MailError, MemoryStore, MessageId, RawMessage,
};

const NOW: i64 = 1_800_000_000;

fn store() -> MemoryStore {
    MemoryStore::with_demo_data(NOW)
}

fn find(store: &MemoryStore, folder: &str, subject: &str) -> u64 {
    store
        .list_messages(folder)
        .expect("dossier de démonstration")
        .into_iter()
        .find(|m| m.subject == subject)
        .unwrap_or_else(|| panic!("{subject} absent de {folder}"))
        .id
}

#[test]
fn demo_data_is_consistent() {
    let s = store();
    assert_eq!(s.accounts().len(), 3);
    let inbox = s.folders().into_iter().find(|f| f.id == "perso:INBOX").unwrap();
    assert_eq!(inbox.total, 8);
    assert_eq!(inbox.unread, 2);
    let client_a = s.folders().into_iter().find(|f| f.id == "pro:Clients/Client A").unwrap();
    assert_eq!(client_a.name, "Client A");
    assert_eq!(client_a.depth, 1);
    assert_eq!(client_a.parent_id.as_deref(), Some("pro:Clients"));
    // Tri du plus récent au plus ancien.
    let list = s.list_messages("perso:INBOX").unwrap();
    assert!(list.windows(2).all(|w| w[0].date >= w[1].date));
}

#[test]
fn cross_account_move_keeps_content_flags_and_date() {
    let mut s = store();
    let mut journal = Journal::default();
    let id = find(&s, "perso:INBOX", "Repas de dimanche");
    let before = s.message(id).unwrap();

    let report = move_messages(&mut s, &mut journal, &[id], "pro:Fournisseurs").unwrap();

    assert_eq!(report.cross_account, 1);
    assert_eq!(report.same_account, 0);
    assert!(report.failed.is_empty(), "aucun déplacement en échec : {:?}", report.failed);
    let new_id = report.moved[0].new_id;

    // La source a disparu…
    assert!(s.message(id).is_err());
    // …et la copie est identique, dans l'autre compte.
    let after = s.message(new_id).unwrap();
    assert_eq!(after.summary.folder_id, "pro:Fournisseurs");
    assert_eq!(after.summary.account_id, "pro");
    assert_eq!(after.summary.subject, before.summary.subject);
    assert_eq!(after.summary.date, before.summary.date);
    assert_eq!(after.summary.flags, before.summary.flags);
    assert!(after.summary.flags.flagged);
    assert_eq!(after.body_html, before.body_html);

    assert_eq!(journal.entries().len(), 1);
    assert_eq!(journal.entries()[0].state, JournalState::Done { new_id });
    assert_eq!(journal.unfinished().count(), 0);
}

#[test]
fn same_account_move_uses_native_move() {
    let mut s = store();
    let mut journal = Journal::default();
    let id = find(&s, "pro:INBOX", "Devis pour l'audit d'architecture");

    let report = move_messages(&mut s, &mut journal, &[id], "pro:Clients/Client B").unwrap();

    assert_eq!(report.same_account, 1);
    assert_eq!(report.cross_account, 0);
    assert!(journal.entries().is_empty(), "pas de journal pour un MOVE serveur");
    assert_eq!(s.message(id).unwrap().summary.folder_id, "pro:Clients/Client B");
}

#[test]
fn mixed_selection_from_several_accounts() {
    let mut s = store();
    let mut journal = Journal::default();
    let a = find(&s, "perso:INBOX", "Photos des vacances");
    let b = find(&s, "pro:INBOX", "Renouvellement des licences");
    let c = find(&s, "asso:INBOX", "Assemblée générale annuelle");

    let report = move_messages(&mut s, &mut journal, &[a, b, c], "pro:Archive").unwrap();

    assert_eq!(report.moved.len(), 3);
    assert_eq!(report.same_account, 1);
    assert_eq!(report.cross_account, 2);
    let archive = s.list_messages("pro:Archive").unwrap();
    assert_eq!(archive.len(), 3);
}

#[test]
fn unread_counters_follow_the_message() {
    let mut s = store();
    let mut journal = Journal::default();
    let id = find(&s, "perso:INBOX", "Votre relevé d'octobre est disponible");
    let unread = |s: &MemoryStore, f: &str| s.folders().into_iter().find(|x| x.id == f).unwrap().unread;

    assert_eq!(unread(&s, "perso:INBOX"), 2);
    move_messages(&mut s, &mut journal, &[id], "asso:Newsletters").unwrap();
    assert_eq!(unread(&s, "perso:INBOX"), 1);
    assert_eq!(unread(&s, "asso:Newsletters"), 1);
}

#[test]
fn moving_into_the_same_folder_is_a_noop() {
    let mut s = store();
    let mut journal = Journal::default();
    let id = find(&s, "perso:INBOX", "Repas de dimanche");
    let report = move_messages(&mut s, &mut journal, &[id], "perso:INBOX").unwrap();
    assert!(report.moved.is_empty() && report.failed.is_empty());
    assert!(s.message(id).is_ok());
}

#[test]
fn invalid_target_and_unknown_message() {
    let mut s = store();
    let mut journal = Journal::default();
    assert!(move_messages(&mut s, &mut journal, &[1], "nulle:part").is_err());

    let report = move_messages(&mut s, &mut journal, &[9999], "pro:INBOX").unwrap();
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].id, 9999);
}

#[test]
fn delete_goes_to_each_accounts_own_trash() {
    let mut s = store();
    let mut journal = Journal::default();
    let a = find(&s, "perso:INBOX", "Photos des vacances");
    let b = find(&s, "pro:INBOX", "Renouvellement des licences");

    let report = move_to_role(&mut s, &mut journal, &[a, b], FolderRole::Trash).unwrap();

    assert_eq!(report.moved.len(), 2);
    assert_eq!(report.cross_account, 0, "chaque message reste dans son compte");
    assert_eq!(s.message(a).unwrap().summary.folder_id, "perso:Trash");
    assert_eq!(s.message(b).unwrap().summary.folder_id, "pro:Trash");
}

#[test]
fn backend_trait_lookups() {
    let s = store();
    assert_eq!(s.folder_by_role("asso", FolderRole::Archive).as_deref(), Some("asso:Archive"));
    assert!(s.folder("perso:Factures").is_some());
}

// ------------------------------------------------------------------ reprise après interruption

/// Journal « sur disque » partagé : survit à la destruction du `Journal`.
#[derive(Clone, Default)]
struct Disk(Arc<Mutex<BTreeMap<u64, JournalEntry>>>);

impl JournalStore for Disk {
    fn save(&mut self, key: u64, entry: &JournalEntry) {
        self.0.lock().expect("verrou du journal de test empoisonné").insert(key, entry.clone());
    }
    fn remove(&mut self, key: u64) {
        self.0.lock().expect("verrou du journal de test empoisonné").remove(&key);
    }
}

impl Disk {
    fn reload(&self) -> Journal {
        let saved: Vec<(u64, JournalEntry)> =
            self.0.lock().expect("verrou du journal de test empoisonné").iter().map(|(k, e)| (*k, e.clone())).collect();
        Journal::with_store(Box::new(self.clone()), saved)
    }
    fn len(&self) -> usize {
        self.0.lock().expect("verrou du journal de test empoisonné").len()
    }
}

/// Backend dont la suppression échoue (coupure réseau juste après la copie).
struct Flaky {
    inner: MemoryStore,
    fail_remove: bool,
    /// Le dépôt aboutit côté serveur, mais la réponse se perd (coupure).
    lose_append_reply: bool,
}

impl MailBackend for Flaky {
    fn folder(&self, id: &str) -> Option<&mail_core::Folder> {
        self.inner.folder(id)
    }
    fn folder_by_role(&self, a: &str, r: FolderRole) -> Option<FolderId> {
        self.inner.folder_by_role(a, r)
    }
    fn message_location(&self, id: MessageId) -> mail_core::Result<FolderId> {
        self.inner.message_location(id)
    }
    fn native_move(&mut self, ids: &[MessageId], t: &str) -> mail_core::Result<Vec<MessageId>> {
        self.inner.native_move(ids, t)
    }
    fn fetch_raw(&mut self, id: MessageId) -> mail_core::Result<RawMessage> {
        self.inner.fetch_raw(id)
    }
    fn append(&mut self, t: &str, raw: &RawMessage) -> mail_core::Result<MessageId> {
        let id = self.inner.append(t, raw)?;
        if self.lose_append_reply {
            return Err(MailError::Network("erreur réseau : connexion fermée".into()));
        }
        Ok(id)
    }
    fn remove(&mut self, id: MessageId) -> mail_core::Result<()> {
        if self.fail_remove {
            return Err(MailError::Backend("erreur réseau".into()));
        }
        self.inner.remove(id)
    }
    fn find_message(&mut self, f: &str, mid: &str) -> mail_core::Result<Option<MessageId>> {
        self.inner.find_message(f, mid)
    }
}

fn with_message_id(store: &mut MemoryStore, folder: &str, mid: &str) -> MessageId {
    let raw = RawMessage {
        bytes: format!("From: a@example.com\r\nSubject: Devis\r\nMessage-ID: {mid}\r\n\r\nBonjour").into_bytes(),
        flags: Flags::default(),
        internal_date: NOW,
    };
    store.insert(folder, raw).expect("ajout dans un dossier de démonstration")
}

fn count(store: &MemoryStore, folder: &str) -> usize {
    store.list_messages(folder).expect("dossier de démonstration").len()
}

#[test]
fn interrupted_after_copy_only_deletes_the_source() {
    let mut inner = store();
    let id = with_message_id(&mut inner, "perso:INBOX", "<devis-1@example.com>");
    let (inbox, target) = (count(&inner, "perso:INBOX"), count(&inner, "pro:INBOX"));
    let mut backend = Flaky { inner, fail_remove: true, lose_append_reply: false };
    let disk = Disk::default();
    let mut journal = disk.reload();

    let report = move_messages(&mut backend, &mut journal, &[id], "pro:INBOX").unwrap();
    assert_eq!(report.failed.len(), 1);
    assert_eq!(disk.len(), 1, "copie faite, source à supprimer : reste au journal");
    drop(journal); // « redémarrage »

    backend.fail_remove = false;
    let mut journal = disk.reload();
    let report = resume_moves(&mut backend, &mut journal);
    assert_eq!(report.moved.len(), 1);
    assert_eq!(count(&backend.inner, "perso:INBOX"), inbox - 1);
    assert_eq!(count(&backend.inner, "pro:INBOX"), target + 1, "pas de doublon");
    assert_eq!(disk.len(), 0);
    assert_eq!(journal.unfinished().count(), 0);
}

#[test]
fn interrupted_before_noting_the_copy_does_not_duplicate() {
    let mut inner = store();
    let id = with_message_id(&mut inner, "perso:INBOX", "<devis-2@example.com>");
    // La copie a été déposée, mais l'arrêt est survenu avant que le journal le note.
    let raw = inner.fetch_raw(id).unwrap();
    inner.append("pro:INBOX", &raw).unwrap();
    let target = count(&inner, "pro:INBOX");
    let disk = Disk::default();
    disk.clone().save(
        7,
        &JournalEntry {
            source_id: id,
            target_folder: "pro:INBOX".into(),
            message_id: Some("<devis-2@example.com>".into()),
            state: JournalState::Pending,
        },
    );

    let mut journal = disk.reload();
    let report = resume_moves(&mut inner, &mut journal);
    assert_eq!(report.moved.len(), 1);
    assert_eq!(count(&inner, "pro:INBOX"), target, "copie existante réutilisée");
    assert!(inner.fetch_raw(id).is_err(), "source supprimée");
    assert_eq!(disk.len(), 0);
}

#[test]
fn interrupted_before_the_copy_redoes_the_move() {
    let mut inner = store();
    let id = with_message_id(&mut inner, "perso:INBOX", "<devis-3@example.com>");
    let target = count(&inner, "pro:INBOX");
    let disk = Disk::default();
    disk.clone()
        .save(3, &JournalEntry { source_id: id, target_folder: "pro:INBOX".into(), message_id: None, state: JournalState::Pending });
    let mut journal = disk.reload();
    resume_moves(&mut inner, &mut journal);
    assert_eq!(count(&inner, "pro:INBOX"), target + 1);
    assert!(inner.fetch_raw(id).is_err());
    assert_eq!(disk.len(), 0);

    // Les nouvelles opérations ne réutilisent pas une clé encore enregistrée.
    let id2 = with_message_id(&mut inner, "perso:INBOX", "<devis-4@example.com>");
    let mut flaky = Flaky { inner, fail_remove: true, lose_append_reply: false };
    let mut journal = Journal::with_store(
        Box::new(disk.clone()),
        vec![(9, JournalEntry { source_id: 999, target_folder: "pro:INBOX".into(), message_id: None, state: JournalState::Pending })],
    );
    move_messages(&mut flaky, &mut journal, &[id2], "pro:INBOX").unwrap();
    assert!(disk.0.lock().unwrap().contains_key(&10));
}

/// Relecture Y2 : la suppression de la source échoue (réseau) après la copie.
/// Dans la même session, les nouvelles tentatives ne recopient jamais.
#[test]
fn source_not_removed_is_finished_without_a_second_copy() {
    let mut inner = store();
    let id = with_message_id(&mut inner, "perso:INBOX", "<devis-3@example.com>");
    let (inbox, target) = (count(&inner, "perso:INBOX"), count(&inner, "pro:INBOX"));
    let mut backend = Flaky { inner, fail_remove: true, lose_append_reply: false };
    let mut journal = Journal::default();

    let report = move_messages(&mut backend, &mut journal, &[id], "pro:INBOX").unwrap();
    assert_eq!(report.failed.len(), 1);
    assert_eq!(journal.unfinished().count(), 1, "copie faite : reste à supprimer la source");

    // Réseau toujours instable : la reprise échoue, sans nouvelle copie.
    let again = resume_moves(&mut backend, &mut journal);
    assert_eq!(again.failed.len(), 1);
    assert_eq!(count(&backend.inner, "pro:INBOX"), target + 1, "pas de doublon");

    backend.fail_remove = false;
    let done = resume_moves(&mut backend, &mut journal);
    assert_eq!(done.moved.len(), 1);
    assert_eq!(count(&backend.inner, "perso:INBOX"), inbox - 1);
    assert_eq!(count(&backend.inner, "pro:INBOX"), target + 1, "toujours une seule copie");
    assert_eq!(journal.unfinished().count(), 0);
}

/// Relecture complète n° 18 : la copie arrive, mais la réponse du serveur se
/// perd. La reprise retrouve la copie par son `Message-ID` au lieu d'en déposer
/// une seconde.
#[test]
fn copy_reply_lost_does_not_duplicate() {
    let mut inner = store();
    let id = with_message_id(&mut inner, "perso:INBOX", "<devis-5@example.com>");
    let target = count(&inner, "pro:INBOX");
    let mut backend = Flaky { inner, fail_remove: false, lose_append_reply: true };
    let mut journal = Journal::default();
    let report = move_messages(&mut backend, &mut journal, &[id], "pro:INBOX").expect("rapport de déplacement");
    assert_eq!(report.failed.len(), 1, "le déplacement est signalé comme interrompu");
    assert_eq!(journal.unfinished().count(), 1, "l'entrée reste à reprendre");

    backend.lose_append_reply = false;
    let resumed = resume_moves(&mut backend, &mut journal);
    assert_eq!(resumed.moved.len(), 1, "reprise terminée");
    assert_eq!(count(&backend.inner, "pro:INBOX"), target + 1, "une seule copie");
    assert!(backend.inner.fetch_raw(id).is_err(), "source supprimée");
}

#[test]
fn b5_empty_message_is_never_copied() {
    // Un message vide (corps non reçu du serveur) ne remplace jamais l'original.
    let mut inner = store();
    let raw = RawMessage { bytes: Vec::new(), flags: Flags::default(), internal_date: NOW };
    let id = inner.insert("perso:INBOX", raw).expect("ajout dans un dossier de démonstration");
    let (inbox, target) = (count(&inner, "perso:INBOX"), count(&inner, "pro:INBOX"));
    let mut journal = Journal::default();

    let report = move_messages(&mut inner, &mut journal, &[id], "pro:INBOX").expect("bilan du déplacement");
    assert_eq!(report.failed.len(), 1, "déplacement refusé : {report:?}");
    assert_eq!(count(&inner, "perso:INBOX"), inbox, "original gardé");
    assert_eq!(count(&inner, "pro:INBOX"), target, "rien de déposé");
}
