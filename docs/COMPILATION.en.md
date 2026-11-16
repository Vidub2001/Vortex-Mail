# Building Vortex Mail

[Français](COMPILATION.md) · **English** · [Español](COMPILATION.es.md)

This guide explains how to build Vortex Mail from source on **Linux** (Ubuntu / Debian)
and on **Windows 10 / 11**, then how to produce an installer.

Vortex Mail is a [Tauri 2](https://tauri.app) application: a **Rust** core (`src-tauri/`,
`crates/`) and a **React + TypeScript** interface (`src/`) displayed by the system's web engine
(WebKitGTK on Linux, WebView2 on Windows). The same code serves both systems.

> A Windows application is built **on Windows** (physical or virtual machine).
> Cross-compiling from Linux exists but is still experimental with Tauri: it is not
> covered here.

---

## Contents

1. [Project layout](#1-project-layout)
2. [Linux](#2-linux)
3. [Windows](#3-windows)
4. [Tests](#4-tests)
5. [Updating an installation](#5-updating-an-installation)
6. [Good to know](#6-good-to-know)
7. [Troubleshooting](#7-troubleshooting)
8. [Appendix: building in WinApps](#8-appendix-building-in-winapps-windows-in-a-container)

---

## 1. Project layout

| Folder / file                | Contents |
|------------------------------|----------|
| `src/`                       | Interface (React 18, Fluent UI v9, TypeScript) |
| `src-tauri/`                 | Tauri application (Rust): IMAP router, SQLite cache, calendar, contacts, OAuth, backup |
| `src-tauri/tauri.conf.json`  | Name, version, identifier, icons, installer options |
| `crates/imap-lite/`          | IMAP client written for the project |
| `crates/mail-core/`          | Data model, cross-account moves, message composition |
| `Cargo.toml`                 | Rust workspace (the three packages above) |
| `package.json`               | Interface dependencies and scripts |
| `target/`                    | Build files and produced installers (not versioned) |

---

## 2. Linux

Tested on Ubuntu 26.04. The commands apply to any Debian-based distribution;
for others, adapt the package names.

### 2.1 Requirements

**System packages** (WebKitGTK and keyring libraries, build tools):

```bash
sudo apt update
sudo apt install -y build-essential curl wget file pkg-config git \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev \
  libdbus-1-dev libnotify-bin
```

| Package | Why |
|---------|-----|
| `build-essential`, `pkg-config` | C compiler (SQLite is compiled with the application) |
| `libwebkit2gtk-4.1-dev` | Web engine of the interface |
| `libssl-dev` | TLS (IMAP, SMTP, HTTPS connections) and cache encryption (SQLCipher, batch G4) |
| `libdbus-1-dev` | System keyring (Secret Service: GNOME Keyring, KWallet) |
| `libnotify-bin` | New-mail notifications (`notify-send`) |
| `libxdo-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev` | Tauri dependencies (`libayatana-appindicator3`: notification-area icon, batch B3) |

**Rust** (through rustup; the project's version, pinned by `rust-toolchain.toml`, installs itself on the first `cargo` run):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version
```

**Node.js** (recent LTS version, 20 or later):

```bash
# for example with NodeSource, or the manager of your choice (nvm, fnm…)
curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash -
sudo apt install -y nodejs
node --version
```

### 2.2 Getting the sources

```bash
mkdir -p ~/projects && cd ~/projects
git clone https://github.com/Vidub2001/Vortex-Mail.git vortexmail
cd vortexmail
npm install
```

### 2.3 Running in development mode

```bash
npm run tauri dev
```

- The first build takes a few minutes, later ones a few seconds.
- Any change in `src/` is reloaded at once; a change in the Rust code restarts the application.
- The terminal must stay open; `Ctrl+C` closes the application.

Interface only, in a browser, without Rust (a mock replaces the core):

```bash
npm run dev
# then open http://localhost:1420
```

To reach it from **another machine on the local network** (tablet, other PC…), the server must
listen on every interface, not only on `localhost`:

```bash
npm run dev -- --host
```

Vite then shows the address to use, in the form `Network: http://<PC-IP-address>:1420/`
(for example `http://192.168.1.50:1420`). If it does not answer, open port 1420 in the
firewall, for the local network only:

```bash
sudo ufw allow from 192.168.1.0/24 to any port 1420 proto tcp   # adapt to your local network
```

In the browser, the **mock** is always shown (sample accounts and messages):
real accounts need the application (`npm run tauri dev` or the installed version).

### 2.4 Producing an installable package

```bash
npm run tauri build -- --bundles deb
```

Result: `target/release/bundle/deb/Vortex Mail_<version>_amd64.deb`

Installation:

```bash
sudo apt install ./target/release/bundle/deb/*.deb
```

The application appears in the menu as "Vortex Mail". It picks up the configuration of the
development version (same identifier); nothing needs to be imported again.

Other possible formats: `--bundles appimage` (single executable file), `--bundles rpm`.
Without the `--bundles` option, the formats listed in `tauri.conf.json` are built (`deb` and `appimage`).

---

## 3. Windows

Tested on Windows 11. Also works on an up-to-date Windows 10.

> **Company computer**: if Windows security is managed by an IT department, the build may be
> blocked ("Action blocked", `Access is denied (os error 5)`): Rust runs programs it has just
> created, and these policies forbid it. Do not try to get around them. Build on a personal
> machine or in a virtual machine (see the appendix).

### 3.1 Requirements

About **6 GB** of disk space for the tools, plus 3 to 4 GB during the build.

In an **administrator Terminal (PowerShell)**, one command at a time, waiting for each one to
finish (pasting several lines at once can interrupt an installer):

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

| Tool | Why |
|------|-----|
| Build Tools 2022 (MSVC C++ compiler + Windows SDK) | Compiler and linker used by Rust on Windows |
| Rust (rustup, `msvc` target; version pinned by `rust-toolchain.toml`, installed automatically) | Building the core |
| Node.js LTS | Building the interface |
| Git | Getting the sources |
| Strawberry Perl | Building OpenSSL, used by SQLCipher to encrypt the cache (batch G4); the Perl shipped with Git is not suitable |
| WebView2 | Web engine of the interface: **already present** on an up-to-date Windows 10 and on Windows 11 |

The Build Tools may ask for a restart: accept it.

Then **close and reopen** the Terminal (so that the new tools are found):

```powershell
rustup set profile minimal
rustup default stable-msvc
rustup --version
node --version
git --version
```

Once, allow local PowerShell scripts (otherwise `npm` is refused):

```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

### 3.2 Getting the sources

Put the project on a local disk (not on a network share: the build would be very slow there).

```powershell
cd C:\
git clone https://github.com/Vidub2001/Vortex-Mail.git vortexmail
cd vortexmail
npm install
npm install-scripts approve esbuild
npm rebuild esbuild
```

The last two lines allow the install script of `esbuild` (the tool that builds the
interface), blocked by default by recent npm versions.

The repository is public: no sign-in is needed.

### 3.3 Producing the installer

```powershell
npm run tauri build -- --bundles nsis
```

- First build: 5 to 10 minutes. Later ones: less than a minute.
- Result: `C:\vortexmail\target\release\bundle\nsis\Vortex Mail_<version>_x64-setup.exe`
- The bare executable is also produced: `C:\vortexmail\target\release\vortexmail.exe`

The installer, in French, installs Vortex Mail **for the current user, without administrator
rights** (desktop and Start menu shortcuts, uninstall from "Installed apps").

As long as the program is not signed, Windows SmartScreen shows "Unknown publisher" on first
launch: "More info" → "Run anyway".

Development mode, as on Linux: `npm run tauri dev`.

### 3.4 Portable version

Vortex Mail without installation, for example on a USB stick: all the data is in a
`data` folder next to the program, nothing in the Windows profile or in the Windows credential
store.

Build and package as a ZIP, in one command (at the root of the project):

```powershell
.\scripts\portable.ps1
```

The script builds the bare executable (no installer), prepares a clean folder
(`vortexmail.exe` + `portable.txt`, no `data` folder) and produces
`C:\vortexmail\target\VortexMail-portable-<version>.zip`. Just unzip it wherever you like
(USB stick, another PC) and run `vortexmail.exe`.

If PowerShell refuses the script ("running scripts is disabled"), allow it for the
current user: `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`.

- The **`portable.txt`** file next to `vortexmail.exe` is what turns portable mode on;
  without it, the same program works like a normal installation.
- On first launch, Vortex Mail asks you to **choose a master password**. It encrypts the
  accounts' passwords and tokens (`data\secrets.vault`); it is asked again at every start
  and can be changed in Settings → Backup. **It cannot be recovered**: if you forget it,
  delete the `data` folder and enter the accounts again (or import a backup).
- Contents of the `data` folder: `config\accounts.json` (accounts, without passwords),
  `cache.sqlite` (mail, calendar, contacts), `secrets.vault` (encrypted vault), `webview\`
  (interface settings).
- To take over an existing installation: Settings → Backup → Export on the installation,
  then Import in the portable version.
- Requirement on the PC: the **WebView2** web engine, built into Windows 10 and 11.
- Opened attachments go through the PC's temporary folder, emptied at the next start.

---

## 4. Tests

From the root of the project, on Linux as on Windows:

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # application: cache, router, calendar, encryption…
cargo test -p imap-lite                           # IMAP client, including a fake server
cargo test -p mail-core                           # cross-account moves, composition
```

`cargo test` accepts only one name filter at a time:

```bash
cargo test --manifest-path src-tauri/Cargo.toml crypto::
```

Type checking of the interface:

```bash
npx tsc --noEmit
```

List of dependency licences (regenerates `THIRD_PARTY_LICENSES.txt` and the "About" page):

```bash
npm run licenses
```

---

## 5. Updating an installation

**Linux**

```bash
cd ~/projects/vortexmail
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

Check that `git log` shows the latest commit: if `git pull` failed (sign-in, file changed
locally), the build produces the **old** version.

Then run the new `setup.exe`: it replaces the old version and keeps the configuration.

Portable version: `.\scripts\portable.ps1` (see 3.4), then unzip the new ZIP over the
old folder; the `data` folder (accounts, settings) is kept.

To free the space taken by the build (2 to 4 GB): `cargo clean` at the root of the project.

---

## 6. Good to know

### Where the data is

| | Linux | Windows |
|---|---|---|
| Accounts (`accounts.json`, no passwords) | `~/.config/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Mail, calendar and contacts cache (`cache.sqlite`) | `~/.local/share/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Passwords and OAuth tokens | Keyring (GNOME Keyring, "Passwords and Keys"), service `vortexmail` | Windows credential store, entries `…vortexmail` |

Tokens too long for the Windows credential store are split over several entries
(`key#1`, `key#2`…) and put back together automatically.

### Names not to change

- The `fr.vortexmail.client` identifier (`tauri.conf.json`): it sets the folders above.
- The keyring service name `vortexmail` (`src-tauri/src/secrets.rs`).

Changing them would lose accounts, cache and passwords for every existing installation,
unless a migration is planned.

### Taking over "FluxMail" installations

The first versions were called FluxMail (identifier `fr.fluxmail.client`, keyring service
`fluxmail`). On the first start of a renamed version, everything is taken over with no action:

- the `fr.fluxmail.client` folders (accounts, cache, interface settings) are renamed to
  `fr.vortexmail.client`, unless the new folder already holds data (nothing is overwritten);
- each password or token is copied from the `fluxmail` service to `vortexmail` the first time
  it is read, then the old entry is deleted;
- the `fluxmail.*` settings of the interface storage become `vortex.*`.

### Moving from one machine to another

Settings → **Backup** → Export: a password-encrypted `.vxbackup` file (accounts,
passwords, Outlook.com and Google connections, settings, local calendar and contacts). On the
other installation, Linux or Windows: Settings → Backup → Import.

### Version

The displayed version and that of the installers come from `"version"` in
`src-tauri/tauri.conf.json`.

---

## 7. Troubleshooting

| Symptom | Cause | Solution |
|---------|-------|----------|
| `failed to run custom build command … Access is denied (os error 5)` and an "Action blocked" notification (Windows) | Company security policy | Build on a personal machine or a VM (see appendix) |
| `npm : File … npm.ps1 cannot be loaded` (Windows) | PowerShell scripts disabled | `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` |
| `npm warn install-scripts … esbuild` | Install script blocked by npm | `npm install-scripts approve esbuild` then `npm rebuild esbuild` |
| `Your local changes to the following files would be overwritten by merge: package.json` (git pull) | `package.json` changed by npm on this machine | `git checkout -- package.json` then `git pull` |
| `fluxmail` / `fluxmail.exe` built | Sources not up to date (`git pull` failed) | Fix the `git pull`, check `git log -1 --oneline`, build again |
| `running scripts is disabled` (`portable.ps1`) | PowerShell scripts disabled | `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` |
| `0x800704c7 : The operation was canceled` (winget) | Installation interrupted (lines pasted one after the other, permission window closed) | Run the command again on its own |
| `Failed when opening source(s)` / `0x8a15000f` (winget) | winget catalogue missing or network without DNS | Check DNS (`Resolve-DnsName github.com`), then `winget source reset --force` and `winget source update` |
| `failed to build bundler settings: invalid category` | Unknown category in `tauri.conf.json` | Use a recognised value (`Productivity`) |
| Passwords lost at every close | Keyring unavailable | Linux: check that GNOME Keyring is running; Windows: check that the built version includes `windows-native` (`src-tauri/Cargo.toml`) |
| No notification (Linux) | `notify-send` missing | `sudo apt install libnotify-bin` |
| `error: linker 'link.exe' not found` (Windows) | Build Tools missing or incomplete | Reinstall the Build Tools with the `VC.Tools.x86.x64` component |

---

## 8. Appendix: building in WinApps (Windows in a container)

[WinApps](https://github.com/winapps-org/winapps) runs Windows in a Docker container
(`dockur/windows` image) on the Linux workstation. It is an ideal environment to build and
test the Windows version without an extra machine.

**Resources**: in `~/winapps/compose.yaml`, allow at least:

```yaml
      RAM_SIZE: "8G"
      CPU_CORES: "8"
      DISK_SIZE: "64G"
```

**DNS**: if Windows cannot reach the Internet by name (winget failing, `Resolve-DnsName`
failing while `ping 1.1.1.1` answers), give the container DNS servers in the
`windows` service of `compose.yaml`:

```yaml
    dns:
      - 1.1.1.1
      - 8.8.8.8
```

then `docker compose up -d`. Check: `docker exec WinApps cat /etc/resolv.conf` (the
`ExtServers` line must show real addresses).

**RDP port**: if GNOME remote desktop already uses port 3389 on the workstation, publish
the Windows one on 3390 in `compose.yaml`:

```yaml
      - "127.0.0.1:3390:3389/tcp"
      - "127.0.0.1:3390:3389/udp"
```

and tell WinApps:

```bash
echo 'RDP_PORT="3390"' >> ~/.config/winapps/winapps.conf
```

**Reaching the Windows desktop**: `winapps windows`, or an RDP client (Remmina) on `127.0.0.1:3390`
with the `RDP_USER` / `RDP_PASS` credentials from `~/.config/winapps/winapps.conf`, or the
browser at http://127.0.0.1:8006.

**Exchanging files**: the Linux home folder is visible in Windows as
`\\host.lan\Data` ("Shared" shortcut on the desktop).

Then follow section [3. Windows](#3-windows).
