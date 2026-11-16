//! Lot 1 : cohérence entre les commandes de l'application, leur déclaration
//! dans `build.rs` et les droits des fenêtres (`capabilities/`).
//! Une commande oubliée dans `build.rs` ne serait soumise à aucun contrôle.

use std::collections::BTreeSet;

fn between<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let from = text.find(start).unwrap_or_else(|| panic!("« {start} » introuvable")) + start.len();
    let len = text[from..].find(end).unwrap_or_else(|| panic!("« {end} » introuvable"));
    &text[from..from + len]
}

fn handler_commands() -> BTreeSet<String> {
    let lib = include_str!("../src/lib.rs");
    between(lib, "generate_handler![", "])")
        .split(',')
        .filter_map(|item| item.trim().rsplit("::").next().map(str::to_string))
        .filter(|name| !name.is_empty())
        .collect()
}

fn declared_commands() -> BTreeSet<String> {
    between(include_str!("../build.rs"), "COMMANDS: &[&str] = &[", "];")
        .split(',')
        .map(|item| item.trim().trim_matches('"').to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

fn granted(capability: &str) -> BTreeSet<String> {
    capability.split('"').filter_map(|s| s.strip_prefix("allow-")).map(|s| s.replace('-', "_")).collect()
}

#[test]
fn toutes_les_commandes_sont_declarees() {
    assert_eq!(handler_commands(), declared_commands(), "build.rs et generate_handler! doivent lister les mêmes commandes");
}

#[test]
fn les_droits_des_fenetres_existent() {
    let declared = declared_commands();
    for (name, file) in [
        ("default", include_str!("../capabilities/default.json")),
        ("message", include_str!("../capabilities/message.json")),
        ("compose", include_str!("../capabilities/compose.json")),
    ] {
        let unknown: Vec<_> = granted(file).difference(&declared).cloned().collect();
        assert!(unknown.is_empty(), "{name}.json accorde des commandes inconnues : {unknown:?}");
    }
}

/// Liste exacte des commandes des fenêtres secondaires : tout ajout est un
/// choix délibéré, à reporter dans `docs/MODELE-MENACE.md`.
#[test]
fn droits_exacts_des_fenetres_secondaires() {
    let message: Vec<String> = granted(include_str!("../capabilities/message.json")).into_iter().collect();
    assert_eq!(
        message,
        [
            "answer_invitation",
            "check_signature",
            "event_save",
            "get_message",
            "list_accounts",
            "list_folders",
            "move_messages",
            "move_to_role",
            "notify_main",
            "open_attachment",
            "open_url",
            "open_window",
            "pim_collections",
            "pim_delete",
            "read_invitation",
            "save_attachment",
            "set_flagged",
            "set_seen",
            "show_images",
            "window_bootstrap",
        ],
        "fenêtre de message : 20 commandes (lot H1 : réponse aux invitations)"
    );
    let compose: Vec<String> = granted(include_str!("../capabilities/compose.json")).into_iter().collect();
    assert_eq!(
        compose,
        [
            "address_suggest",
            "discard_draft",
            "list_accounts",
            "notify_main",
            "open_draft",
            "save_draft",
            "save_draft_backup",
            "send_message",
            "smime_missing",
            "smime_signers",
            "window_bootstrap",
        ],
        "fenêtre de rédaction : 11 commandes"
    );
}

/// Lot 9c : les fenêtres secondaires écoutent les événements mais n'en
/// émettent pas ; elles préviennent la fenêtre principale par `notify_main`.
#[test]
fn fenetres_secondaires_sans_emission_d_evenements() {
    for (name, file) in
        [("message", include_str!("../capabilities/message.json")), ("compose", include_str!("../capabilities/compose.json"))]
    {
        assert!(!file.contains("core:event:default"), "{name} : core:event:default accorde l'émission d'événements");
        assert!(!file.contains("allow-emit"), "{name} : émission d'événements accordée");
        assert!(file.contains("core:event:allow-listen"), "{name} : l'écoute des événements est nécessaire");
    }
    let main = include_str!("../capabilities/default.json");
    let granted_main = granted(main);
    for only_main in ["draft_backups", "clear_draft_backup"] {
        assert!(granted_main.contains(only_main), "main : {only_main} manquant");
    }
}

#[test]
fn la_redaction_n_a_pas_acces_aux_comptes() {
    let compose = granted(include_str!("../capabilities/compose.json"));
    let message = granted(include_str!("../capabilities/message.json"));
    // Certificats S/MIME (lot D2) : importés, retirés et réglés depuis la fenêtre principale seulement.
    for forbidden in [
        "export_backup",
        "import_backup",
        "test_account",
        "add_account",
        "update_account",
        "remove_account",
        "list_account_configs",
        "smime_identity",
        "smime_import",
        "smime_remove",
        "smime_sign_by_default",
        "smime_correspondents",
        "smime_forget_correspondent",
    ] {
        assert!(!compose.contains(forbidden), "compose : {forbidden} ne doit pas être accordé");
        assert!(!message.contains(forbidden), "message : {forbidden} ne doit pas être accordé");
    }
    // Les liens d'un message s'ouvrent dans le navigateur externe (lot 4b) :
    // la fenêtre de message en a besoin, pas la rédaction.
    assert!(!compose.contains("open_url"), "compose : open_url ne doit pas être accordé");
    assert!(message.contains("open_url"), "message : open_url est nécessaire pour les liens");
    assert!(!compose.contains("open_window"), "une fenêtre de rédaction n'ouvre pas d'autres fenêtres");
}
