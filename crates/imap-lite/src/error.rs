use std::fmt;

use crate::lang::pick;

/// Erreur d'une commande IMAP.
#[derive(Debug)]
pub enum Error {
    /// Erreur réseau ou connexion fermée.
    Io(std::io::Error),
    /// Le serveur a refusé la commande (réponse `NO`).
    No(String),
    /// Le serveur n'a pas compris la commande (réponse `BAD`).
    Bad(String),
    /// Le serveur a fermé la session (`BYE`).
    Bye(String),
    /// Réponse illisible ou inattendue.
    Protocol(String),
}

impl Error {
    /// Vrai si la connexion est inutilisable : il faut se reconnecter.
    #[must_use]
    pub fn is_connection_lost(&self) -> bool {
        matches!(self, Error::Io(_) | Error::Bye(_))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{} {e}", pick("erreur réseau :", "network error:", "error de red:")),
            Error::No(t) => write!(f, "{} {t}", pick("refusé par le serveur :", "refused by the server:", "rechazado por el servidor:")),
            Error::Bad(t) => write!(
                f,
                "{} {t}",
                pick("commande rejetée par le serveur :", "command rejected by the server:", "comando rechazado por el servidor:")
            ),
            Error::Bye(t) => write!(
                f,
                "{} {t}",
                pick("le serveur a fermé la connexion :", "the server closed the connection:", "el servidor cerró la conexión:")
            ),
            Error::Protocol(t) => {
                write!(f, "{} {t}", pick("réponse IMAP inattendue :", "unexpected IMAP response:", "respuesta IMAP inesperada:"))
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Résultat des opérations de cette crate.
pub type Result<T> = std::result::Result<T, Error>;
