# Threat model

[Français](MODELE-MENACE.md) · **English** · [Español](MODELE-MENACE.es.md)


What Vortex Mail protects, against whom, and how, with the code that does it. Automated checks are described in [AUDIT.en.md](AUDIT.en.md). A claim in this document that the code does not hold is a defect: to be fixed as such, and added to the corpus (`tests/corpus/manifest.toml`).

The document is also the starting point for new features: any new input coming from outside must find its place in one of the "Adversaries" sections below, with its limits, its tests and its fuzzing target.

## Assets to protect

| Asset | Where it lives |
| --- | --- |
| Correspondence: messages, attachments, drafts | the servers; the local cache (`cache.sqlite`, `src-tauri/src/cache.rs`) |
| Access: passwords, `OAuth2` tokens (O365, Outlook.com, Google) | the system keyring (`secrets.rs`); in portable mode, the encrypted vault `data/secrets.vault`; access tokens in memory only |
| Calendars and contacts | the servers (CalDAV, CardDAV, Outlook.com API); the local cache |
| Account configuration | `accounts.json` (without any password); encrypted backups (`backup.rs`, `crypto.rs`) |
| Privacy: when and whether a message is read, from which IP address | everything that leaves the computer |
| The computer itself | nothing that arrives in a message may run on it |

## Adversaries, and what Vortex Mail does

### A malicious sender

They write whatever they want in a message: MIME, HTML, headers, attachments, invitations, links.

- **Nothing a message contains is executed.**
    - The body is always shown in a script-less `iframe` (`sandbox="allow-same-origin allow-popups"`, without `allow-scripts`, `ReadingPane.tsx`), under the `iframe` CSP (`format.ts`: `default-src 'none'`, no script, `data:` and `cid:` images only, even after "Show images": accepted images are embedded by the backend, everything else (`srcset`, `poster`, local address) stays blocked, batch F5b) and under the application's CSP (`tauri.conf.json`).
    - **"Original" mode** (the default), as in Thunderbird: the original HTML is kept for layout, without any `<meta>`, `<link>` or `<base>` tag (`sanitize::strip_refresh`, parsing that respects quotes), and DNS prefetching is turned off (`x-dns-prefetch-control`). What neutralises it is the script-less `iframe` and the CSP, not a sanitiser.
    - **"Simplified HTML" mode**, quotes (reply, forward) and reopened drafts: allowlist sanitiser (`sanitize.rs`, `ammonia` crate): allowed tags and attributes only, `http`, `https` and `mailto` links, `data:` and `cid:` images only, styles reduced to a list of properties without `url()`, `image-set()`, `image()`, `cross-fade()`, `element()` or `position`.
    - On the interface side, HTML goes through `src/safe-html.ts`, parsed once in an inert `<template>`. Semgrep (`vortex-raw-html-sink`) refuses elsewhere `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write`, `dangerouslySetInnerHTML`, `srcDoc`/`srcdoc`, `createContextualFragment`, `setHTMLUnsafe` and the `insertHTML` command. Three exceptions are justified in the code: the `srcDoc` of the `iframe` (`ReadingPane.tsx`) and the two insertions of the formatting toolbar, which only insert HTML it builds itself (`FormatToolbar.tsx`).
- **A flaw in the interface does not give access to everything.**
    - Each window only gets its own commands (`src-tauri/capabilities/`): 21 for the message window, 12 for the compose window; none touches accounts or backups. `body_views` (audit P7) only runs an HTML body received from the window through the allowlist: it reads neither the cache nor the server. `forget_address` (lot J13) only removes an address from autocompletion. The exact list is checked by `src-tauri/tests/permissions.rs` (`droits_exacts_des_fenetres_secondaires`).
    - The compose window only reopens, replaces and deletes messages of the "Drafts" folder (`router.rs`, `ensure_draft`); it only attaches the attachments of another message for forwarded messages, declared by the window that opened it (`windows.rs`, `may_reuse`; `commands.rs`, `check_sources`). A compose window cannot open any other window.
    - `open_window` is only accepted from the main window or a message window, and checks that the label matches the loaded page (`windows.rs`, tested in `windows.rs`).
    - Secondary windows have their own web-engine storage, separate from the main window's (`windows.rs`, `webview-fenetres` folder): they can neither read nor change the sorting rules, the settings or the signatures. The settings they need (theme, language, signatures) and the starting draft are handed to them on opening by the backend (`window_bootstrap`), and only if they come from the main window; a message window passes on the ones it received, never its own.
    - They listen to events without being able to emit any (`core:event:allow-listen`, checked by `permissions.rs`): the main window is only notified by the backend (`notify_main`), which refuses any unknown or oversized event shape. The backup copy of a draft also goes through the backend, one per compose window (`save_draft_backup`). Sending happens in the background (batch E1): its failure, which hands the message back to the main window to reopen the compose window, is only emitted by the backend (`SendFailed`, which is not an event accepted from a window, tested in `windows.rs`); the backup copy stays until sending succeeds.
- **Spoofed sender** (`authres.rs`, batch G1): a message rejected by DMARC, or whose sending server is not authorised (SPF) without a valid DKIM signature, is shown under an "Unverified sender" banner; only the first `Authentication-Results` header, the one of the receiving server, is read (corpus G1).
- **Attachments** (`attachments.rs`):
    - names cleaned when displayed (`mimeparse.rs`) as when saved: trailing dots and spaces, `& ^ % !`, writing-direction (bidi) and invisible characters (including soft hyphen, Korean fillers, variation selectors, U+E0000 tags), Windows reserved names; the extension is kept when the name is shortened;
    - allowlist of types opened directly, chosen by their extension (PDF, images, text, office and OpenDocument documents; any macros in a document remain the office suite's responsibility); everything else goes through "Download", to the Downloads folder, with a message;
    - real type checked from the content (`infer` crate): a program disguised as a PDF is recognised;
    - opened without a command interpreter (`opener` crate, `ShellExecuteW` on Windows); Semgrep refuses `Command::new` with an interpreter written out, path included (`cmd`, `powershell`, `wscript`, `mshta`, `sh`, `bash`, `zsh`…: `vortex-shell-command`);
    - Mark-of-the-Web (Internet zone) on Windows; temporary files read-only, deleted at start-up and on close, in the application's cache folder; in portable mode, in a folder with a random name for each session, and a symbolic link planted in advance in place of the file is removed, not followed.
- **Links:** never followed in the reading pane. Each click becomes a window request, always refused by the backend (`windows.rs`, `on_new_window`): `http(s)` goes to the browser, `mailto:` opens the compose window (recipients, copy, subject and text taken over, the text as plain text; never a blind copy coming from the link, batch F3); SVG links (`xlink:href`) are handled like the others, and any other element carrying an address (MathML) loses it (batch F5b). A link whose text shows a different site from the real address triggers the "Misleading link" warning (`src/links.ts`): normalised text (NFKC, without invisible characters or trailing dot), compared in Punycode.
- **Tracking images:** no remote image without the "Show images" click; they are then downloaded by the backend and embedded in the message (`images.rs`: 80 images, 5 MB each, 25 MB in total counting each copy of a repeated image; downloading stops after the batch of 8 that exceeds it). Quoted mails (reply, forward) load none.
- **Embedded images (`cid:`):** budget of 8 MB per message counting each use (`mimeparse.rs`): an image repeated thousands of times is not embedded.
- **Notifications:** sender and subject passed to `notify-send` after `--`, text escaped (`idle::notify_send_args`); Semgrep checks it (`vortex-notify-send-args`).
- **Invitations and contact cards** (`pimfmt.rs`): 20,000 events per file, 5,000 excluded dates per event, 1 MB lines, 100 addresses per contact; a reminder more than a year ahead is ignored, computations checked; a recurrence Outlook.com cannot represent is refused instead of being simplified (`o365::recurrence_from_rule`). When displayed, the `BYDAY` and `BYMONTHDAY` values of a recurrence are filtered, de-duplicated and limited to 64 (`calendar.ts`).
- **S/MIME signatures** (batch D1, `crates/smime`, taken from MailCraft; `smime.rs`): a signature is only called "verified" if it is valid, its certificate leads to an authority trusted **for mail** (never a web authority), it is meant for signing mail, it is in the name of the single `From` address and the signature covers the whole message; otherwise the bar says why. A message changed after signing is flagged in red. MD5, SHA-1 and RSA below 2048 bits do not count as a signature. The certificate's name and addresses are shown as text. Parsing is bounded (BER nesting, number of certificates, signers and paths to an authority) and panic-free (`proptest` tests on arbitrary bytes and damaged CMS). An "opaque" signed message is shown through the content of its signature, with the same protections as any other body.
- **S/MIME signing key** (batch D2, `smime.rs`, `crates/smime/src/sign.rs`): imported from a PKCS #12 file whose password is only used for the import; the private key goes into the system keyring (or the portable-mode vault), never in plain text in a file or in the log (`Debug` masked). Only the main window imports, removes or configures a certificate; the compose window only knows which accounts can sign (`permissions.rs`). A certificate is only accepted for the account's address, and each signature is checked again before sending.
- **Encrypted messages received** (batch D3a, `crates/smime/src/decrypt.rs`, `Router::open`): decrypted in memory at each opening; the cache only keeps the encrypted message, with no body, preview or attachment in plain text, and "Show images" is refused for them. Parsing of the encrypted CMS is bounded like a signature's (BER turned into DER, 64 recipients, 64 MB of content); the decrypted content then goes through the same protections as any other body.
- **Encrypted messages sent** (batch D3b, `crates/smime/src/encrypt.rs`): content key and IV drawn from the system's randomness, AES-256 CBC, RSA PKCS #1 v1.5 key transport; each encrypted message is read back with the sender's key before leaving; the content key and the key-encryption key are wiped from memory after use. Encrypted sending is refused as long as a recipient has no valid certificate: never a plain-text send "for lack of anything better". The draft of a message to be encrypted is encrypted for the sender in "Drafts", and has no local backup copy (batch D4); a recipient whose certificate comes from an unknown authority is flagged in the compose window.
- **Crashes:** every parser of external input has its fuzzing target (`fuzz/`, see "Evidence"); the key derivation of a backup is not fuzzed (it only depends on the password), its envelope and content are.

### An image or link host

They see the requests the user accepted, and can answer anything, including a redirect.

- An image never targets the local network: loopback, private addresses, link-local, CGNAT, multicast; in IPv6, unique local addresses, link-local, site-local, Teredo, local-use NAT64, documentation and discard prefixes; embedded IPv4 included; checked after DNS resolution, at each redirect and for the address actually connected (`http::check_image_url`, `http.rs`).
- Addresses with a control character refused everywhere; at most 5 redirects; an image requested over https does not fall back to http; truncated responses refused.
- What they learn: the IP address and the time, once per message, and only after the click.

### GitHub (new versions, batch K1)

With the "Tell me about new versions" setting (on by default, Settings › General), Vortex Mail asks the GitHub API once a day for the latest published version number.

- What it learns: the IP address and the time, once a day; the request carries no account, identifier or cookie.
- Response of at most 512 KiB, read by the in-house JSON reader; the version tag is only kept if made of digits, letters, dots and hyphens, and the page opened is always built by the application (`https://github.com/Vidub2001/Vortex-Mail/releases/tag/…`), never the address given in the response (`update.rs`).
- Nothing is downloaded or installed: the user opens the release page in their browser, if they want to.

### A hostile or compromised server

IMAP, SMTP, CalDAV or CardDAV server, or O365, Outlook.com or Google API: it can send back any bytes, for as long as it likes.

- **IMAP** (`crates/imap-lite`): limits per response and per command (`limits.rs`: 8 MB line, 256 MB literal, 64 literals, 1,000,000 responses, nesting of 64), literal size checked before any allocation; a protocol error ends the session; dates bounded before computing (`date.rs`). A folder name returned by the server is sent back quoted without CR, LF or NUL (`client.rs`, `quote`), or as a literal.
- **SMTP** (`transport.rs`): responses of 4 KiB per line and 200 lines at most.
- **WebDAV** (`dav.rs`, `xml.rs`): responses of 32 MB at most, XML nesting kept to 256 levels; entities decoded without expansion, in linear time.
- **JSON** (`json.rs`, in-house parser): nesting of 200 at most; numbers checked before conversion, following the RFC 8259 grammar and finite (batch F4). An Outlook.com list stops at 200 pages and 256 MB in total (`o365.rs`).
- **Token server** (`oauth.rs`): responses of 1 MB at most.
- **Log** (`diag.rs`, batch 10): an error message from the server only reaches the terminal after control and layout characters are replaced (escape sequences, line breaks, display reversal), cut at 2,000 characters; it is the only place that writes the log (`print_stderr` refused by Clippy).
- **Data:** a folder renumbered by the server (`UIDVALIDITY`) is read again from scratch; every UID-based action (move, flags, deletion, offline replay) first checks that the server's `UIDVALIDITY` is the cache's, and the pending actions of the renumbered folder are cleared (batch F1); an account that changes server loses its pending actions instead of replaying them on the wrong one (`cache.rs`, `router.rs`); without UIDPLUS, a deletion does not purge messages another program had only marked (`client.rs`, `uid_expunge`). A send retried after a network cut first looks for the message by its `Message-ID` (`router.rs`; test Y8 in the corpus, batch E5). A cross-account move whose copy arrived without an answer is resumed the same way, without duplicates, and a new send attempt from the same window keeps its `Message-ID` (batch F1).
- **Secrets:**
    - WebDAV credentials only go to the account's sites (`dav.rs`, `Scope`): domain of the email address and servers entered or known, over HTTPS; in plain text, only to a host entered as `http://`. An address (`href`) or redirect returned by the server outside these sites is refused before connecting, for reads as for writes (`PUT`, `DELETE`; batch F1). The password only goes to a site after that site asked for it (`401` response with `WWW-Authenticate: Basic`);
    - the Outlook.com token only goes to the API address, even if a response (`@odata.nextLink`) points elsewhere (`o365.rs`, `call_ex`);
    - when Outlook.com has turned off SMTP sending for a mailbox (code 5.7.139), the message goes through the API (`o365::send_mime`), with a token limited to sending (`Mail.Send`, `oauth::send_token`), requested only at that moment and never kept;
    - after a redirect, `Authorization`, `Cookie` and `Proxy-Authorization` are no longer sent if the scheme, host or port changes; https → http is refused (`http::send`). Semgrep catches the old form of the defect (`vortex-auth-redirect`), not all its variants.

### A network attacker

They see and change the traffic between the computer and the servers.

- TLS for everything that carries a password or a message: SSL/TLS or STARTTLS (`transport.rs`, `native-tls`: system certificate store), TLS 1.2 at least (RFC 8996, batch F1: a single connector, `tls_connector`). "Auto" picks one or the other depending on the port; a server that does not announce STARTTLS is an error, never a fallback to plain text.
- A single case without the system certificate store: Proton Mail Bridge, which runs on the computer with a self-signed certificate (batch I3). The certificate is pinned: its SHA-256 fingerprint, shown and confirmed when the account is added, must match the one presented, otherwise the connection is refused before any credential is sent. This connector is only allowed towards a loopback address (`127.0.0.1`), never towards a server on the network: no general TLS exception.
- An unencrypted connection only exists if the user explicitly chooses it; a server announcing `LOGINDISABLED` receives the password neither through `LOGIN` nor through `AUTHENTICATE PLAIN`.
- `OAuth2` sign-in (`oauth.rs`, `google.rs`): authorisation code with PKCE (S256) and random `state`; the listening server is on `127.0.0.1` (`localhost` redirect address for Outlook.com); randomness from `getrandom`, error propagated instead of a predictable fallback (without randomness, the portable mode's temporary folder goes into the stick's `data` folder, batch F5). Sign-in is refused if the identity token is missing, is meant for another application (`aud`) or names another account than the one entered (Gmail addresses compared without dots or `googlemail.com`, batch F5).
- Truncated HTTP responses refused (`Content-Length`, final `chunked` block): a network cut does not make contacts disappear.

### A hostile imported file

A backup, a portable vault, an invitation or a contact card saved on disk.

- Encrypted backups and vault (`crypto.rs`, dependency-free, checked against the official vectors): key derived with PBKDF2-HMAC-SHA256 (600,000 iterations since batch F5, as recommended by OWASP; a vault at 200,000 is rewritten on unlock; random salt), `ChaCha20` encryption, then HMAC-SHA256 of the whole; a modified file or a wrong password is refused before any decryption; the iteration count read is bounded (10,000,000).
- A backup only writes into the keyring the entries of the accounts it contains, with their usual label, and only restores `vortex.*` settings (`backup.rs`, `screen`; `BackupSection.tsx`), which it replaces; installation-specific state (messages already examined by the rules, reminders shown, draft backups) is neither exported nor restored (batch F5b).
- Import in a safe order (`backup.rs`): secrets, then local data replaced in one piece, then configuration; missing accounts are only removed at the end. A password absent from the backup does not erase the one already stored. Tests: `import_interrompu_garde_les_contacts_locaux` (`cache.rs`) and `import_sans_mot_de_passe_garde_celui_du_trousseau` (`secrets.rs`), C2 and C4 in the corpus (batch E5).
- Imported messages (`.eml`, `.mbox`, batch E8a) are split by `mbox::split` (fuzzing target `mbox`), files capped at 200 MB, and stored on the server as they are; imported calendars (`.ics`) and address books (`.vcf`, batch E8b) go through the parsers already fuzzed (`ical`, `vcard`), 20 MB at most.
- The portable vault is written all the way to disk, into a temporary file renamed afterwards. The backup copy is only read if the vault is damaged, never when it refuses the password, and it is rewritten after a password change (`secrets.rs`): the old password no longer opens anything.

### Other programs and users of the computer

- Passwords and refresh tokens in the system keyring (Secret Service on Linux, the Windows credential store), never in `accounts.json` or in the logs; the access token stays in memory.
- Temporary attachment files deleted on close; in portable mode, a folder with an unpredictable name, without following symbolic links (see above).
- Signing out is final: a token refresh that finishes afterwards writes nothing (`auth.rs`).

### A compromised or vulnerable dependency

- `cargo deny` (security advisories, licences, origin, duplicates) and `npm audit` on every `make audit`; `Cargo.lock` and `package-lock.json` versioned.
- Few dependencies on sensitive paths: IMAP (`imap-lite`), JSON, XML, iCalendar, vCard, SHA-256, HMAC, PBKDF2 and `ChaCha20` are written in the repository, without dependencies.
- Abandoned crates accepted, with their reason (`deny.toml`): `zvariant`, `zbus_names` and `zvariant_derive`, pulled in by `zbus` (Secret Service keyring, notifications), so on the path of secrets on Linux. To be watched.
- Embedded SQLite kept up to date (`src-tauri/tests/sqlite_version.rs` fails on a vulnerable version, run by `make audit`).
- Release builds (batch G5c): GitHub Actions pinned to commit hashes (`scripts/epingler-actions.sh`), no persisted token after checkout, `SHA256SUMS` published with each release.

## Argument

**Claim.** Vortex Mail keeps the user's correspondence and access confidential and intact against the adversaries above: nothing that arrives in a message runs or leaves by itself, credentials do not leave the system keyring (or the encrypted vault) and only go to the account's servers, and known hostile inputs neither crash the application nor corrupt data, within the limits described at the end.

**Trust boundaries.** Everything that comes from outside is hostile and checked where it enters:

| Input | Checked by |
| --- | --- |
| Messages and attachments | `mimeparse.rs` (`mail-parser` 1.0 crate, limits set, panic caught), `sanitize.rs`, `attachments.rs`; script-less `iframe` and CSP for Original mode |
| Invitations and contact cards | `pimfmt.rs`, `o365.rs`, `calendar.ts` |
| IMAP and SMTP servers | `crates/imap-lite`, `transport.rs` |
| WebDAV servers, O365, Outlook.com and Google APIs | `http.rs`, `dav.rs` (`Scope`), `xml.rs`, `json.rs`, `oauth.rs`, `google.rs`, `o365.rs` |
| The web, for requested images | `images.rs`, `http::check_image_url` (the only way out to a host the user did not configure) |
| Backups and vault | `crypto.rs`, `backup.rs`, `secrets.rs` |
| The interface (WebView) to the backend | per-window permissions (`src-tauri/capabilities/`), command checks (`ensure_draft`, `check_sources`, `open_window`), CSP |

**Principles applied.**

| Principle | How |
| --- | --- |
| Least privilege | Each window only gets its commands, and the compose window only acts on its drafts and the attachments it was given; credentials only go to the account's sites |
| Deny by default | Remote images blocked; allowlist of attachments opened, tags, styles and link schemes; no plain-text fallback when STARTTLS is missing |
| Few choke points | One HTML sanitiser for simplified mode, quoting and drafts; a single function opening attachments and links; a single way out for images; three HTML insertions in the interface outside `safe-html.ts`, each justified |
| Input validation | Sizes, counts and depths capped; checked arithmetic (`checked_*`, `try_from`); strings split on character boundaries |
| Fail without loss | Writes to a temporary file then rename; journal of cross-account moves resumed after a cut; deletions last on import; after a panic, a lock is only taken back on data replaced in one piece, and an IMAP connection is closed and reopened (batch 10). Rule: under these locks, no multi-step change outside a transaction |

**Common weaknesses countered.**

| Weakness | Countered by |
| --- | --- |
| Memory safety (CWE-119, CWE-416, CWE-787) | Rust, no `unsafe` block in Vortex Mail's code (`mail-parser` 1.0 contains some, audited by its authors, for its SIMD kernels and UTF-8 conversion; fuzzed with AddressSanitizer) |
| Crashes and exhaustion (CWE-248, CWE-400, CWE-674) | Read limits on every network input, budgets counting each copy (images), bounded nesting, checked arithmetic; fuzzing of the main parsers |
| Script and active content (CWE-79) | Script-less `iframe` and CSP for every display; allowlist sanitiser for simplified mode and quotes; per-window permissions |
| Command injection (CWE-78, CWE-88) | No command interpreter; `--` before the arguments of `notify-send`; Semgrep rules |
| Log injection (CWE-117, CWE-150) | A single place writing the log (`diag::report`), which neutralises control characters, escape sequences and display reversals |
| Protocol injection (CWE-93) | Control characters refused in addresses; IMAP arguments quoted without CR or LF, or as literals (`client.rs`, `quote`); `In-Reply-To` and `References` reused without control characters (`compose.rs`) |
| Requests to the internal network from a message (CWE-918) | Images on demand only, public addresses only, check redone at each redirect and connection |
| Cleartext transmission (CWE-319) | TLS 1.2 at least, TLS or STARTTLS by default, no silent fallback; images never going from https to http |
| Credential leaks (CWE-200, CWE-522) | WebDAV credentials and Outlook.com token limited to the account's sites; authentication headers removed when the host changes; https → http refused |
| Secret storage (CWE-256, CWE-312) | System keyring, or an encrypted and authenticated vault whose backup copy follows the password; no secret in `accounts.json` |
| Misleading or dangerous file names (CWE-22, CWE-451) | `safe_file_name`, on display as on save: bidi, invisible characters, trailing dots, reserved names; type checked from the content |
| Predictable randomness (CWE-338) | `getrandom` for keys, `OAuth2`, the portable temporary folder, `Message-ID`s and the UIDs of contacts and events |

**Evidence.**

- Tests, all run by `make audit` (`cargo test --workspace`, then the corpus and the interface tests): 176 tests of the application (unit and integration: `corpus.rs`, `permissions.rs`, `sqlite_version.rs`), 51 for `imap-lite` (including a fake IMAP server), 39 for `mail-core`, 44 for `smime` (including two `proptest`), 53 for the interface, 12 interface walkthroughs in Chromium (Playwright).
- Regression corpus (`tests/corpus/`): booby-trapped mails and data-loss scenarios, each with its expected behaviour. `src-tauri/tests/corpus.rs` checks the mails, `tests/frontend` those concerning the interface; every entry marked `corrige` names the test that checks it, and `make audit` refuses an entry without an existing test (batch 9b).
- Fuzzing: `cargo-fuzz` targets (`fuzz/`, instructions in [fuzz/README.en.md](../fuzz/README.en.md)): MIME, HTML, iCalendar, vCard, WebDAV XML, JSON, dates, IMAP responses and a complete IMAP session against a server answering anything; since batch 9a, HTTP and SMTP responses, backups and vault, `OAuth2` sign-in return, headers reused for replying; since batch G2, S/MIME: signed messages verified, encrypted messages decrypted, certificates (8 h without a crash for these five targets, from 553 million to 1.25 billion inputs); since batch E8a, imported `.eml` and `.mbox` files. Every input that crashed a target is kept in `tests/corpus/fuzz/<target>/`, replayed by `corpus.rs` and at each fuzzing run. Eight targets ran 8 h each without a crash (from 40 million to 3.9 billion inputs depending on the target). The `mime` target found two `mail-parser` panics on damaged `multipart` messages: the first, in 0.9.4, is fixed by 0.11.9 (batch 8f); the second, "Invalid part ID", still exists in 0.11.9 and is caught by `mimeparse.rs` (batch 8h). The target then tolerated the crate's panics and held 8 h without any other crash (45 million inputs). Version 1.0 no longer panics on these two mails: since batch 9e, the target stops again on any panic, inside the crate too, and it held 8 h without a crash with version 1.0 (38.2 million inputs).
- Blocking static analysis: formatting (`cargo fmt --check`), Clippy in pedantic mode, with the project's rules (batch 10, based on BeautifulRust: announced panics, no `as`, `Debug` and documentation on every public type, a single log), with no tolerated warning, each exception written as `#[expect(…, reason)]` in the code; six Semgrep rules drawn from the reviews, each exception justified in the code and checked (`semgrep_gate.py`); cargo-deny; npm audit.
- Two independent reviews, a review of the Rust code of every batch since batch 10, and a full review (52 findings, fix batches F1 to F5); reports kept outside the repository; each finding has its fix batch in the project plan.

## Out of scope and known limits

- **A malicious program running under the user's account.** It can read the cache, `accounts.json` and, in general, the keyring. Vortex Mail does not defend itself against its own user's account.
- **Local cache encrypted with a key from the keyring** (batch G4). `cache.sqlite` is encrypted by SQLCipher (AES-256, journal included, temporary files in memory) with a random 32-byte key kept in the system keyring: a stolen disk, profile copy or file backup does not give the messages away. A program running under the user's account reads the keyring, so the cache too. Draft backups are also in it, instead of the `brouillons-secours.json` file and the window storage. A cache from before batch G4 is encrypted on first start, then deleted, and the old draft backups are taken over then deleted; on an SSD, their old blocks, like the window storage before it is compacted, may remain readable for a while: disk encryption (LUKS, BitLocker) is still advised. Without a usable keyring, the cache stays in plain text, and the log says so. A lost key (keyring reset) makes the cache unreadable: it is set aside (`cache.sqlite-illisible-…`) and mail synchronises again; "On this computer" calendars and contacts only come back with a backup. In portable mode, the key is kept in the vault (batch G4b): the cache on the USB stick only opens with the master password. SQLCipher's SQLite (3.53.4, imposed by `libsqlite3-sys` 0.39.1) predates the fix for the booby-trapped `-wal` file (RUSTSEC-2026-0459, batch 0b): temporarily accepted, since it would take files encrypted with the keyring's key (or the vault's in portable mode); the `tests/sqlite_version.rs` safeguard reports the day a fixed version is available.
- **Web-engine storage not encrypted.** Display settings (theme, language, formats, calendar) are in the main window's `localStorage`, in plain text (on the USB stick in portable mode). Sorting rules, signatures and mail settings, which name correspondents, are in the encrypted cache since batch J12b (audit S8, `prefs` table, `prefs_get` and `prefs_set` commands reserved to the main window); those of a previous version are taken over at startup, then erased from the window storage. Draft backups are no longer there since batch G4. On macOS, the web engine does not allow separating the storage of secondary windows (batch 9c only protects events there).
- **Original mode:** only the script-less `iframe` and the CSP separate the original HTML from the application; the `iframe` keeps `allow-same-origin`, needed to rewrite links.
- **Partial S/MIME, no OpenPGP.** Received S/MIME signatures are verified (batch D1) and sent messages can be signed (batch D2), received encrypted messages can be read (batch D3a) and a message can be sent encrypted (batch D3b). Encrypted messages are read with an RSA or elliptic-curve key (P-256, P-384, ECDH; batch G2b), in AES-CBC, 3DES or AES-GCM; a message is sent encrypted in AES-256-CBC to RSA, P-256 or P-384 certificates (ECDH with an ephemeral key per message). An encrypted message leaves its subject, sender and recipients readable (headers), and the list of encryption recipients reveals blind copies to anyone who can read the CMS. A correspondent's certificate kept without a trusted authority (unknown authority) may be an impostor's who signed a message in that name: the list of correspondents flags it, and it is never preferred to a certificate from a trusted authority. The signing key, kept by the system keyring, can be read by a program running under the user's account, like the passwords; it is not in Vortex Mail's backups (to be imported again after a restore). Revocation is checked through the authorities' certificate revocation lists (CRL), downloaded in the background and consulted offline (batch G2c): the authority only learns that its certificates are used, never which ones; OCSP is not used; only the signer's certificate is checked (not intermediate authorities); a result kept in the cache is only updated by "Check again"; an authority without a list leaves revocation "not checked". On Ubuntu, Debian and macOS, no authority trusted for mail is read from the system: a signature there is only "verified" with an authority placed by the user in `smime/autorites`. The certificate of a correspondent whose signature is valid is still kept there, marked "not verified": the compose window points it out and Settings › Mail shows its fingerprint, to compare with them. The result is computed at download: a certificate that has since expired is only seen by clicking "Check again". The `smime` crate is fuzzed since batch G2 (targets `smime_verify`, `smime_decrypt`, `smime_cert`); importing a `.p12` file, chosen by the user, is not.
- **Sender authenticated by the receiving server only** (batch G1). Vortex Mail reads the SPF, DKIM and DMARC results the receiving server writes at the top of the message (`Authentication-Results`), without checking them itself. A server that writes none leaves the sender unchecked; a sender adding a fake header can only cause a false warning, never hide one (only the first header counts, and a failure wins). A sender writing from a look-alike domain (`banque-exemple.example`), authenticated for that domain, is not flagged.
- **Password sent to guessed WebDAV servers** from the address's domain, when no address is given: it only goes to that domain, over HTTPS, and only to a site that asks for it (batch F1). A website of the domain asking for a `Basic` password therefore receives it; since batch G3, servers announced by DNS (`SRV` `_caldavs._tcp`, `_carddavs._tcp`, RFC 6764) are tried first, over HTTPS and only in the same domain as the address (DNS is not authenticated); the query goes to the system resolver (`/etc/resolv.conf` on Linux and macOS, `DnsQuery_W` on Windows, batch G3b).
- **Images shown:** once the click is made, the host learns the IP address and the time.
- **Unencrypted connection** possible on the user's explicit choice; the password then travels in plain text. An imported backup may contain such an account.
- **Checks that remain partial:** the corpus checks that a named test exists, not that it really checks the expected behaviour (that is the review's job); Semgrep rules catch forms written out in full (an interpreter passed through a variable escapes them).
- **Windows releases not signed yet.** Releases are built by GitHub Actions from the public code and published with their `SHA256SUMS` (batch G5c); local checks still run on the developer's machine (`make audit`, Semgrep hook before each commit, Jenkins). Windows code signing through SignPath Foundation is pending: until then, Windows shows a warning on first install.
- **External MIME parser.** `mail-parser` 1.0 is configured by `mimeparse.rs`: 32 nesting levels, 1,000 parts, 3 nested encoded messages (beyond that, the rest of the mail stays in a last part). It is fuzzed (`fuzz/fuzz_targets/mime.rs`). A panic of the crate would be caught by `mimeparse.rs` (message shown as "unreadable"; the code's only `catch_unwind`, an intended departure from the batch 10 rules), which requires a build that unwinds the stack on panic (the default setting; `make audit` refuses `panic = "abort"` since batch 10e).

