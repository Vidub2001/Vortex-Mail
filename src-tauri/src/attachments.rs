//! Pièces jointes : nom de fichier sûr, types ouverts directement, contrôle du
//! contenu, fichiers temporaires et Mark-of-the-Web (lot 2, relecture S1 et S7).
//!
//! Principe : on n'ouvre directement que des types connus et inoffensifs
//! (liste blanche), dont le contenu correspond bien à l'extension. Tout le
//! reste passe par « Télécharger ». L'ouverture ne passe jamais par un
//! interpréteur de commandes (`cmd`, `sh`).

use std::path::{Path, PathBuf};

/// Longueur maximale d'un nom de fichier, en octets UTF-8 : les systèmes de
/// fichiers courants s'arrêtent à 255 octets, et 150 caractères cyrilliques
/// ou chinois les dépassaient (relecture complète n° 29). La marge laisse la
/// place à un numéro ajouté pour éviter un doublon.
const MAX_NAME: usize = 200;

/// Début de `s` qui tient dans `max` octets, sans couper un caractère.
fn take_bytes(s: &str, max: usize) -> &str {
    let end = s.char_indices().map(|(i, c)| i + c.len_utf8()).take_while(|&end| end <= max).last().unwrap_or(0);
    &s[..end]
}

/// Familles de fichiers ouverts directement avec l'application par défaut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pdf,
    Image,
    Text,
    /// Bureautique Office Open XML sans macros (archive ZIP).
    Ooxml,
    /// `OpenDocument` (archive ZIP).
    OpenDocument,
}

/// Liste blanche, par extension (en minuscules, sans le point).
fn kind_of(extension: &str) -> Option<Kind> {
    Some(match extension {
        "pdf" => Kind::Pdf,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Kind::Image,
        "txt" => Kind::Text,
        "docx" | "xlsx" | "pptx" => Kind::Ooxml,
        "odt" | "ods" | "odp" => Kind::OpenDocument,
        _ => return None,
    })
}

/// Caractères invisibles ou d'inversion du sens d'écriture : ils permettent de
/// faire afficher `photo` + U+202E + `gpj.exe` comme `photoexe.jpg` (relecture S7).
/// Avec les caractères qui ne dessinent rien : trait d'union conditionnel,
/// remplissages coréens, sélecteurs de variante, étiquettes (relecture du
/// 10 novembre 2026).
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// Noms réservés de Windows, interdits comme nom de fichier quelle que soit l'extension.
fn is_reserved(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((upper.starts_with("COM") || upper.starts_with("LPT")) && upper.len() == 4 && upper.as_bytes()[3].is_ascii_digit())
}

/// Nom de fichier sûr sur tous les systèmes : sans séparateur, caractère de
/// contrôle, caractère invisible ni caractère spécial de `cmd` (`& ^ % !`),
/// sans point ni espace en tête ou en fin (Windows retire les points finaux :
/// `invoice.exe.` deviendrait `invoice.exe`), sans nom réservé de Windows, et
/// raccourci si besoin en gardant l'extension.
#[must_use]
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String =
        name.chars().filter(|c| !is_invisible(*c)).map(|c| if c.is_control() || "/\\:*?\"<>|&^%!".contains(c) { '_' } else { c }).collect();
    let mut cleaned = cleaned.trim_start_matches(['.', ' ']).to_string();
    while cleaned.ends_with(['.', ' ']) {
        cleaned.pop();
    }
    let mut name = shorten(&cleaned);
    while name.ends_with(['.', ' ']) {
        name.pop();
    }
    if name.is_empty() {
        return tr!("pièce jointe").to_string();
    }
    let stem = name.split('.').next().unwrap_or_default();
    if is_reserved(stem.trim()) {
        name.insert(0, '_');
    }
    name
}

/// Raccourcit un nom trop long (en octets) en gardant son extension (si elle est courte).
fn shorten(name: &str) -> String {
    if name.len() <= MAX_NAME {
        return name.to_string();
    }
    if let Some((stem, ext)) = name.rsplit_once('.') {
        if !stem.is_empty() && ext.chars().count() <= 16 {
            let keep = take_bytes(stem, MAX_NAME - ext.len() - 1);
            return format!("{}.{ext}", keep.trim_end_matches(['.', ' ']));
        }
    }
    take_bytes(name, MAX_NAME).to_string()
}

/// Extension en minuscules, sans le point ; vide s'il n'y en a pas.
fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_lowercase(),
        _ => String::new(),
    }
}

/// Programme reconnu à son contenu (Windows, Linux, macOS, script), quel que
/// soit son nom.
#[must_use]
pub fn looks_executable(data: &[u8]) -> bool {
    if data.starts_with(b"MZ") || data.starts_with(b"\x7fELF") || data.starts_with(b"#!") {
        return true;
    }
    // Mach-O (32/64 bits, deux boutismes) et binaires universels.
    let macho: [&[u8]; 5] = [b"\xfe\xed\xfa\xce", b"\xfe\xed\xfa\xcf", b"\xce\xfa\xed\xfe", b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe"];
    if macho.iter().any(|m| data.starts_with(m)) {
        return true;
    }
    infer::get(data).is_some_and(|t| t.matcher_type() == infer::MatcherType::App)
}

/// Le contenu correspond-il à la famille annoncée par l'extension ?
fn content_matches(kind: Kind, data: &[u8]) -> bool {
    let detected = infer::get(data);
    let mime = detected.map_or_default(|t| t.mime_type());
    match kind {
        Kind::Pdf => mime == "application/pdf",
        Kind::Image => detected.is_some_and(|t| t.matcher_type() == infer::MatcherType::Image) && mime != "image/svg+xml",
        // Texte : aucun format binaire reconnu, et pas d'octet nul.
        Kind::Text => detected.is_none() && !data.contains(&0),
        Kind::Ooxml => mime == "application/zip" || mime.starts_with("application/vnd.openxmlformats-officedocument."),
        Kind::OpenDocument => mime == "application/zip" || mime.starts_with("application/vnd.oasis.opendocument."),
    }
}

/// Peut-on ouvrir ce fichier directement ? `name` est déjà passé par
/// [`safe_file_name`].
///
/// # Errors
///
/// Message à afficher (traduit) : programme, type hors liste blanche, ou
/// contenu qui ne correspond pas à l'extension.
pub fn check_open(name: &str, data: &[u8]) -> Result<(), String> {
    if looks_executable(data) {
        return Err(tr!("Par sécurité, « {name} » n'est pas ouvert : c'est un programme.", name = name));
    }
    let Some(kind) = kind_of(&extension(name)) else {
        return Err(tr!(
            "Par sécurité, « {name} » n'est pas ouvert directement : ce type de fichier ne fait pas partie des types sûrs. Utilisez « Télécharger ».",
            name = name
        ));
    };
    if !content_matches(kind, data) {
        return Err(tr!(
            "Par sécurité, « {name} » n'est pas ouvert : son contenu ne correspond pas à son extension. Utilisez « Télécharger » si vous faites confiance à l'expéditeur.",
            name = name
        ));
    }
    Ok(())
}

/// Écrit une pièce jointe à ouvrir dans un dossier temporaire privé (0700
/// sous Linux et macOS), en lecture seule, marquée comme venant d'Internet.
///
/// # Errors
///
/// Dossier ou fichier impossible à créer.
pub fn write_temp(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| tr!("dossier temporaire : {e}", e = e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let private = std::fs::Permissions::from_mode(0o700);
        let _ = std::fs::set_permissions(dir, private.clone());
        if let Some(parent) = dir.parent() {
            let _ = std::fs::set_permissions(parent, private);
        }
    }
    let path = dir.join(name);
    // Un lien symbolique posé à l'avance n'est jamais suivi (relecture du
    // 10 novembre 2026) : il est retiré avant l'écriture.
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        std::fs::remove_file(&path).map_err(|e| tr!("écriture de {name} : {e}", name = name, e = e))?;
    }
    // Fichier déjà ouvert une première fois : lecture seule, à déverrouiller.
    if let Ok(meta) = std::fs::metadata(&path) {
        let mut perms = meta.permissions();
        #[expect(
            clippy::permissions_set_readonly_false,
            reason = "fichier temporaire de Vortex Mail, rendu modifiable pour être remplacé ou effacé"
        )]
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(&path, perms);
    }
    std::fs::write(&path, data).map_err(|e| tr!("écriture de {name} : {e}", name = name, e = e))?;
    mark_from_internet(&path);
    if let Ok(meta) = std::fs::metadata(&path) {
        let mut perms = meta.permissions();
        perms.set_readonly(true);
        let _ = std::fs::set_permissions(&path, perms);
    }
    Ok(path)
}

/// Mark-of-the-Web : sous Windows, le flux `Zone.Identifier` (zone Internet)
/// fait afficher les avertissements du système et ouvre les documents Office
/// en mode protégé. Sans effet ailleurs ; une erreur (clé FAT…) est ignorée.
pub fn mark_from_internet(path: &Path) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let _ = std::fs::write(PathBuf::from(stream), b"[ZoneTransfer]\r\nZoneId=3\r\n");
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// Ouvre un fichier ou une adresse avec l'application par défaut, sans
/// interpréteur de commandes (`ShellExecuteW` sous Windows, `xdg-open` ou
/// `open` ailleurs).
///
/// # Errors
///
/// Aucune application ne prend le fichier en charge, ou lancement impossible.
pub fn open_with_system(target: &std::ffi::OsStr) -> Result<(), String> {
    opener::open(target).map_err(|e| tr!("impossible d'ouvrir le fichier : {e}", e = e))
}

/// Vide le dossier des fichiers temporaires (au démarrage et à la fermeture),
/// fichiers en lecture seule compris.
pub fn clean_temp(dir: &Path) {
    fn unlock(dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                unlock(&path);
            } else if let Ok(meta) = std::fs::metadata(&path) {
                let mut perms = meta.permissions();
                #[expect(
                    clippy::permissions_set_readonly_false,
                    reason = "fichier temporaire de Vortex Mail, rendu modifiable pour être remplacé ou effacé"
                )]
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(&path, perms);
            }
        }
    }
    unlock(dir);
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    const PDF: &[u8] = b"%PDF-1.4\n%%EOF\n";
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const PE: &[u8] = b"MZ\x90\0\x03\0\0\0\x04\0\0\0\xff\xff\0\0";

    #[test]
    fn noms_nettoyes() {
        assert_eq!(safe_file_name("a&calc&.pdf"), "a_calc_.pdf");
        assert_eq!(safe_file_name("invoice.exe."), "invoice.exe");
        assert_eq!(safe_file_name("x.hta. . ."), "x.hta");
        assert_eq!(safe_file_name("photo\u{202E}gpj.exe"), "photogpj.exe");
        assert_eq!(safe_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_file_name("..."), "pièce jointe");
        assert_eq!(safe_file_name("100%^!.txt"), "100___.txt");
        assert_eq!(safe_file_name("CON.txt"), "_CON.txt");
        assert_eq!(safe_file_name("com1"), "_com1");
        assert_eq!(safe_file_name("console.txt"), "console.txt");
    }

    #[test]
    fn nom_long_garde_son_extension() {
        let long = format!("{}.exe", "a".repeat(400));
        let short = safe_file_name(&long);
        assert_eq!(short.rsplit_once('.').map(|(_, ext)| ext), Some("exe"));
        assert!(short.len() <= MAX_NAME && short.len() > MAX_NAME - 8, "{}", short.len());
        // Relecture complète n° 29 : 150 caractères cyrilliques font 300 octets.
        let cyrillic = format!("{}.pdf", "ж".repeat(150));
        let short = safe_file_name(&cyrillic);
        assert!(short.len() <= MAX_NAME, "{} octets", short.len());
        assert_eq!(extension(&short), "pdf");
        assert!(safe_file_name(&"文".repeat(300)).len() <= MAX_NAME);
    }

    #[test]
    fn liste_blanche() {
        assert!(check_open("rapport.pdf", PDF).is_ok());
        assert!(check_open("photo.PNG", PNG).is_ok());
        assert!(check_open("notes.txt", b"Bonjour\n").is_ok());
        assert!(check_open("invoice.exe", b"x").is_err());
        assert!(check_open("x.hta", b"<html>").is_err());
        assert!(check_open("raccourci.lnk", b"L\0\0\0").is_err());
        assert!(check_open("image.svg", b"<svg/>").is_err());
        assert!(check_open("sans-extension", b"x").is_err());
    }

    #[test]
    fn contenu_controle() {
        // Exécutable déguisé en PDF, en image ou en texte.
        assert!(check_open("facture.pdf", PE).is_err());
        assert!(check_open("photo.jpg", PE).is_err());
        assert!(check_open("lisez-moi.txt", b"#!/bin/sh\nrm -rf ~\n").is_err());
        // Extension et contenu discordants.
        assert!(check_open("facture.pdf", PNG).is_err());
        assert!(check_open("photo.png", PDF).is_err());
        assert!(looks_executable(PE));
        assert!(!looks_executable(PDF));
    }
}
