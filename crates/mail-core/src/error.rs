use std::fmt;

use crate::lang::pick;

/// Erreur d'une opération sur les messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailError {
    /// Message, dossier ou compte inexistant.
    NotFound(String),
    /// Dossier de destination invalide pour l'opération demandée.
    InvalidTarget(String),
    /// Erreur remontée par le serveur (IMAP, SMTP…).
    Backend(String),
    /// Serveur injoignable ou connexion perdue : l'opération a pu aboutir ou
    /// non côté serveur, et peut être reprise plus tard.
    Network(String),
}

impl fmt::Display for MailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(s) => write!(f, "{} {s}", pick("introuvable :", "not found:", "no encontrado:")),
            Self::InvalidTarget(s) => write!(f, "{} {s}", pick("destination invalide :", "invalid destination:", "destino no válido:")),
            Self::Backend(s) => write!(f, "{} {s}", pick("erreur serveur :", "server error:", "error del servidor:")),
            // Le texte vient du transport et dit déjà « erreur réseau », « délai »…
            Self::Network(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for MailError {}

/// Résultat des opérations du cœur métier.
pub type Result<T> = std::result::Result<T, MailError>;
