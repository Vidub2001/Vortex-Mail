# Vortex Mail
[Français](README.fr.md) · **English** · [Español](README.es.md)

A multi-account desktop mail client for Linux and Windows, written in Rust (Tauri 2) with a
React 18 / Fluent UI v9 interface. IMAP/SMTP mail, synchronised calendars and contacts, and a user
interface available in French, English and Spanish.

What makes it different: a message can be filed into a folder **of any account** — by hand
(drag and drop, "Move to") or automatically with rules — and cross-account moves are journaled so
that nothing is lost or duplicated if the network drops halfway.

> **Status:** early release (0.1.0), in daily use by its author. The source code will be published here soon.

## Features

**Mail**
- IMAP/SMTP accounts: SSL/TLS or STARTTLS; password or OAuth2 sign-in (Outlook.com, O365 personal
  accounts, Gmail).
- Unified folders (Inbox, Sent, Drafts…) or separate accounts; full folder management (create,
  rename, move, delete, empty).
- Reading: sandboxed HTML with no scripts, remote images blocked by default, attachments (open,
  save), calendar invitations added in one click.
- Writing: HTML or plain text (default format configurable), per-account signatures, reply and
  forward, drag-and-drop attachments, automatic draft saving, recipient suggestions,
  **forgotten-attachment warning**.
- Moving and deleting across accounts, with a journal: an interrupted move (network loss, app
  closed) is resumed at next start-up, without duplicates or loss.
- Search across every folder of every account, message bodies included.
- Real-time new mail (IMAP IDLE) and desktop notifications; configurable automatic sync,
  incremental on CONDSTORE / QRESYNC servers (fast on large folders).
- **Rules**: conditions (sender, recipients, subject, start of the body, has attachment) and actions
  (move to a folder of any account, mark as read, flag, delete); run on new mail or on demand;
  create a rule straight from a message (right-click).
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

Tests (core logic, IMAP protocol against a fake server, application):

```bash
cargo test
```

Release builds, installers, the Windows portable ZIP and troubleshooting are covered in
`docs/COMPILATION.md` (in French).

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
  src/idle.rs            real-time new mail, notifications
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

Next:
1. Fixes from the November 2026 code review (security, data safety, calendar time zones).
2. Background operation: rules for mail received while the app was closed, system tray.
3. O365 work and school accounts (today: personal Outlook.com / O365 accounts only).
4. S/MIME: signing and encryption, certificate import and export.

## Known limitations

- Google sign-in uses your own OAuth app; while that app is in Google's "testing" status, Gmail
  tokens expire after 7 days and a one-click reconnect banner appears.
- Reminders and rules only run while Vortex Mail is open.
- No automated tests for the interface yet (the Rust side has 126 tests).

## Contributing

Issues and pull requests are welcome. Any new or changed interface text must go through `t()`
with entries in `src/locales/en.ts` and `src/locales/es.ts`; texts produced by the Rust backend go
through `tr!` with entries in `src-tauri/src/locales/`.

## License

To be defined.
