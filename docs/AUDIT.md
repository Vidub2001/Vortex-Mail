# Contrôles qualité et sécurité

Ce document décrit le filet de sécurité mis en place au lot 0 de [plan-lots.md](plan-lots.md). Son rôle est de repérer toute régression pendant les lots suivants.

## En bref

```bash
make audit             # tous les contrôles
make audit-baseline    # relevé des avis acceptés de cargo-deny
make sonar             # envoi de l'analyse à SonarQube
make hooks             # Semgrep avant chaque commit
```

`make audit` passe au vert sur l'état actuel du code. Depuis le lot 8, Semgrep et Clippy sont **bloquants sans tolérance** : la moindre alerte fait échouer la commande, et une exception s'écrit dans le code avec sa raison. Pour les dépendances (cargo-deny, npm audit) et le corpus, les défauts déjà connus restent **acceptés temporairement** : chacun est listé avec le finding qu'il porte et le lot qui le corrigera ; un défaut **nouveau** fait échouer la commande.

## Installation des outils (une fois)

```bash
rustup component add clippy
cargo install --locked cargo-deny
pipx install semgrep
```

`make audit-tools` vérifie qu'ils sont tous présents. Il faut aussi Python 3.11 ou plus récent.

## Premier lancement

Le relevé de cargo-deny dépend de la machine de compilation. Il faut le créer une fois, puis le versionner. Depuis le lot F4, un avis nouveau n'y entre pas d'office : `make audit-baseline` le signale et échoue ; pour l'accepter délibérément, `make audit-baseline ACCEPTER_NOUVEAUX=1`, puis écrire sa raison dans `deny.toml` :

```bash
make audit-baseline
git add deny.toml
make audit
```

## Ce que vérifie `make audit`

| Contrôle | Outil | Findings acceptés |
| --- | --- | --- |
| Failles connues, crates abandonnées, licences, provenance des dépendances Rust | `cargo deny` | bloc « acceptés temporairement » de `deny.toml` |
| Failles connues des dépendances JavaScript (gravité haute ou critique) | `npm audit` | section `[[npm]]` de `security/accepted.toml` |
| Règles maison tirées de la relecture | `semgrep` | aucun : bloquant (exceptions justifiées dans le code, voir plus bas) |
| Mise en forme du code Rust (`rustfmt.toml`, lot 10e) | `cargo fmt --check` | aucun : bloquant ; `cargo fmt` corrige |
| Qualité du code Rust (mode `pedantic` et règles du projet, lot 10) | `cargo clippy` | aucun : bloquant (exceptions justifiées dans le code, voir plus bas) |
| Paniques déroulées (filet de `mimeparse.rs`, lot 10e) | `make panic-unwind` | aucun : `panic = "abort"` refusé dans les `Cargo.toml`, `.cargo/config.toml` et `RUSTFLAGS` |
| Corpus de mails piégés et de scénarios de perte de données | `corpus_check.py` et `cargo test --test corpus` | champ `etat` de `tests/corpus/manifest.toml` |

Les rapports détaillés sont écrits dans `target/audit/`.

## Règles Semgrep maison

Elles se trouvent dans `.semgrep/rules/`, avec un fichier de test pour chacune. `make semgrep-test` vérifie qu'elles repèrent bien ce qu'elles doivent repérer ; `make semgrep` (et donc `make audit`) le fait avant d'analyser le code.

**Bloquant depuis le lot 8 :** la moindre occurrence fait échouer `make audit`, sans liste de tolérance. Si un cas est vraiment sans danger, l'exception s'écrit dans le code, sur la ligne signalée ou celle d'avant, avec le nom de la règle et sa raison :

```rust
// nosemgrep: vortex-extension-denylist -- liste des types reconnus à leur contenu, pas une liste noire
```

Un `nosemgrep` sans nom de règle (il ferait ignorer toutes les règles) ou sans raison d'au moins dix caractères fait échouer le contrôle. `make audit` affiche la liste des exceptions en place. Depuis le lot F4, toute erreur d'analyse de Semgrep (avertissement compris) et tout fichier source sauté pour une autre raison qu'une exclusion voulue font aussi échouer le contrôle : un fichier lu en partie n'est plus couvert par les règles. Les deux exceptions de la barre d'outils de rédaction portent sur des fonctions qui n'acceptent que du HTML construit par la barre elle-même (type `ToolbarHtml`).

`make hooks` installe un crochet git versionné (`scripts/hooks/pre-commit`) : Semgrep tourne avant chaque commit qui touche du code Rust ou TypeScript. `git commit --no-verify` permet de passer outre ponctuellement.

| Règle | Repère | Relecture | Lot |
| --- | --- | --- | --- |
| `vortex-shell-command` | `Command::new("cmd")` et autres interpréteurs de commandes, chemin compris (`/bin/sh`, `zsh`, `mshta`…) | S1 | 2, 8e |
| `vortex-extension-denylist` | liste noire d'extensions (`".exe"`…) | S1 | 2 |
| `vortex-auth-redirect` | en-têtes `Authorization` renvoyés après une redirection | A1 | 5 |
| `vortex-notify-send-args` | expéditeur et objet passés à `notify-send` | S5 | 5 |
| `vortex-raw-html-sink` | `innerHTML =` et équivalents (`srcDoc`, `createContextualFragment`, `insertHTML`…), hors module autorisé `src/safe-html.ts` ; trois exceptions justifiées | S2, S3, S4 | 4, 8e |
| `vortex-html-reparse` | HTML nettoyé dans un `DOMParser` puis reparsé | S3 | 4 |

## Clippy

`make clippy` (et donc `make audit`) lance Clippy sur tout l'espace de travail, tests compris, avec `-D warnings` : la moindre alerte bloque. Le relevé d'alertes tolérées (`security/clippy-baseline.json`) a disparu au lot 8c, une fois les 501 alertes traitées.

**Règles du projet (lot 10).** Elles ne sont plus passées en ligne de commande par le `Makefile` : elles sont écrites une fois pour toutes dans `[workspace.lints]` du `Cargo.toml` racine, d'après la configuration de [BeautifulRust](https://github.com/morganlinton/BeautifulRust) (`lints/lints.toml`), et chaque crate les reprend (`[lints] workspace = true`). Un simple `cargo clippy` les applique donc, dans l'éditeur comme dans `make audit`. Au-delà du mode `pedantic` :

| Famille | Règles | Pourquoi |
| --- | --- | --- |
| Paniques | `unwrap_used`, `panic_in_result_fn`, `missing_panics_doc`, `missing_assert_message` | une panique s'annonce (`expect("raison")`, section `# Panics`) |
| Erreurs | `missing_errors_doc` | toute fonction publique qui renvoie un `Result` dit quand elle échoue |
| Exceptions | `allow_attributes`, `allow_attributes_without_reason` | `#[expect(…, reason)]` seulement, jamais `#[allow]` |
| `unsafe` | `undocumented_unsafe_blocks`, `multiple_unsafe_ops_per_block`, `unsafe_op_in_unsafe_fn` | garde-fous ; Vortex Mail n'a aucun bloc `unsafe` |
| Forme des API | `unreachable_pub`, `missing_debug_implementations`, `missing_docs`, `fn_params_excessive_bools`, `struct_excessive_bools` | `pub(crate)` par défaut ; tout type public a `Debug` et une documentation |
| Pointeurs partagés | `clone_on_ref_ptr`, `redundant_clone`, `rc_buffer` | `Arc::clone(&x)` montre qu'on partage, pas qu'on copie |
| Conversions | `as_conversions`, `cast_possible_truncation`, `cast_sign_loss` | `From`, `try_from` ; pas de `as` silencieux |
| Restes de mise au point | `dbg_macro`, `todo`, `print_stdout`, `print_stderr` | le journal passe par `src-tauri/src/diag.rs` (voir plus bas) |
| Idiomes | `string_add`, `wildcard_imports`, `rust_2018_idioms` | |

Sont écartées, comme dans BeautifulRust : `module_name_repetitions`, `must_use_candidate`, `module_inception`, `enum_glob_use` ; et `exhaustive_enums`, qui vise les bibliothèques publiées. `clippy.toml` admet `unwrap`, `expect`, `dbg!` et les affichages dans les tests (`#[test]` et modules `#[cfg(test)]`). Le faux serveur IMAP des tests d'intégration (`crates/imap-lite/tests/fake_server.rs`), hors de ces cas, porte un `#![expect(clippy::unwrap_used, …)]` en tête de fichier : une requête qu'il ne sait pas lire doit faire échouer le test. `rust-version = "1.100"` est déclaré dans chaque crate (`Option::map_or_default`), et `rust-toolchain.toml` fixe la version utilisée (1.100.0) sous Linux, sous Windows et dans la CI : un nouveau Rust peut ajouter des lints, la mise à jour se fait donc dans un lot à part.

**Exceptions.** Si une alerte est voulue (un calcul de la spécification `SHA-256` avec ses noms à une lettre, une commande Tauri qui reçoit forcément ses arguments par valeur…), l'exception s'écrit au plus près du code, avec sa raison :

```rust
#[expect(clippy::many_single_char_names, reason = "noms de la spécification (FIPS 180-4)")]
```

`expect` plutôt qu'`allow` (refusé par `clippy::allow_attributes`) : le jour où l'alerte ne se déclenche plus, `expect` le signale, et l'exception devenue inutile disparaît. Une exception sans raison est elle-même refusée (`clippy::allow_attributes_without_reason`). Pour les retrouver toutes : `git grep -n "expect(clippy"`.

**Deux écarts voulus aux règles de BeautifulRust**, justifiés dans le code et dans `docs/MODELE-MENACE.md` :

- **Verrous empoisonnés.** La règle demande d'arrêter l'application quand un verrou est « empoisonné » (une panique est survenue pendant qu'il était pris). Vortex Mail reprend le verrou (`guard` dans `router.rs`, `lock` dans `auth.rs`), parce que les données protégées sont remplacées d'un bloc ; les connexions IMAP, elles, sont fermées et rouvertes. En contrepartie : sous ces verrous, aucune modification en plusieurs étapes hors transaction.
- **`catch_unwind`.** Une seule fois, autour de la crate `mail-parser` (`guarded` dans `mimeparse.rs`) : un message qui la fait paniquer est affiché comme illisible, au lieu d'arrêter le fil qui le lisait. Ce filet ne marche que si les paniques déroulent la pile : `make panic-unwind` refuse `panic = "abort"`.

**Journal.** `print_stderr` étant refusé, tout passe par `diag::report` (`src-tauri/src/diag.rs`), qui remplace les caractères de contrôle et de mise en page (séquences d'échappement, retours à la ligne, inversion d'affichage) et coupe les messages à 2 000 caractères : les messages d'erreur des serveurs ne peuvent ni fabriquer de fausses lignes de journal, ni piloter le terminal.

## Corpus de non-régression

Le dossier `tests/corpus/` contient deux types d'entrées :

- **`attaques/`** : un mail `.eml` par attaque de la relecture (pièces jointes piégées, traceurs dans les mails cités, scripts, liens, invitations malformées…). Les charges sont inertes : les adresses visées sont en `.invalid`, et le script de test se contente de poser un marqueur.
- **`manifest.toml`** : pour chaque mail, le comportement attendu une fois le défaut corrigé. Le fichier décrit aussi, pas à pas, chaque **scénario de perte de données** (brouillon et message avec le même numéro, déplacement interrompu…).

Une entrée passe de `accepte-temporairement` à `corrige` quand un test automatique vérifie son comportement attendu. Depuis le lot 9b, elle nomme ce test, et `corpus_check.py` (donc `make audit`) refuse une entrée `corrige` sans test, ou dont le test n'existe pas. Depuis le lot F4, un test en commentaire, marqué `#[ignore]`, ou `test.skip` / `test.todo` côté interface, ne compte pas :

```toml
tests = [
  "src-tauri/src/secrets.rs::damaged_vault_falls_back_to_its_copy",      # test Rust : #[test] fn …
  "tests/frontend/calendar.test.mjs::F10 : une journée entière le jour…", # test de l'interface : test("…")
]
```

## Quand un lot corrige un défaut

Dans le même commit que la correction :

1. baisser le compteur, ou retirer l'entrée, dans `security/accepted.toml` (`make audit` signale ce qui a disparu) ;
2. passer l'entrée du corpus à `corrige`, en nommant son test (`tests = ["chemin::fonction"]`) ;
3. relancer `make audit-baseline` si des avis cargo-deny ont disparu.

Depuis le lot 8, Semgrep (lot 8b) et Clippy (lot 8c) sont bloquants sans aucune tolérance.

## SonarQube

SonarQube Community Build tourne dans un conteneur LXC du cluster Proxmox. Il sert de tableau de bord de la dette technique ; ce n'est pas un garde-fou.

- **Frontend TypeScript / React** : analysé nativement.
- **Rust** : la Community Build ne l'analyse pas. Les alertes Clippy sont importées au format générique (`sonar.externalIssuesReportPaths`).

L'adresse et le jeton ne sont **jamais** écrits dans le dépôt. Ils viennent de l'environnement :

```bash
export SONAR_HOST_URL=http://sonarqube.exemple.lan:9000
export SONAR_TOKEN=…        # SonarQube › Mon compte › Sécurité › Générer un jeton
make sonar
```

`make sonar` utilise `sonar-scanner` s'il est installé. Sinon, il passe par l'image Docker `sonarsource/sonar-scanner-cli`. Le projet s'appelle `vortexmail`.
