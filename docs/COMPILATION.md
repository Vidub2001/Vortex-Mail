# Compiler Vortex Mail

Ce guide explique comment compiler Vortex Mail à partir des sources, sous **Linux** (Ubuntu / Debian)
et sous **Windows 10 / 11**, puis comment produire un installateur.

Vortex Mail est une application [Tauri 2](https://tauri.app) : un cœur en **Rust** (`src-tauri/`,
`crates/`) et une interface **React + TypeScript** (`src/`) affichée par le moteur web du système
(WebKitGTK sous Linux, WebView2 sous Windows). Le même code sert les deux systèmes.

> Une application Windows se compile **sous Windows** (machine physique ou machine virtuelle).
> La compilation croisée depuis Linux existe mais reste expérimentale avec Tauri : elle n'est pas
> décrite ici.

---

## Sommaire

1. [Structure du projet](#1-structure-du-projet)
2. [Linux](#2-linux)
3. [Windows](#3-windows)
4. [Tests](#4-tests)
5. [Mettre à jour une installation](#5-mettre-à-jour-une-installation)
6. [Bon à savoir](#6-bon-à-savoir)
7. [Dépannage](#7-dépannage)
8. [Annexe : compiler dans WinApps](#8-annexe--compiler-dans-winapps-windows-dans-un-conteneur)

---

## 1. Structure du projet

| Dossier / fichier            | Contenu |
|------------------------------|---------|
| `src/`                       | Interface (React 18, Fluent UI v9, TypeScript) |
| `src-tauri/`                 | Application Tauri (Rust) : routeur IMAP, cache SQLite, agenda, contacts, OAuth, sauvegarde |
| `src-tauri/tauri.conf.json`  | Nom, version, identifiant, icônes, options des installateurs |
| `crates/imap-lite/`          | Client IMAP écrit pour le projet |
| `crates/mail-core/`          | Modèle de données, déplacements entre comptes, composition des messages |
| `Cargo.toml`                 | Espace de travail Rust (les trois paquets ci-dessus) |
| `package.json`               | Dépendances et scripts de l'interface |
| `target/`                    | Fichiers de compilation et installateurs produits (non versionné) |

---

## 2. Linux

Testé sous Ubuntu 26.04. Les commandes valent pour toute distribution dérivée de Debian ;
pour les autres, adapter les noms de paquets.

### 2.1 Prérequis

**Paquets système** (bibliothèques de WebKitGTK, du trousseau et outils de compilation) :

```bash
sudo apt update
sudo apt install -y build-essential curl wget file pkg-config git \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev \
  libdbus-1-dev libnotify-bin
```

| Paquet | Pourquoi |
|--------|----------|
| `build-essential`, `pkg-config` | Compilateur C (SQLite est compilé avec l'application) |
| `libwebkit2gtk-4.1-dev` | Moteur web de l'interface |
| `libssl-dev` | TLS (connexions IMAP, SMTP, HTTPS) et chiffrement du cache (SQLCipher, lot G4) |
| `libdbus-1-dev` | Trousseau du système (Secret Service : GNOME Keyring, KWallet) |
| `libnotify-bin` | Notifications des nouveaux messages (`notify-send`) |
| `libxdo-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev` | Dépendances de Tauri (`libayatana-appindicator3` : icône de la zone de notification, lot B3) |

**Rust** (via rustup ; la version du projet, fixée par `rust-toolchain.toml`, s'installe d'elle-même au premier `cargo`) :

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version
```

**Node.js** (version LTS récente, 20 ou plus) :

```bash
# par exemple avec NodeSource, ou le gestionnaire de votre choix (nvm, fnm…)
curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash -
sudo apt install -y nodejs
node --version
```

### 2.2 Récupérer les sources

```bash
mkdir -p ~/projets && cd ~/projets
git clone https://github.com/Vidub2001/Vortex-Mail.git vortexmail
cd vortexmail
npm install
```

### 2.3 Lancer en mode développement

```bash
npm run tauri dev
```

- La première compilation prend quelques minutes, les suivantes quelques secondes.
- Toute modification de `src/` est rechargée aussitôt ; une modification du Rust relance l'application.
- Le terminal doit rester ouvert ; `Ctrl+C` ferme l'application.

Interface seule, dans un navigateur, sans Rust (une maquette remplace le cœur) :

```bash
npm run dev
# puis ouvrir http://localhost:1420
```

Pour y accéder depuis une **autre machine du réseau local** (tablette, autre PC…), le serveur doit
écouter sur toutes les interfaces, et non sur `localhost` seulement :

```bash
npm run dev -- --host
```

Vite affiche alors l'adresse à utiliser, de la forme `Network: http://<adresse-IP-du-PC>:1420/`
(par exemple `http://192.168.1.50:1420`). Si elle ne répond pas, ouvrir le port 1420 dans le
pare-feu, pour le réseau local uniquement :

```bash
sudo ufw allow from 192.168.1.0/24 to any port 1420 proto tcp   # adapter au réseau local
```

Dans le navigateur, c'est toujours la **maquette** qui s'affiche (comptes et messages d'exemple) :
les vrais comptes nécessitent l'application (`npm run tauri dev` ou la version installée).

### 2.4 Produire un paquet installable

```bash
npm run tauri build -- --bundles deb
```

Résultat : `target/release/bundle/deb/Vortex Mail_<version>_amd64.deb`

Installation :

```bash
sudo apt install ./target/release/bundle/deb/*.deb
```

L'application apparaît dans le menu sous « Vortex Mail ». Elle reprend la configuration de la
version de développement (même identifiant), rien n'est à réimporter.

Autres formats possibles : `--bundles appimage` (fichier unique exécutable), `--bundles rpm`.
Sans l'option `--bundles`, ce sont les formats listés dans `tauri.conf.json` (`deb` et `appimage`).

---

## 3. Windows

Testé sous Windows 11. Fonctionne aussi sous Windows 10 à jour.

> **Poste d'entreprise** : si la sécurité Windows est gérée par un service informatique, la
> compilation peut être bloquée (« Action bloquée », `Accès refusé (os error 5)`) : Rust exécute des
> programmes qu'il vient de créer, et ces règles l'interdisent. Il ne faut pas chercher à les
> contourner. Compilez sur une machine personnelle ou dans une machine virtuelle (voir l'annexe).

### 3.1 Prérequis

Environ **6 Go** d'espace disque pour les outils, plus 3 à 4 Go pendant la compilation.

Dans un **Terminal (PowerShell) administrateur**, une commande à la fois, en attendant la fin de
chacune (coller plusieurs lignes d'un coup peut interrompre un installateur) :

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools --source winget --accept-source-agreements --accept-package-agreements --override "--wait --passive --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.22621"
```
```powershell
winget install --id Rustlang.Rustup --source winget --accept-source-agreements --accept-package-agreements
```
```powershell
winget install --id OpenJS.NodeJS.LTS --source winget --accept-source-agreements --accept-package-agreements
```
```powershell
winget install --id Git.Git --source winget --accept-source-agreements --accept-package-agreements
```
```powershell
winget install --id StrawberryPerl.StrawberryPerl --source winget --accept-source-agreements --accept-package-agreements
```

| Outil | Pourquoi |
|-------|----------|
| Visual Studio Build Tools 2022 (compilateur C++ MSVC + SDK Windows) | Compilateur et éditeur de liens utilisés par Rust sous Windows |
| Rust (rustup, cible `msvc` ; version fixée par `rust-toolchain.toml`, installée d'elle-même) | Compilation du cœur |
| Node.js LTS | Compilation de l'interface |
| Git | Récupération des sources |
| Strawberry Perl | Compilation d'OpenSSL, utilisé par SQLCipher pour chiffrer le cache (lot G4) ; le Perl fourni avec Git ne convient pas |
| WebView2 | Moteur web de l'interface : **déjà présent** sur Windows 10 à jour et Windows 11 |

Les Build Tools peuvent demander un redémarrage : acceptez-le.

Puis **fermez et rouvrez** le Terminal (pour que les nouveaux outils soient trouvés) :

```powershell
rustup set profile minimal
rustup default stable-msvc
rustup --version
node --version
git --version
```

Une seule fois, autorisez les scripts locaux de PowerShell (sinon `npm` est refusé) :

```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

### 3.2 Récupérer les sources

Placez le projet sur un disque local (pas sur un partage réseau : la compilation y serait très lente).

```powershell
cd C:\
git clone https://github.com/Vidub2001/Vortex-Mail.git vortexmail
cd vortexmail
npm install
npm install-scripts approve esbuild
npm rebuild esbuild
```

Les deux dernières lignes autorisent le script d'installation d'`esbuild` (outil qui compile
l'interface), bloqué par défaut par les versions récentes de npm.

Le dépôt est public : aucune identification n'est demandée.

### 3.3 Produire l'installateur

```powershell
npm run tauri build -- --bundles nsis
```

- Première compilation : 5 à 10 minutes. Les suivantes : moins d'une minute.
- Résultat : `C:\vortexmail\target\release\bundle\nsis\Vortex Mail_<version>_x64-setup.exe`
- L'exécutable seul est aussi produit : `C:\vortexmail\target\release\vortexmail.exe`

L'installateur, en français, installe Vortex Mail **pour l'utilisateur courant, sans droits
administrateur** (raccourcis du bureau et du menu Démarrer, désinstallation depuis « Applications
installées »).

Le programme n'étant pas signé, Windows SmartScreen affiche « Éditeur inconnu » au premier
lancement : « Informations complémentaires » → « Exécuter quand même ».

Mode développement, comme sous Linux : `npm run tauri dev`.

### 3.4 Version portable

Vortex Mail sans installation, par exemple sur une clé USB : toutes les données sont dans un
dossier `data` à côté du programme, rien dans le profil Windows ni dans le Gestionnaire
d'identification.

Construire et livrer en ZIP, en une commande (à la racine du projet) :

```powershell
.\scripts\portable.ps1
```

Le script compile l'exécutable seul (sans installateur), prépare un dossier propre
(`vortexmail.exe` + `portable.txt`, sans dossier `data`) et produit
`C:\vortexmail\target\VortexMail-portable-<version>.zip`. Il suffit de décompresser ce ZIP où
l'on veut (clé USB, autre PC) et de lancer `vortexmail.exe`.

Si PowerShell refuse le script (« l'exécution de scripts est désactivée »), l'autoriser pour
l'utilisateur courant : `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`.

- C'est la présence du fichier **`portable.txt`** à côté de `vortexmail.exe` qui active le mode
  portable ; sans lui, le même programme fonctionne comme une installation normale.
- Au premier lancement, Vortex Mail demande de **choisir un mot de passe maître**. Il chiffre les
  mots de passe et jetons des comptes (`data\secrets.vault`) ; il est redemandé à chaque démarrage
  et se change dans Paramètres → Sauvegarde. **Il ne peut pas être retrouvé** : en cas d'oubli,
  supprimer le dossier `data` et ressaisir les comptes (ou importer une sauvegarde).
- Contenu du dossier `data` : `config\accounts.json` (comptes, sans mot de passe),
  `cache.sqlite` (courrier, agenda, contacts), `secrets.vault` (coffre chiffré), `webview\`
  (réglages de l'interface).
- Reprendre une installation existante : Paramètres → Sauvegarde → Exporter sur l'installation,
  puis Importer dans la version portable.
- Prérequis sur le PC : le moteur web **WebView2**, présent d'origine sur Windows 10 et 11.
- Les pièces jointes ouvertes passent par le dossier temporaire du PC, vidé au démarrage suivant.

---

## 4. Tests

Depuis la racine du projet, sous Linux comme sous Windows :

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # application : cache, routeur, agenda, chiffrement…
cargo test -p imap-lite                           # client IMAP, dont un faux serveur
cargo test -p mail-core                           # déplacements entre comptes, composition
```

`cargo test` n'accepte qu'un seul filtre de nom à la fois :

```bash
cargo test --manifest-path src-tauri/Cargo.toml crypto::
```

Vérification des types de l'interface :

```bash
npx tsc --noEmit
```

Liste des licences des dépendances (régénère `THIRD_PARTY_LICENSES.txt` et la page « À propos ») :

```bash
npm run licenses
```

---

## 5. Mettre à jour une installation

**Linux**

```bash
cd ~/projets/vortexmail
git pull
npm install
npm run tauri build -- --bundles deb
sudo apt install ./target/release/bundle/deb/*.deb
```

**Windows**

```powershell
cd C:\vortexmail
git pull
git log -1 --oneline
npm install
npm run tauri build -- --bundles nsis
```

Vérifiez que `git log` affiche bien le dernier commit poussé depuis Linux : si `git pull` a échoué
(identification, fichier modifié localement), la compilation produit l'**ancienne** version.

Puis lancer le nouveau `setup.exe` : il remplace l'ancienne version et conserve la configuration.

Version portable : `.\scripts\portable.ps1` (voir 3.4), puis décompresser le nouveau ZIP par-dessus
l'ancien dossier ; le dossier `data` (comptes, réglages) est conservé.

Pour libérer la place prise par la compilation (2 à 4 Go) : `cargo clean` à la racine du projet.

---

## 6. Bon à savoir

### Où sont les données

| | Linux | Windows |
|---|---|---|
| Comptes (`accounts.json`, sans mot de passe) | `~/.config/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Cache du courrier, agenda, contacts (`cache.sqlite`) | `~/.local/share/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Mots de passe et jetons OAuth | Trousseau (GNOME Keyring, « Mots de passe et clés »), service `vortexmail` | Gestionnaire d'identification Windows, entrées `…vortexmail` |

Les jetons trop longs pour le Gestionnaire d'identification de Windows sont répartis sur plusieurs
entrées (`clé#1`, `clé#2`…) et réassemblés automatiquement.

### Noms à ne pas modifier

- L'identifiant `fr.vortexmail.client` (`tauri.conf.json`) : il fixe les dossiers ci-dessus.
- Le nom du service du trousseau `vortexmail` (`src-tauri/src/secrets.rs`).

Les changer ferait perdre comptes, cache et mots de passe à toutes les installations existantes,
sauf à prévoir une migration.

### Reprise des installations « FluxMail »

Les premières versions s'appelaient FluxMail (identifiant `fr.fluxmail.client`, service du
trousseau `fluxmail`). Au premier démarrage d'une version renommée, tout est repris sans action :

- les dossiers `fr.fluxmail.client` (comptes, cache, réglages de l'interface) sont renommés en
  `fr.vortexmail.client`, sauf si le nouveau dossier contient déjà des données (rien n'est écrasé) ;
- chaque mot de passe ou jeton est recopié du service `fluxmail` vers `vortexmail` à sa première
  lecture, puis l'ancienne entrée est supprimée ;
- les réglages `fluxmail.*` du stockage de l'interface deviennent `vortex.*`.

### Passer d'une machine à l'autre

Paramètres → **Sauvegarde** → Exporter : un fichier `.vxbackup` chiffré par mot de passe (comptes,
mots de passe, connexions Outlook.com et Google, réglages, agenda et contacts locaux). Sur l'autre
installation, Linux ou Windows : Paramètres → Sauvegarde → Importer.

### Version

La version affichée et celle des installateurs viennent de `"version"` dans
`src-tauri/tauri.conf.json`.

---

## 7. Dépannage

| Symptôme | Cause | Solution |
|----------|-------|----------|
| `failed to run custom build command … Accès refusé (os error 5)` et notification « Action bloquée » (Windows) | Politique de sécurité d'entreprise | Compiler sur une machine personnelle ou une VM (voir annexe) |
| `npm : Impossible de charger le fichier … npm.ps1` (Windows) | Scripts PowerShell désactivés | `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` |
| `npm warn install-scripts … esbuild` | Script d'installation bloqué par npm | `npm install-scripts approve esbuild` puis `npm rebuild esbuild` |
| `Your local changes to the following files would be overwritten by merge: package.json` (git pull) | `package.json` modifié par npm sur cette machine | `git checkout -- package.json` puis `git pull` |
| Compilation de `fluxmail` / `fluxmail.exe` produit | Sources pas à jour (`git pull` en échec) | Corriger le `git pull`, vérifier `git log -1 --oneline`, recompiler |
| `l'exécution de scripts est désactivée` (`portable.ps1`) | Scripts PowerShell désactivés | `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` |
| `0x800704c7 : L'opération a été annulée` (winget) | Installation interrompue (lignes collées à la suite, fenêtre d'autorisation fermée) | Relancer la commande seule |
| `Failed when opening source(s)` / `0x8a15000f` (winget) | Catalogue winget absent ou réseau sans DNS | Vérifier le DNS (`Resolve-DnsName github.com`), puis `winget source reset --force` et `winget source update` |
| `failed to build bundler settings: invalid category` | Catégorie inconnue dans `tauri.conf.json` | Utiliser une valeur reconnue (`Productivity`) |
| Mots de passe perdus à chaque fermeture | Trousseau non disponible | Linux : vérifier que GNOME Keyring tourne ; Windows : vérifier que la version compilée inclut `windows-native` (`src-tauri/Cargo.toml`) |
| Pas de notification (Linux) | `notify-send` absent | `sudo apt install libnotify-bin` |
| `error: linker 'link.exe' not found` (Windows) | Build Tools absents ou incomplets | Réinstaller les Build Tools avec le composant `VC.Tools.x86.x64` |

---

## 8. Annexe : compiler dans WinApps (Windows dans un conteneur)

[WinApps](https://github.com/winapps-org/winapps) fait tourner Windows dans un conteneur
Docker (image `dockur/windows`) sur la station Linux. C'est un environnement idéal pour compiler et
tester la version Windows sans machine supplémentaire.

**Ressources** — dans `~/winapps/compose.yaml`, prévoir au moins :

```yaml
      RAM_SIZE: "8G"
      CPU_CORES: "8"
      DISK_SIZE: "64G"
```

**DNS** — si Windows n'accède pas à Internet par son nom (winget en échec, `Resolve-DnsName`
qui échoue alors que `ping 1.1.1.1` répond), donner des serveurs DNS au conteneur dans le
service `windows` du `compose.yaml` :

```yaml
    dns:
      - 1.1.1.1
      - 8.8.8.8
```

puis `docker compose up -d`. Vérification : `docker exec WinApps cat /etc/resolv.conf` (la ligne
`ExtServers` doit montrer de vraies adresses).

**Port RDP** — si le bureau à distance de GNOME utilise déjà le port 3389 de la station, publier
celui de Windows sur 3390 dans `compose.yaml` :

```yaml
      - "127.0.0.1:3390:3389/tcp"
      - "127.0.0.1:3390:3389/udp"
```

et l'indiquer à WinApps :

```bash
echo 'RDP_PORT="3390"' >> ~/.config/winapps/winapps.conf
```

**Accès au bureau Windows** : `winapps windows`, ou un client RDP (Remmina) sur `127.0.0.1:3390`
avec les identifiants `RDP_USER` / `RDP_PASS` de `~/.config/winapps/winapps.conf`, ou le
navigateur sur http://127.0.0.1:8006.

**Échanger des fichiers** : le dossier personnel Linux est visible dans Windows sous
`\\host.lan\Data` (raccourci « Shared » sur le bureau).

Ensuite, suivre la section [3. Windows](#3-windows).
