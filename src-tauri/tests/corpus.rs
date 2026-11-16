//! Corpus de mails piégés (tests/corpus) : chaque mail doit se décoder sans
//! plantage, et ses pièces jointes doivent garder un nom.
//!
//! Les vérifications propres à chaque attaque (nom nettoyé, ressources
//! distantes supprimées…) s'ajoutent ici au fil des lots, selon manifest.toml.

use std::path::{Path, PathBuf};

use mail_parser::MessageParser;
use vortexmail_lib::attachments;

fn corpus() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/corpus/attaques");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("corpus introuvable ({}) : {e}", dir.display()))
        .map(|entry| entry.expect("entrée du corpus").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "eml"))
        .collect();
    files.sort();
    files
}

#[test]
fn chaque_mail_du_corpus_se_decode() {
    let files = corpus();
    assert!(!files.is_empty(), "le corpus est vide");
    for path in files {
        let raw = std::fs::read(&path).expect("lecture du mail");
        let message = MessageParser::default().parse(&raw[..]).unwrap_or_else(|| panic!("{} : mail illisible", path.display()));
        assert!(message.subject().is_some(), "{} : objet absent", path.display());

        let announces_attachment = raw.windows(10).any(|w| w.eq_ignore_ascii_case(b"attachment"));
        if announces_attachment {
            assert!(message.attachments().next().is_some(), "{} : pièce jointe non reconnue", path.display());
            for part in message.attachments() {
                if let Some(name) = part.attachment_name() {
                    assert!(!name.trim().is_empty(), "{} : nom de pièce jointe vide", path.display());
                }
            }
        }
    }
}

/// Première pièce jointe nommée d'un mail du corpus : (nom, contenu).
fn first_attachment(file: &str) -> (String, Vec<u8>) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/corpus/attaques").join(file);
    let raw = std::fs::read(&path).expect("lecture du mail");
    let message = MessageParser::default().parse(&raw[..]).expect("mail lisible");
    // Variable locale : l'itérateur de `mail-parser` 0.11 emprunte `message`
    // jusqu'à la fin de l'expression.
    let found = message
        .attachments()
        .find_map(|part| part.attachment_name().map(|n| (n.to_string(), part.decoded().into_owned())))
        .unwrap_or_else(|| panic!("{file} : aucune pièce jointe nommée"));
    found
}

/// Lot 2 : les pièces jointes piégées de la relecture (S1, S7) sont neutralisées.
#[test]
fn pieces_jointes_piegees_neutralisees() {
    // S1-a : plus de caractère spécial de cmd dans le nom ; le PDF lui-même est sûr.
    let (name, data) = first_attachment("S1-a-injection-cmd.eml");
    let safe = attachments::safe_file_name(&name);
    assert_eq!(safe, "a_calc_.pdf");
    assert!(attachments::check_open(&safe, &data).is_ok());

    // S1-b, S1-c, S1-d, S7 : jamais ouverts directement.
    for (file, expected) in [
        ("S1-b-point-final.eml", "invoice.exe"),
        ("S1-c-hta.eml", "x.hta"),
        ("S1-d-pe-deguise-en-pdf.eml", "facture.pdf"),
        ("S7-bidi-nom-inverse.eml", "photogpj.exe"),
    ] {
        let (name, data) = first_attachment(file);
        let safe = attachments::safe_file_name(&name);
        assert_eq!(safe, expected, "{file} : nom nettoyé");
        assert!(attachments::check_open(&safe, &data).is_err(), "{file} : ne doit pas s'ouvrir directement");
    }
}

/// Le nom affiché est déjà nettoyé : S7 ne se lit plus « photoexe.jpg »
/// (relecture du 10 novembre 2026).
#[test]
fn noms_affiches_nettoyes() {
    for (file, expected) in [("S7-bidi-nom-inverse.eml", "photogpj.exe"), ("S1-a-injection-cmd.eml", "a_calc_.pdf")] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/corpus/attaques").join(file);
        let raw = std::fs::read(&path).expect("lecture du mail");
        assert_eq!(vortexmail_lib::corpus_api::attachment_names(&raw), vec![expected.to_string()], "{file}");
    }
}

/// Lots 8f, 8h et 9e : entrées qui ont fait paniquer `mail-parser` 0.9 et
/// 0.11 pendant le fuzzing (`tests/corpus/fuzz/mime/`). La version 1.0 ne
/// doit plus paniquer, et `mimeparse` rattraperait une panique de la crate.
#[test]
fn entrees_du_fuzzing_sans_panique() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/corpus/fuzz/mime");
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(&dir).expect("dossier des entrées du fuzzing").map(|e| e.expect("entrée").path()).collect();
    files.sort();
    assert!(!files.is_empty(), "aucune entrée dans {}", dir.display());
    for path in files {
        let raw = std::fs::read(&path).expect("lecture");
        let direct = std::panic::catch_unwind(|| {
            if let Some(m) = MessageParser::default().parse(&raw[..]) {
                let _ = (m.subject(), m.date(), m.body_html(0), m.body_text(0));
                for part in m.parts() {
                    let _ = (part.decoded().len(), part.text(), part.attachment_name(), part.content_type());
                    let _ = part.nested().and_then(|nested| nested.subject());
                }
            }
        });
        assert!(direct.is_ok(), "{} : mail-parser panique", path.display());
        let shown = std::panic::catch_unwind(|| vortexmail_lib::corpus_api::attachment_names(&raw));
        assert!(shown.is_ok(), "{} : la panique de mail-parser n'est pas rattrapée", path.display());
    }
}

/// Corps HTML d'un mail du corpus, tel que le cache le stocke.
fn html_body(file: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/corpus/attaques").join(file);
    let raw = std::fs::read(&path).expect("lecture du mail");
    let message = MessageParser::default().parse(&raw[..]).expect("mail lisible");
    message.body_html(0).expect("corps HTML").into_owned()
}

/// Lot 4a (S2, S3, S4) : le HTML cité ou rouvert comme brouillon passe par la
/// liste blanche du backend. Rien n'y doit plus charger de ressource distante,
/// exécuter de script ni recouvrir la fenêtre.
#[test]
fn html_cite_ou_rouvert_nettoye() {
    const INTERDITS: [&str; 16] = [
        "tracker.example.invalid",
        "<script",
        "onerror",
        "onload",
        "javascript:",
        "position",
        "<noscript",
        "srcset",
        "background=",
        "url(",
        "<svg",
        "<video",
        "poster",
        "<input",
        "<picture",
        "__vortexCorpusPwned",
    ];
    let files = [
        "S2-a-srcset.eml",
        "S2-b-svg-image.eml",
        "S2-c-style-background.eml",
        "S2-d-table-background.eml",
        "S2-e-video-poster.eml",
        "S2-f-input-image.eml",
        "S2-g-picture-source.eml",
        "S2-h-faux-dialogue.eml",
        "S2-i-image-set.eml",
        "S3-mxss-noscript.eml",
        "S4-brouillon-script.eml",
    ];
    for file in files {
        let html = html_body(file);
        let clean = vortexmail_lib::sanitize::simplify(&html);
        for bad in INTERDITS {
            assert!(
                !clean.to_ascii_lowercase().contains(&bad.to_ascii_lowercase()),
                "{file} : « {bad} » subsiste après nettoyage :\n{clean}"
            );
        }
        // Nettoyer deux fois ne change rien : pas de seconde lecture qui
        // comprendrait le HTML autrement (mXSS).
        assert_eq!(vortexmail_lib::sanitize::simplify(&clean), clean, "{file} : nettoyage instable");
    }
}

/// Lot 4a (S6-b) et 8e (S6-c) : en mode Original, meta, link et base
/// disparaissent du corps affiché, variantes comprises.
#[test]
fn meta_refresh_supprime() {
    for file in ["S6-b-meta-refresh.eml", "S6-c-meta-variantes.eml"] {
        let html = html_body(file);
        assert!(html.to_ascii_lowercase().contains("refresh"), "{file} : le mail de test a changé");
        let shown = vortexmail_lib::sanitize::strip_refresh(&html).to_ascii_lowercase();
        assert!(!shown.contains("http-equiv"), "{file} : meta refresh présent :\n{shown}");
        assert!(!shown.contains("<link") && !shown.contains("<base"), "{file} : link ou base présent :\n{shown}");
        assert!(!shown.contains("tracker.example.invalid"), "{file} : cible présente :\n{shown}");
        assert!(shown.contains("bonjour"), "{file} : le texte a disparu :\n{shown}");
    }
}

/// Lot 5 (A2) : « Afficher les images » ne contacte ni 127.0.0.1 par une
/// adresse piégée (CR/LF), ni le réseau local, ni l'adresse des métadonnées
/// d'un hébergeur.
#[test]
fn images_vers_le_reseau_local_refusees() {
    for file in ["A2-a-crlf-dans-url.eml", "A2-b-ssrf-reseau-local.eml"] {
        let urls = vortexmail_lib::corpus_api::remote_image_urls(&html_body(file));
        assert!(!urls.is_empty(), "{file} : aucune image distante trouvée");
        for url in urls {
            assert!(vortexmail_lib::corpus_api::check_image_url(&url).is_err(), "{file} : {url:?} serait téléchargée");
        }
    }
    assert!(vortexmail_lib::corpus_api::check_image_url("https://93.184.216.34/logo.png").is_ok());
}

/// Lot 5 (S5) : un expéditeur ou un objet piégé ne passe ni en option de
/// notify-send, ni en balisage.
#[test]
fn notifications_piegees_neutralisees() {
    for file in ["S5-a-notify-option.eml", "S5-b-notify-balisage.eml"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/corpus/attaques").join(file);
        let raw = std::fs::read(&path).expect("lecture du mail");
        let message = MessageParser::default().parse(&raw[..]).expect("mail lisible");
        let from = message.from().and_then(|f| f.first()).and_then(|a| a.name()).unwrap_or_default().to_string();
        let subject = message.subject().unwrap_or_default().to_string();
        let args = vortexmail_lib::corpus_api::notify_send_args(&from, &format!("{subject}\nCompte"));
        let sep = args.iter().position(|a| a == "--").unwrap_or_else(|| panic!("{file} : pas de --"));
        assert_eq!(args[sep + 1], from, "{file}");
        assert!(!args[sep + 2].contains('<'), "{file} : balisage dans le texte : {}", args[sep + 2]);
        assert!(args[..sep].iter().all(|a| !a.contains(&from) || from.is_empty()), "{file}");
    }
}

/// Invitation (fichier iCalendar joint) d'un mail du corpus.
fn invitation(file: &str) -> String {
    let (_, bytes) = first_attachment(file);
    String::from_utf8(bytes).expect("invitation en UTF-8")
}

/// Lot 6 (S8) : un rappel aberrant dans une invitation est ignoré, sans plantage.
#[test]
fn rappel_aberrant_ignore() {
    let reminders = vortexmail_lib::corpus_api::ical_reminders(&invitation("S8-trigger-debordement.eml"));
    assert_eq!(reminders.len(), 1, "S8 : le rendez-vous doit être lu");
    assert_eq!(reminders[0], None, "S8 : rappel aberrant gardé");
}

/// Lot 6 (A5) : « 1er et 3e lundi » n'existe pas dans l'API d'Outlook.com ;
/// l'enregistrement est refusé au lieu de créer « 1er lundi » seulement.
#[test]
fn repetition_complexe_refusee_pour_outlook_com() {
    let accepted = vortexmail_lib::corpus_api::o365_accepts_recurrences(&invitation("A5-rrule-complexe.eml"));
    assert_eq!(accepted, vec![false]);
}

/// Lot G1 : expéditeur usurpé, refusé par DMARC au serveur de réception ;
/// l'en-tête « tout va bien » ajouté par l'expéditeur, plus bas, n'y change rien.
#[test]
fn expediteur_usurpe_signale() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/corpus/attaques");
    let raw = std::fs::read(dir.join("G1-expediteur-usurpe.eml")).expect("G1 dans le corpus");
    assert_eq!(vortexmail_lib::corpus_api::sender_check(&raw), "dmarc");
}
