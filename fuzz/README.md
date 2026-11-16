# Fuzzing de Vortex Mail

**Français** · [English](README.en.md) · [Español](README.es.md)

Cibles `cargo-fuzz` pour tout ce qui arrive de l'extérieur (lot 8 du plan du projet). Le but : qu'aucune entrée, même fabriquée exprès, ne fasse paniquer un thread, déborder la pile ou boucler sans fin.

| Cible | Ce qui est lu | Code concerné |
| --- | --- | --- |
| `imap_session` | un serveur IMAP qui répond n'importe quoi, sur toute une session | `crates/imap-lite` |
| `imap_response` | une réponse IMAP (ligne et littéraux) | `imap-lite/src/response.rs` |
| `imap_date` | une date `INTERNALDATE` | `imap-lite/src/date.rs` |
| `mime` | un message reçu, comme à l'affichage, et sa première pièce jointe | `mimeparse.rs` (crate `mail-parser`), `attachments.rs`, `sanitize.rs` |
| `html` | un corps HTML | `sanitize.rs`, `images.rs` |
| `ical` | une invitation ou un agenda CalDAV, sa réécriture, ses répétitions pour Outlook.com | `pimfmt.rs`, `o365.rs` |
| `vcard` | une carte de visite, sa réécriture | `pimfmt.rs` |
| `json` | une réponse de l'API d'Outlook.com : rendez-vous et fiches convertis, fiche réécrite puis relue, raison d'une erreur | `json.rs`, `o365.rs` |
| `dav_xml` | une réponse d'un serveur CalDAV ou CardDAV : éléments, adresses résolues et contrôlées, raison d'une erreur | `xml.rs`, `dav.rs` |
| `url` | une adresse reçue d'un serveur ou d'un message (`Location`, `href`, `@odata.nextLink`) : découpage, résolution, sites du compte | `http.rs`, `dav.rs` |
| `http` | une réponse HTTP brute : en-têtes, longueur, `chunked` | `http.rs`, `oauth.rs` |
| `smtp` | les réponses d'un serveur SMTP, l'une après l'autre | `transport.rs` |
| `backup` | une sauvegarde ou le coffre : enveloppe (sans calcul de clé), puis contenu déchiffré | `crypto.rs`, `backup.rs` |
| `oauth` | le retour de la page de connexion et le jeton d'identité | `oauth.rs` |
| `headers` | les en-têtes repris d'un message reçu pour y répondre | `crates/mail-core` |
| `mbox` | un fichier importé, `.eml` ou `.mbox` : découpé, réécrit en mbox, relu (lot E8a) | `crates/mail-core` (`mbox.rs`) |
| `smime_verify` | un message signé en S/MIME : ses deux moitiés, puis la signature CMS et la chaîne de certificats (lot G2) | `crates/smime` (`parts.rs`, `verify.rs`, `chain.rs`) |
| `smime_decrypt` | un message chiffré en S/MIME : reconnu, remis en DER, déchiffré avec la clé de test (lot G2) | `crates/smime` (`ber.rs`, `decrypt.rs`) |
| `smime_cert` | un certificat reçu : résumé, signataire, chiffrement possible vers lui (lot G2) | `crates/smime` (`cert.rs`, `encrypt.rs`) |

Les cibles S/MIME appellent directement la crate `vortex-smime`, qui n'a ni disque ni réseau. Les cibles de l'application passent par `vortexmail_lib::fuzz_api` (`src-tauri/src/lib.rs`), qui appelle les lecteurs comme le fait l'application.

## Installation (une fois)

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
```

Le dossier `dist/` doit exister (`npm run build`), comme pour toute compilation de l'application.

## Lancer

```bash
scripts/fuzz.sh 60              # essai : 60 s par cible, toutes en parallèle
scripts/fuzz.sh 28800           # une nuit : 8 h par cible
scripts/fuzz.sh 600 ical vcard  # seulement ces cibles
make fuzz DUREE=28800           # la même chose par make
```

Une cible seule, à la main :

```bash
cargo +nightly fuzz run -a ical fuzz/corpus/ical fuzz/seeds/ical
```

`-a` active les vérifications de débordement des calculs, désactivées sinon en mode release. Si la compilation avec AddressSanitizer pose problème, `SANITIZER=none scripts/fuzz.sh …` (le code de Vortex Mail n'a pas de bloc `unsafe`, c'est surtout utile pour les bibliothèques C).

Amorces : `fuzz/seeds/<cible>/`, et les mails piégés de `tests/corpus/attaques/` pour `mime`. Ce que le fuzzer apprend va dans `fuzz/corpus/<cible>/` (non versionné, gardé d'une nuit à l'autre).

## Quand une cible plante

L'entrée est enregistrée dans `fuzz/artifacts/<cible>/crash-…` (ou `timeout-…`, `oom-…`).

1. Rejouer : `cargo +nightly fuzz run -a <cible> fuzz/artifacts/<cible>/crash-…`
2. Réduire (facultatif) : `cargo +nightly fuzz tmin -a <cible> fuzz/artifacts/<cible>/crash-…`
3. Corriger, avec un test unitaire qui reprend l'entrée.
4. Copier l'entrée dans `tests/corpus/fuzz/<cible>/` : `scripts/fuzz.sh` la rejoue à chaque passage, et `src-tauri/tests/corpus.rs` la vérifie pour `mime`.

Paniques de `mail-parser` : depuis la version 1.0 (lot 9e), la cible `mime` s'arrête sur toute panique, crate comprise. `mimeparse.rs` la rattraperait dans l'application, mais elle doit être signalée aux auteurs de la crate, avec l'entrée en cause.

Fin du lot 8 : 8 h sans plantage sur chaque cible. Lot 9a : cinq cibles de plus (`http`, `smtp`, `backup`, `oauth`, `headers`).

Le harnais `imap_session` est adapté de MailCraft (`fuzz/fuzz_targets/imap.rs`, © MailCraft contributors, MIT OR Apache-2.0).
