//! Journal des déplacements entre comptes, enregistré dans la base SQLite :
//! un déplacement interrompu (arrêt, coupure réseau) est repris au démarrage
//! ou au retour du réseau, sans perte ni doublon (voir `mail_core::resume_moves`).

use std::sync::{Arc, Mutex};

use mail_core::{Journal, JournalEntry, JournalState, JournalStore};

use crate::cache::{Cache, JournalRow};
use crate::router::lock_cache;

struct SqlJournal {
    cache: Arc<Mutex<Cache>>,
}

impl JournalStore for SqlJournal {
    fn save(&mut self, key: u64, entry: &JournalEntry) {
        let mut c = lock_cache(&self.cache);
        let Some(source) = c.imap_ref(entry.source_id) else { return };
        let row = JournalRow {
            key,
            source,
            target: entry.target_folder.clone(),
            message_id: entry.message_id.clone(),
            copied: matches!(entry.state, JournalState::Copied { .. }),
        };
        if let Err(e) = c.journal_save(&row) {
            crate::diag::report(&format_args!("journal des déplacements : {e}"));
        }
    }

    fn remove(&mut self, key: u64) {
        if let Err(e) = lock_cache(&self.cache).journal_remove(key) {
            crate::diag::report(&format_args!("journal des déplacements : {e}"));
        }
    }
}

/// Journal persistant, avec les déplacements interrompus lors de la session précédente.
pub(crate) fn load(cache: Arc<Mutex<Cache>>) -> Journal {
    let unfinished = {
        let mut c = lock_cache(&cache);
        let rows = c.journal_rows().unwrap_or_else(|e| {
            crate::diag::report(&format_args!("journal des déplacements illisible : {e}"));
            Vec::new()
        });
        rows.into_iter()
            .map(|row| {
                let source_id = c.id_for(row.source.clone());
                let state = if row.copied { JournalState::Copied { new_id: c.fresh_id() } } else { JournalState::Pending };
                let entry = JournalEntry { source_id, target_folder: row.target, message_id: row.message_id, state };
                (row.key, entry)
            })
            .collect()
    };
    Journal::with_store(Box::new(SqlJournal { cache }), unfinished)
}
