# Vortex Mail
**Français** · [English](README.md) · [Español](README.es.md)

Client mail multi-comptes pour Linux et Windows, écrit en Rust (Tauri 2) avec une interface
React 18 / Fluent UI v9. Courrier IMAP/SMTP, agendas et contacts synchronisés, interface en
français, anglais et espagnol.

Sa particularité : un message peut être classé dans un dossier **de n'importe quel compte**, à la
main (glisser-déposer, « Déplacer vers ») ou automatiquement par des règles. Les déplacements entre
comptes sont journalisés : rien n'est perdu ni dupliqué si le réseau coupe en cours de route.

> **État :** première version (0.1.0), utilisée au quotidien par son auteur. Le code source sera
> bientôt publié ici.

## Fonctionnalités

**Courrier**
- Comptes IMAP/SMTP : SSL/TLS ou STARTTLS ; mot de passe ou connexion OAuth2 (Outlook.com, comptes
  personnels O365, Gmail).
- Boîtes unifiées (réception, envoyés, brouillons…) ou comptes séparés ; gestion complète des
  dossiers (créer, renommer, déplacer, supprimer, vider).
- Lecture : HTML isolé et sans script, images distantes bloquées par défaut, pièces jointes
  (ouverture, enregistrement), invitations d'agenda ajoutées en un clic.
- Rédaction : HTML ou texte simple (format par défaut réglable), signatures par compte, réponses et
  transferts, pièces jointes par glisser-déposer, brouillons enregistrés automatiquement,
  suggestions de destinataires, **avertissement de pièce jointe oubliée**.
- Déplacement et suppression entre comptes, avec journal : un déplacement interrompu (coupure
  réseau, fermeture) est repris au démarrage suivant, sans doublon ni perte.
- Recherche dans tous les dossiers de tous les comptes, contenu compris.
- Nouveaux messages en temps réel (IMAP IDLE) et notifications du bureau ; synchronisation
  automatique réglable, incrémentale sur les serveurs CONDSTORE / QRESYNC (rapide sur les grands
  dossiers).
- **Règles** : conditions (expéditeur, destinataires, objet, début du message, pièce jointe) et
  actions (déplacer vers un dossier de n'importe quel compte, marquer lu, indicateur, supprimer) ;
  appliquées aux nouveaux messages ou à la demande ; création directement depuis un message
  (clic droit).
- Hors connexion : cache local complet ; les actions sont mises en attente et rejouées au retour du
  réseau.

**Agenda et contacts**
- Gmail (OAuth2 Google, CalDAV/CardDAV), Outlook.com, Mailo/Netcourrier et tout serveur
  CalDAV/CardDAV ; agendas et carnets d'adresses « Sur cet ordinateur ».
- Vues semaine, mois et année ; couleur par agenda ; numéros de semaine (ISO 8601, semaine du
  1er janvier ou première semaine complète) ; premier jour de la semaine et échelle de la grille
  réglables.
- Rendez-vous : répétitions (y compris personnalisées : jours de la semaine, intervalle, jour du
  mois, fin), **rappels** (notifications du bureau, synchronisés avec les serveurs).
- Contacts par compte, avec prénom et nom (ordre d'affichage réglable).

**Réglages**
- Langue de l'interface et des messages d'erreur (français, anglais, espagnol), thème
  clair/sombre/système, formats de date et d'heure, disposition du volet de lecture (à droite,
  en bas, masqué).
- **Sauvegarde** chiffrée de toute la configuration (comptes, mots de passe, réglages, règles,
  agendas et contacts locaux) pour installer Vortex Mail sur une autre machine.
- **Mode portable** pour Windows : toutes les données à côté de l'exécutable, protégées par un mot
  de passe maître.

**Sécurité**
- Aucun mot de passe ni jeton n'est écrit dans un fichier en clair : tout est dans le trousseau du
  système (GNOME Keyring / Secret Service sous Linux, Gestionnaire d'identification sous Windows),
  ou dans le coffre chiffré en mode portable.
- Sauvegardes chiffrées (PBKDF2-HMAC-SHA256, ChaCha20, HMAC).

## Démarrage

Prérequis : Rust (stable), Node.js 18+ et les dépendances système de Tauri 2 pour votre plateforme
(WebKitGTK sous Linux, WebView2 sous Windows).

```bash
git clone <adresse-du-dépôt> vortexmail
cd vortexmail
npm install
npm run tauri dev
```

Le premier lancement compile toutes les dépendances Rust (quelques minutes) ; les suivants sont
rapides.

Pour travailler uniquement l'interface, sans Rust, avec rechargement instantané dans le
navigateur :

```bash
npm run dev
# puis ouvrir http://localhost:1420 (une maquette JavaScript remplace le backend)
```

Tests (cœur métier, protocole IMAP contre un faux serveur, application) :

```bash
cargo test
```

La compilation en version finale, les installateurs, le ZIP portable pour Windows et le dépannage
sont décrits dans `docs/COMPILATION.md`.

## Ajouter un compte

Paramètres (engrenage) → Comptes → « Ajouter un compte ». Pour les fournisseurs connus (Gmail,
Outlook.com, Mailo…), les serveurs se remplissent seuls. Types de connexion :

| Choix    | IMAP | SMTP | Fonctionnement |
|----------|------|------|----------------|
| SSL/TLS  | 993  | 465  | chiffré dès la connexion |
| STARTTLS | 143  | 587  | connexion en clair puis passage en TLS |
| Auto     |      |      | SSL/TLS sur les ports 993/465, sinon STARTTLS |
| Aucun    | 143  | 25   | mot de passe en clair, déconseillé |

« Tester les paramètres » vérifie l'IMAP et le SMTP sans rien enregistrer.

**OAuth2 (Gmail, Outlook.com) :** Vortex Mail n'embarque aucun identifiant client OAuth.
Enregistrez votre propre application (console Google Cloud pour Gmail, portail développeur
d'Outlook.com pour Outlook.com), puis saisissez son ID client (et son secret pour Google) dans les
réglages du compte ; ils sont rangés dans le trousseau et ne sont plus jamais affichés.

## Emplacement des données

| | Linux | Windows |
|---|---|---|
| Comptes (`accounts.json`, sans mot de passe) | `~/.config/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Cache du courrier, de l'agenda et des contacts (`cache.sqlite`) | `~/.local/share/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Mots de passe et jetons | trousseau, service `vortexmail` | Gestionnaire d'identification |
| Mode portable | — | dossier `data\` à côté de l'exécutable |

Le cache peut être supprimé sans risque : il est reconstruit depuis les serveurs. Les
installations faites sous l'ancien nom (FluxMail) sont reprises automatiquement au premier
démarrage.

## Organisation du projet

```
crates/imap-lite/        client IMAP sans dépendance : protocole, littéraux, UTF-7, IDLE, dates
crates/mail-core/        logique métier, sans interface (testée seule)
  src/model.rs           comptes, dossiers, messages
  src/backend.rs         trait MailBackend (implémenté par la démo et par le routeur IMAP)
  src/ops.rs             déplacements dans un compte et entre comptes + journal
  src/compose.rs         construction des messages (MIME, pièces jointes, images intégrées)
  tests/                 tests des déplacements
src-tauri/               couche Tauri (Rust)
  src/router.rs          comptes IMAP derrière MailBackend, synchronisation, recherche
  src/transport.rs       TCP, SSL/TLS, STARTTLS, authentification IMAP et SMTP, envoi
  src/cache.rs           cache SQLite (dossiers, en-têtes, corps, agendas, contacts)
  src/journal.rs         actions hors connexion et déplacements à reprendre
  src/idle.rs            nouveaux messages en temps réel, notifications
  src/oauth.rs           OAuth2 Outlook.com (courrier)
  src/o365.rs            agenda et contacts Outlook.com
  src/endpoints.rs       adresses techniques des services
  src/google.rs          OAuth2 Google (courrier, agenda, contacts Gmail)
  src/dav.rs, pim.rs     CalDAV / CardDAV, synchronisation de l'agenda et des contacts
  src/pimfmt.rs          iCalendar et vCard
  src/secrets.rs         mots de passe et jetons dans le trousseau (ou le coffre portable)
  src/crypto.rs          chiffrement des sauvegardes
  src/backup.rs          export / import de la configuration
  src/migrate.rs         reprise des installations FluxMail
  src/portable.rs        mode portable (marqueur portable.txt, dossier data)
  src/windows.rs         création des fenêtres
  src/i18n.rs, locales/  langue des messages produits par le backend (macro tr!)
  src/commands.rs        commandes appelées par l'interface
src/                     interface React + Fluent UI
  App.tsx, Popup.tsx     fenêtre principale, fenêtres de message et de rédaction
  api.ts                 appels au backend (ou maquette hors Tauri)
  i18n.ts, locales/      langues (le français est la langue source ; en.ts et es.ts)
  general.ts, settings.ts  réglages
  rules.ts               règles de classement
  calendar.ts, recurrence.ts, reminders.ts  agenda : occurrences, répétitions, rappels
  components/            volets, listes, rédaction, agenda, contacts, paramètres
docs/COMPILATION.md      compilation sous Linux et Windows
```

## Feuille de route

Fait :
- ~~Journal persistant des déplacements entre comptes ; recherche dans tous les dossiers.~~
- ~~Notifications IDLE (nouveaux messages en temps réel).~~
- ~~Rédaction et envoi (SMTP), réponses et transferts.~~
- ~~Pièces jointes : liste, ouverture, enregistrement.~~
- ~~OAuth2 Outlook.com.~~
- ~~Agenda et contacts (Gmail, Outlook.com, Mailo, CalDAV/CardDAV).~~
- ~~Sauvegarde et reprise de la configuration ; version Windows.~~
- ~~Réglages généraux, règles, rappels, alerte pièce jointe, interface en trois langues.~~
- ~~CONDSTORE / QRESYNC : synchronisation incrémentale des grands dossiers.~~
- ~~OAuth2 Google pour le courrier Gmail.~~
- ~~Messages d'erreur traduits en anglais et en espagnol.~~
- ~~Version portable pour Windows (données à côté de l'exécutable, mot de passe maître).~~

À venir :
1. Corrections issues de la relecture du code de novembre 2026 (sécurité, protection des données,
   fuseaux horaires de l'agenda).
2. Fonctionnement en arrière-plan : règles appliquées aux mails reçus pendant que l'application
   était fermée, icône dans la zone de notification.
3. Comptes O365 professionnels et scolaires (aujourd'hui : comptes personnels Outlook.com / O365
   uniquement).
4. S/MIME : signature et chiffrement, import et export des certificats.

## Limites connues

- La connexion Google utilise votre propre application OAuth ; tant qu'elle est en mode « test »
  chez Google, les jetons Gmail expirent au bout de 7 jours et un bandeau de reconnexion en un clic
  apparaît.
- Les rappels et les règles ne fonctionnent que lorsque Vortex Mail est ouvert.
- Pas encore de tests automatiques pour l'interface (la partie Rust compte 126 tests).

## Contribuer

Les signalements et les contributions sont les bienvenus. Tout texte de l'interface, nouveau ou
modifié, passe par `t()` avec ses entrées dans `src/locales/en.ts` et `src/locales/es.ts` ; les
textes produits par le backend Rust passent par `tr!` avec leurs entrées dans
`src-tauri/src/locales/`.

## Licence

À définir.
