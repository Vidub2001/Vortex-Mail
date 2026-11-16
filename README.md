# Vortex Mail
[Français](README.fr.md) · **English** · [Español](README.es.md)

A multi-account desktop mail client for Linux and Windows, written in Rust (Tauri 2) with a
React 18 / Fluent UI v9 interface. IMAP/SMTP mail, synchronised calendars and contacts, and a user
interface available in French, English and Spanish.

What makes it different: a message can be filed into a folder **of any account** — by hand
(drag and drop, "Move to") or automatically with rules — and cross-account moves are journaled so
that nothing is lost or duplicated if the network drops halfway.

> **Status:** early release (0.1.0), in daily use by its author. Expect rough edges.

## Features

**Mail**
- IMAP/SMTP accounts: SSL/TLS or STARTTLS; password or OAuth2 sign-in (Outlook.com, O365 personal
  accounts, Gmail).
- Unified folders (Inbox, Sent, Drafts…) or separate accounts; full folder management (create,
  rename, move, delete, empty).
- Reading: sandboxed HTML with no scripts, remote images blocked by default, attachments (open,
  save), calendar invitations added, updated or
  removed (cancellation) in one click; reply to the organizer (Accept, Maybe, Decline), sent or
  not, as you choose.
- Received S/MIME signatures checked: trusted signer, sender's address, message changed after
  signing flagged; certificate details. System authorities reserved for mail, or ones placed in
  the `smime/autorites` folder of the settings.
- S/MIME-signed messages: per-account certificate imported from a .p12 file (key kept in the
  keyring), “Sign” button when writing, signing by default as an option.
- S/MIME-encrypted messages: read with the same certificate, decrypted in memory (nothing in
  plain text on disk); sent encrypted with the “Encrypt” button, using the correspondents'
  certificates kept from their signed messages.
- Writing: HTML or plain text (default format configurable), per-account signatures, reply and
  forward, drag-and-drop attachments, automatic draft saving, recipient suggestions,
  **forgotten-attachment warning**.
- Moving and deleting across accounts, with a journal: an interrupted move (network loss, app
  closed) is resumed at next start-up, without duplicates or loss.
- Message import and export: one message as `.eml`, a folder as `.mbox` (Downloads); `.eml` and `.mbox` files (Thunderbird's included) imported into the chosen folder.
- Calendar (`.ics`) and address book (`.vcf`) import and export, without duplicates when imported twice.
- Search across every folder of every account, message bodies included, with criteria: `from:`, `to:`, `subject:`, `before:2026-11-16`, `after:`, `is:unread`, `is:flagged`, `has:attachment`, `"exact phrase"`, `-word` to exclude.
- Real-time new mail (IMAP IDLE) and desktop notifications; configurable automatic sync,
  incremental on CONDSTORE / QRESYNC servers (fast on large folders).
- **Rules**: conditions (sender, recipients, subject, start of the body, has attachment) and actions
  (move to a folder of any account, mark as read, flag, delete); run on new mail or on demand;
  create a rule straight from a message (right-click).
- **Background**: closing the window keeps Vortex Mail in the notification area (configurable in
  Settings › General); mail, rules and reminders keep running. New-mail notifications come after
  the rules (none for a message they delete), two rules that send a message back and forth no
  longer bounce it, and reminders missed while the app was closed are summarised at startup.
- Offline: full local cache; actions are queued and replayed when the network is back.

**Calendar and contacts**
- Gmail (Google OAuth2, CalDAV/CardDAV), Outlook.com, Mailo/Netcourrier and any CalDAV/CardDAV
  server; plus local "On this computer" calendars and address books.
- Week, month and year views; per-calendar colours; week numbers (ISO 8601, week containing
  January 1st, or first full week); configurable first day of the week and grid scale.
- Events: recurrence (including custom rules: weekdays, interval, day of month, end), **reminders**
  (desktop notifications, synchronised with the servers).
- Contacts per account, with given and family names (display order configurable).

**Settings**
- Interface and error-message language (French, English, Spanish), light/dark/system theme, date
  and time formats, reading pane layout (right, bottom, hidden).
- Encrypted **backup** of the whole configuration (accounts, passwords, settings, rules, local
  calendars and contacts) to move Vortex Mail to another machine.
- **Portable mode** for Windows: all data next to the executable, protected by a master password.

**Security**
- No password or token is ever written to a plain file: everything lives in the system keyring
  (GNOME Keyring / Secret Service on Linux, Credential Manager on Windows), or in the encrypted
  vault in portable mode.
- Encrypted backups (PBKDF2-HMAC-SHA256, ChaCha20, HMAC).
- Automated checks before every merge (`make audit`): dependencies (cargo-deny, npm audit), Semgrep
  rules drawn from the code reviews, strict Clippy with the project's rules, formatting, tests (including
  interface walkthroughs in Chromium) and a corpus of booby-trapped mails; fuzzing of the parsers (MIME, IMAP, HTML, iCalendar, network
  responses…). See [docs/MODELE-MENACE.md](docs/MODELE-MENACE.md) and [docs/AUDIT.md](docs/AUDIT.md)
  (in French).

## Getting started

Requirements: Rust (stable), Node.js 18+ and the Tauri 2 system dependencies for your platform
(WebKitGTK on Linux, WebView2 on Windows).

```bash
git clone <repository-url> vortexmail
cd vortexmail
npm install
npm run tauri dev
```

The first run compiles every Rust dependency (a few minutes); later runs are fast.

To work on the interface only, without Rust, with instant reload in a browser:

```bash
npm run dev
# then open http://localhost:1420 (a JavaScript mock replaces the backend)
```

Tests and checks:

```bash
cargo test --workspace   # Rust: core logic, IMAP against a fake server, application
npm test                 # interface: calendar, links, rules, reminders
make ui-test             # interface walkthroughs in Chromium (npx playwright install chromium)
make audit               # every check (see docs/AUDIT.md)
```

Release builds, installers, the Windows portable ZIP and troubleshooting are covered in
**[docs/COMPILATION.md](docs/COMPILATION.md)** (in French).

## Adding an account

Settings (gear icon) → Accounts → "Add an account". For well-known providers (Gmail,
Outlook.com, Mailo…) the server settings are filled in automatically. Connection types:

| Choice   | IMAP | SMTP | How it works |
|----------|------|------|--------------|
| SSL/TLS  | 993  | 465  | encrypted from the start |
| STARTTLS | 143  | 587  | plain connection upgraded to TLS |
| Auto     |      |      | SSL/TLS on ports 993/465, otherwise STARTTLS |
| None     | 143  | 25   | password sent in clear, not recommended |

"Test settings" checks IMAP and SMTP without saving anything.

**OAuth2 (Gmail, Outlook.com):** Vortex Mail does not ship with built-in OAuth client credentials.
Register your own application (Google Cloud console for Gmail, the Outlook.com developer portal for Outlook.com), then enter its client ID (and secret
for Google) in the account settings; they are stored in the keyring and never displayed again.

## Data locations

| | Linux | Windows |
|---|---|---|
| Accounts (`accounts.json`, no passwords) | `~/.config/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Mail, calendar and contacts cache (`cache.sqlite`) | `~/.local/share/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Passwords and tokens | keyring, service `vortexmail` | Credential Manager |
| Portable mode | — | `data\` folder next to the executable |

The cache can be deleted safely: it is rebuilt from the servers. Installations made under the
former name (FluxMail) are migrated automatically on first start.

## Project layout

```
crates/imap-lite/        dependency-free IMAP client: protocol, literals, UTF-7, IDLE, dates
crates/mail-core/        business logic, UI-free (tested on its own)
  src/model.rs           accounts, folders, messages
  src/backend.rs         MailBackend trait (implemented by the demo and by the IMAP router)
  src/ops.rs             same-account and cross-account moves + journal
  src/compose.rs         message building (MIME, attachments, inline images)
  tests/                 move tests
src-tauri/               Tauri layer (Rust)
  src/router.rs          IMAP accounts behind MailBackend, sync, search
  src/transport.rs       TCP, SSL/TLS, STARTTLS, IMAP and SMTP authentication, sending
  src/cache.rs           SQLite cache (folders, headers, bodies, calendars, contacts)
  src/journal.rs         offline actions and moves to resume
  src/idle.rs            real-time new mail (IMAP IDLE)
  src/tray.rs            notification area
  src/diag.rs            diagnostic log (sanitised messages)
  src/oauth.rs           Outlook.com OAuth2 (mail)
  src/o365.rs            Outlook.com calendar and contacts
  src/endpoints.rs       technical service addresses
  src/google.rs          Google OAuth2 (Gmail mail, calendar, contacts)
  src/dav.rs, pim.rs     CalDAV / CardDAV, calendar and contact sync
  src/pimfmt.rs          iCalendar and vCard
  src/secrets.rs         passwords and tokens in the keyring (or portable vault)
  src/crypto.rs          backup encryption
  src/backup.rs          configuration export / import
  src/migrate.rs         migration of FluxMail installations
  src/portable.rs        portable mode (portable.txt marker, data folder)
  src/windows.rs         window creation
  src/i18n.rs, locales/  language of messages produced by the backend (tr! macro)
  src/commands.rs        commands called by the interface
src/                     React + Fluent UI interface
  App.tsx, Popup.tsx     main window, message and compose windows
  api.ts                 backend calls (or mock outside Tauri)
  i18n.ts, locales/      languages (French is the source language; en.ts and es.ts)
  general.ts, settings.ts  settings
  rules.ts               mail rules
  calendar.ts, recurrence.ts, reminders.ts  calendar: occurrences, recurrence, reminders
  components/            panes, lists, compose, calendar, contacts, settings
docs/COMPILATION.md      Linux and Windows build guide (French)
docs/AUDIT.md            checks (make audit); MODELE-MENACE.md: threat model (French)
docs/plan-lots.md        plan of fix and feature batches (French)
fuzz/, tests/            fuzzing, regression corpus, interface tests
```

## Roadmap

Done:
- ~~Persistent cross-account move journal; search across all folders.~~
- ~~IDLE notifications (real-time new mail).~~
- ~~Writing and sending (SMTP), reply and forward.~~
- ~~Attachments: list, open, save.~~
- ~~Outlook.com OAuth2.~~
- ~~Calendar and contacts (Gmail, Outlook.com, Mailo, CalDAV/CardDAV).~~
- ~~Configuration backup and restore; Windows version.~~
- ~~General settings, rules, reminders, attachment warning, three-language interface.~~
- ~~CONDSTORE / QRESYNC incremental sync for large folders.~~
- ~~Google OAuth2 for Gmail mail.~~
- ~~Error messages translated into English and Spanish.~~
- ~~Windows portable version (data next to the executable, master password).~~
- ~~Fixes from the November 2026 code review (security, data safety, calendar time zones),
  fuzzing, threat model, Rust coding rules.~~
- ~~Background operation: notification area, rules and notifications, missed reminders.~~

Next:
1. Polish: faster startup, translated folder names, first tests of the display.
2. O365 work and school accounts, if the need arises (today: personal Outlook.com / O365 accounts
   only).

## Known limitations

- Google sign-in uses your own OAuth app; while that app is in Google's "testing" status, Gmail
  tokens expire after 7 days and a one-click reconnect banner appears.
- Reminders and rules only run while Vortex Mail is running (window open or icon in the
  notification area).
- On GNOME without the AppIndicator extension (enabled by default on Ubuntu), the notification-area
  icon does not show, yet closing the window still hides it: launch Vortex Mail again to bring it
  back, or turn the setting off in Settings › General.
- The interface's automated tests cover its logic (calendar, links, rules, reminders), not yet the
  display.

## Contributing

Issues and pull requests are welcome. Any new or changed interface text must go through `t()`
with entries in `src/locales/en.ts` and `src/locales/es.ts`; texts produced by the Rust backend go
through `tr!` with entries in `src-tauri/src/locales/`.

## Downloads and code signing

Releases are built from this repository by GitHub Actions and published on the Releases page, with the SHA-256 checksums of every file (`SHA256SUMS`). Windows files are signed: free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by [SignPath Foundation](https://signpath.org/). See the [code signing policy](CODE_SIGNING.md).

## License

Vortex Mail is free software, released under the GNU General Public License, version 3 or
(at your option) any later version: see [LICENSE](LICENSE). Third-party licences are listed in [THIRD_PARTY_LICENSES.txt](THIRD_PARTY_LICENSES.txt).
