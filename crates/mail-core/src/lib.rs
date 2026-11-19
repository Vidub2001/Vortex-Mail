//! Cœur métier de Vortex Mail, indépendant de l'interface graphique.
//!
//! - [`model`] : types partagés (comptes, dossiers, messages)
//! - [`backend`] : le trait [`MailBackend`] qu'implémentera le futur backend IMAP
//! - [`memory`] : un backend en mémoire avec des données de démonstration
//! - [`ops`] : les opérations, dont le déplacement inter-comptes
//! - [`mime`] : un analyseur MIME minimal (remplacé plus tard par `mail-parser`)
//! - [`mbox`] : boîtes mbox et messages `.eml` (import, export)
//! - [`search`] : le langage de recherche (`de:`, `avant:`, `est:nonlu`…)

mod demo;
mod error;
mod lang;

pub mod backend;
pub mod compose;
pub mod mbox;
pub mod memory;
pub mod mime;
pub mod model;
pub mod ops;
pub mod search;

pub use backend::MailBackend;
pub use compose::Draft;
pub use error::{MailError, Result};
pub use lang::set_language;
pub use memory::MemoryStore;
pub use model::*;
pub use ops::{
    move_messages, move_to_role, resume_moves, FailedMove, Journal, JournalAccess, JournalEntry, JournalState, JournalStore, MoveReport,
    MovedMessage, SharedJournal,
};
