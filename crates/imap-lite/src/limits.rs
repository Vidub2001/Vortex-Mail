//! Plafonds de lecture des réponses du serveur.
//!
//! Un serveur hostile ou compromis (ou quiconque sur le réseau quand le compte
//! n'est pas chiffré) ne doit ni épuiser la mémoire, ni faire déborder la pile,
//! ni faire paniquer la synchronisation. Valeurs inspirées de `mailcraft-proto`
//! (`MailCraft`, licence MIT OR Apache-2.0), adaptées aux besoins de Vortex Mail.
//!
//! Un plafond dépassé donne une [`Error::Protocol`] : la session est alors
//! abandonnée (la suite de la réponse est peut-être restée dans le tampon).

use crate::lang::pick;
use crate::Error;

/// Plus longue ligne lue. Large : un `UID SEARCH` sur un très gros dossier
/// tient sur une seule ligne (environ un million d'UID).
pub(crate) const MAX_LINE: usize = 8 << 20;
/// Plus gros littéral `{n}` accepté (un message entier).
pub(crate) const MAX_LITERAL: usize = 256 << 20;
/// Plus grand nombre de littéraux dans une même réponse.
pub(crate) const MAX_LITERALS: usize = 64;
/// Plus grand nombre de réponses pour une même commande.
pub(crate) const MAX_RESPONSES: usize = 1_000_000;
/// Texte reçu pour une même commande, littéraux exclus.
pub(crate) const MAX_COMMAND_TEXT: usize = 64 << 20;
/// Littéraux reçus pour une même commande (au plus dix messages entiers par lot).
pub(crate) const MAX_COMMAND_LITERALS: usize = 1 << 30;
/// Plus profonde imbrication de parenthèses (`BODYSTRUCTURE` compris).
pub(crate) const MAX_DEPTH: usize = 64;

/// Le plafond dépassé.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Limit {
    Line,
    Literal,
    Literals,
    Responses,
    CommandText,
    CommandLiterals,
    Depth,
}

fn megabytes(bytes: usize) -> usize {
    bytes >> 20
}

/// L'erreur à rendre quand `limit` est dépassé.
pub(crate) fn exceeded(limit: Limit) -> Error {
    let mb = pick("Mo", "MB", "MB");
    let detail = match limit {
        Limit::Line => format!("{} {} {mb}", pick("ligne de plus de", "line over", "línea de más de"), megabytes(MAX_LINE)),
        Limit::Literal => format!(
            "{} {} {mb}",
            pick("bloc de données de plus de", "data block over", "bloque de datos de más de"),
            megabytes(MAX_LITERAL)
        ),
        Limit::Literals => format!(
            "{} {MAX_LITERALS} {}",
            pick("plus de", "more than", "más de"),
            pick("blocs de données dans une réponse", "data blocks in one response", "bloques de datos en una respuesta")
        ),
        Limit::Responses => format!(
            "{} {MAX_RESPONSES} {}",
            pick("plus de", "more than", "más de"),
            pick("réponses à une commande", "responses to one command", "respuestas a un comando")
        ),
        Limit::CommandText => format!(
            "{} {} {mb} {}",
            pick("plus de", "more than", "más de"),
            megabytes(MAX_COMMAND_TEXT),
            pick("de texte pour une commande", "of text for one command", "de texto para un comando")
        ),
        Limit::CommandLiterals => format!(
            "{} {} {mb} {}",
            pick("plus de", "more than", "más de"),
            megabytes(MAX_COMMAND_LITERALS),
            pick("de données pour une commande", "of data for one command", "de datos para un comando")
        ),
        Limit::Depth => format!(
            "{} {MAX_DEPTH} {}",
            pick("plus de", "more than", "más de"),
            pick("niveaux de parenthèses", "levels of parentheses", "niveles de paréntesis")
        ),
    };
    Error::Protocol(format!(
        "{} ({detail})",
        pick("réponse du serveur trop volumineuse", "server response too large", "respuesta del servidor demasiado grande")
    ))
}
