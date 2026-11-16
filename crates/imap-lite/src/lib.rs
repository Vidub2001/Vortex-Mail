//! Client IMAP minimal, synchrone et sans dépendance.
//!
//! Le transport est n'importe quel flux `Read + Write` : `TcpStream` en clair,
//! ou flux TLS (SSL/TLS d'emblée, ou après [`Client::starttls`]). Le
//! chiffrement est donc fourni par l'appelant, ce qui garde cette crate
//! testable sans réseau ni dépendance.
//!
//! Commandes couvertes : CAPABILITY, STARTTLS, LOGIN, AUTHENTICATE PLAIN, LIST,
//! STATUS, SELECT, (UID) FETCH, UID STORE, UID MOVE (ou COPY + EXPUNGE),
//! APPEND, NOOP, LOGOUT, ENABLE ; synchronisation incrémentale CONDSTORE /
//! QRESYNC (RFC 7162 : `HIGHESTMODSEQ`, `CHANGEDSINCE`, `VANISHED`).

mod base64;
mod client;
mod date;
mod error;
mod lang;
mod limits;
mod reader;
mod response;
pub mod utf7;

pub use client::{Changes, Client, FetchItem, ListEntry, MailboxStatus, SearchKey, SelectInfo, UidSet};
pub use date::{format_internal_date, parse_internal_date};
pub use error::{Error, Result};
pub use lang::set_language;
pub use response::{parse_response, Response, StatusKind, Token};

/// Encode en base64 standard (utilisé aussi par l'authentification SMTP).
#[must_use]
pub fn base64_encode(data: &[u8]) -> String {
    base64::encode(data)
}
