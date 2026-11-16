//! Données de démonstration : trois comptes fictifs, de quoi tester l'interface
//! et le glisser-déposer entre comptes sans serveur IMAP.

use crate::memory::MemoryStore;
use crate::mime;
use crate::model::{Flags, FolderRole, RawMessage};

/// (nom expéditeur, e-mail, sujet, corps HTML, il y a N heures, lu, suivi, pièce jointe)
type DemoMail = (&'static str, &'static str, &'static str, &'static str, i64, bool, bool, bool);

const PERSO_INBOX: &[DemoMail] = &[
    ("Banque Exemple", "alertes@banque.example.com", "Votre relevé d'octobre est disponible",
     "<p>Bonjour,</p><p>Votre relevé de compte du mois d'octobre est disponible dans votre espace client.</p><p>Cordialement,<br>Le service client</p>",
     2, false, false, true),
    ("Rail Voyages", "billets@railvoyages.example.com", "Confirmation de votre réservation Paris → Lyon",
     "<p>Bonjour,</p><p>Votre réservation est confirmée : départ vendredi à 8 h 04, voiture 12, place 45.</p><p>Bon voyage !</p>",
     5, false, false, false),
    ("Marie Dupont", "marie.dupont@example.com", "Repas de dimanche",
     "<p>Coucou !</p><p>On se retrouve à 12 h 30 dimanche ? Je m'occupe du dessert, tu prends le pain ?</p><p>Bises,<br>Marie</p>",
     26, true, true, false),
    ("Énergie Exemple", "factures@energie.example.com", "Votre facture de novembre",
     "<p>Bonjour,</p><p>Votre facture de novembre est disponible. Le prélèvement aura lieu le 15.</p>",
     30, true, false, true),
    ("Club Photo du quartier", "contact@clubphoto.example.org", "Sortie photo samedi matin",
     "<p>Bonjour à tous,</p><p>Rendez-vous samedi à 9 h devant la médiathèque pour la sortie « lumières d'automne ».</p>",
     50, true, false, false),
    ("Librairie en ligne", "commandes@livres.example.com", "Votre commande a été expédiée",
     "<p>Bonne nouvelle : votre commande n° 48213 a été expédiée et arrivera sous 48 h.</p>",
     75, true, false, false),
    ("Thomas Martin", "thomas.martin@example.net", "Photos des vacances",
     "<p>Salut,</p><p>Je t'envoie enfin les photos de la Bretagne. La lumière était incroyable le dernier soir !</p><p>Thomas</p>",
     120, true, false, true),
    ("Mutuelle Exemple", "noreply@mutuelle.example.com", "Remboursement effectué",
     "<p>Un remboursement de 23,50 € a été effectué sur votre compte.</p>",
     200, true, false, false),
];

const PERSO_FACTURES: &[DemoMail] = &[(
    "Opérateur Box",
    "facture@box.example.com",
    "Votre facture internet d'octobre",
    "<p>Votre facture d'octobre est disponible dans votre espace abonné.</p>",
    400,
    true,
    false,
    true,
)];

const PERSO_VOYAGES: &[DemoMail] = &[(
    "Hôtel du Port",
    "reservation@hotelduport.example.com",
    "Votre séjour à Saint-Malo",
    "<p>Merci pour votre séjour ! Nous espérons vous revoir bientôt.</p>",
    900,
    true,
    false,
    false,
)];

const PRO_INBOX: &[DemoMail] = &[
    ("Sophie Bernard", "s.bernard@client-a.example.com", "Point d'avancement – migration CI/CD",
     "<p>Bonjour,</p><p>Pouvons-nous caler un point jeudi pour faire le bilan de la migration des pipelines ? Il reste deux jobs de déploiement à basculer.</p><p>Bien cordialement,<br>Sophie</p>",
     1, false, true, false),
    ("Support Hébergeur", "support@hebergeur.example.net", "Maintenance planifiée du 12 novembre",
     "<p>Une maintenance réseau aura lieu le 12 novembre entre 2 h et 4 h. Une coupure de 5 minutes est possible.</p>",
     3, false, false, false),
    ("Julien Petit", "julien.petit@client-b.example.com", "Devis pour l'audit d'architecture",
     "<p>Bonjour,</p><p>Vous trouverez ci-joint le cahier des charges pour l'audit. Pourriez-vous nous faire parvenir un devis d'ici la fin du mois ?</p><p>Julien</p>",
     22, true, false, true),
    ("Cabinet comptable", "compta@cabinet.example.fr", "Pièces manquantes pour la clôture",
     "<p>Bonjour,</p><p>Il nous manque trois justificatifs pour finaliser la clôture trimestrielle.</p>",
     48, false, false, false),
    ("Supervision", "monitoring@example.fr", "[RÉSOLU] Disque /var à 92 % sur srv-build-02",
     "<p>L'alerte « espace disque » sur srv-build-02 est résolue (taux actuel : 61 %).</p>",
     70, true, false, false),
    ("Nadia Leroy", "n.leroy@fournisseur.example.com", "Renouvellement des licences",
     "<p>Bonjour,</p><p>Vos licences arrivent à échéance le 31 décembre. Souhaitez-vous les renouveler aux mêmes conditions ?</p>",
     140, true, false, false),
];

const PRO_CLIENT_A: &[DemoMail] = &[(
    "Sophie Bernard",
    "s.bernard@client-a.example.com",
    "Compte rendu du comité de pilotage",
    "<p>Bonjour à tous,</p><p>Vous trouverez ci-joint le compte rendu du dernier comité.</p>",
    300,
    true,
    false,
    true,
)];

const PRO_SENT: &[DemoMail] = &[(
    "Moi",
    "contact@example.fr",
    "Re: Devis pour l'audit d'architecture",
    "<p>Bonjour Julien,</p><p>Bien reçu, je vous envoie une proposition d'ici vendredi.</p>",
    20,
    true,
    false,
    false,
)];

const ASSO_INBOX: &[DemoMail] = &[
    (
        "Lettre Rust francophone",
        "news@rust-fr.example.org",
        "Les nouveautés de la semaine",
        "<p>Au programme : nouvelle version stable, retour sur les async closures et une sélection de crates.</p>",
        6,
        false,
        false,
        false,
    ),
    (
        "Association des anciens",
        "bureau@anciens.example.org",
        "Assemblée générale annuelle",
        "<p>Chers membres,</p><p>L'assemblée générale se tiendra le 6 décembre à 18 h. Ordre du jour en pièce jointe.</p>",
        28,
        true,
        false,
        true,
    ),
    (
        "Atelier Makers",
        "contact@makers.example.org",
        "Nouvel atelier découpe laser",
        "<p>Un nouvel atelier d'initiation à la découpe laser ouvre ses portes le mardi soir.</p>",
        96,
        true,
        false,
        false,
    ),
];

const ASSO_NEWSLETTERS: &[DemoMail] = &[(
    "Revue Linux",
    "lettre@revue-linux.example.org",
    "Dossier : bien choisir son client mail",
    "<p>Ce mois-ci, nous comparons les clients mail du bureau Linux.</p>",
    500,
    true,
    false,
    false,
)];

const ASSO_TRASH: &[DemoMail] = &[(
    "Promo Express",
    "offres@promo.example.com",
    "-50 % sur tout le site ce week-end",
    "<p>Ne manquez pas nos offres exceptionnelles !</p>",
    60,
    true,
    false,
    false,
)];

impl MemoryStore {
    /// Magasin rempli avec trois comptes fictifs. `now` : timestamp Unix actuel.
    ///
    /// # Panics
    ///
    /// Si un message de démonstration vise un dossier qui n'a pas été créé
    /// (erreur de programmation, pas de données reçues).
    #[must_use]
    pub fn with_demo_data(now: i64) -> Self {
        let mut s = MemoryStore::new();

        s.add_account("perso", "Gmail perso", "fabrice.perso@example.com", "#c4314b");
        s.add_account("pro", "OVH pro", "contact@example.fr", "#0f6cbd");
        s.add_account("asso", "Association", "fabrice@example.org", "#107c10");

        for account in ["perso", "pro", "asso"] {
            s.add_folder(account, "INBOX", FolderRole::Inbox);
            s.add_folder(account, "Drafts", FolderRole::Drafts);
            s.add_folder(account, "Sent", FolderRole::Sent);
            s.add_folder(account, "Archive", FolderRole::Archive);
            s.add_folder(account, "Trash", FolderRole::Trash);
            s.add_folder(account, "Junk", FolderRole::Junk);
        }
        s.add_folder("perso", "Factures", FolderRole::Custom);
        s.add_folder("perso", "Voyages", FolderRole::Custom);
        s.add_folder("pro", "Clients", FolderRole::Custom);
        s.add_folder("pro", "Clients/Client A", FolderRole::Custom);
        s.add_folder("pro", "Clients/Client B", FolderRole::Custom);
        s.add_folder("pro", "Fournisseurs", FolderRole::Custom);
        s.add_folder("asso", "Newsletters", FolderRole::Custom);

        let sets: [(&str, &str, &[DemoMail]); 9] = [
            ("perso:INBOX", "fabrice.perso@example.com", PERSO_INBOX),
            ("perso:Factures", "fabrice.perso@example.com", PERSO_FACTURES),
            ("perso:Voyages", "fabrice.perso@example.com", PERSO_VOYAGES),
            ("pro:INBOX", "contact@example.fr", PRO_INBOX),
            ("pro:Clients/Client A", "contact@example.fr", PRO_CLIENT_A),
            ("pro:Sent", "julien.petit@client-b.example.com", PRO_SENT),
            ("asso:INBOX", "fabrice@example.org", ASSO_INBOX),
            ("asso:Newsletters", "fabrice@example.org", ASSO_NEWSLETTERS),
            ("asso:Trash", "fabrice@example.org", ASSO_TRASH),
        ];
        for (folder, to, mails) in sets {
            for &(name, email, subject, html, hours_ago, seen, flagged, attachment) in mails {
                let raw = RawMessage {
                    bytes: mime::build(name, email, to, subject, html, attachment),
                    flags: Flags { seen, flagged, answered: false },
                    internal_date: now - hours_ago * 3600,
                };
                s.insert(folder, raw).expect("dossier de démo manquant");
            }
        }
        s
    }
}
