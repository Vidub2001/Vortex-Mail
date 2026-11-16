# Compilar Vortex Mail

[Français](COMPILATION.md) · [English](COMPILATION.en.md) · **Español**

Esta guía explica cómo compilar Vortex Mail a partir del código fuente, en **Linux** (Ubuntu / Debian)
y en **Windows 10 / 11**, y luego cómo producir un instalador.

Vortex Mail es una aplicación [Tauri 2](https://tauri.app): un núcleo en **Rust** (`src-tauri/`,
`crates/`) y una interfaz **React + TypeScript** (`src/`) mostrada por el motor web del sistema
(WebKitGTK en Linux, WebView2 en Windows). El mismo código sirve para ambos sistemas.

> Una aplicación de Windows se compila **en Windows** (máquina física o virtual).
> La compilación cruzada desde Linux existe, pero sigue siendo experimental con Tauri: no se
> describe aquí.

---

## Índice

1. [Estructura del proyecto](#1-estructura-del-proyecto)
2. [Linux](#2-linux)
3. [Windows](#3-windows)
4. [Pruebas](#4-pruebas)
5. [Actualizar una instalación](#5-actualizar-una-instalación)
6. [Conviene saber](#6-conviene-saber)
7. [Solución de problemas](#7-solución-de-problemas)
8. [Anexo: compilar en WinApps](#8-anexo-compilar-en-winapps-windows-en-un-contenedor)

---

## 1. Estructura del proyecto

| Carpeta / archivo            | Contenido |
|------------------------------|-----------|
| `src/`                       | Interfaz (React 18, Fluent UI v9, TypeScript) |
| `src-tauri/`                 | Aplicación Tauri (Rust): enrutador IMAP, caché SQLite, calendario, contactos, OAuth, copia de seguridad |
| `src-tauri/tauri.conf.json`  | Nombre, versión, identificador, iconos, opciones de los instaladores |
| `crates/imap-lite/`          | Cliente IMAP escrito para el proyecto |
| `crates/mail-core/`          | Modelo de datos, movimientos entre cuentas, composición de mensajes |
| `Cargo.toml`                 | Espacio de trabajo Rust (los tres paquetes anteriores) |
| `package.json`               | Dependencias y scripts de la interfaz |
| `target/`                    | Archivos de compilación e instaladores generados (no versionado) |

---

## 2. Linux

Probado en Ubuntu 26.04. Los comandos valen para cualquier distribución derivada de Debian;
para las demás, hay que adaptar los nombres de los paquetes.

### 2.1 Requisitos

**Paquetes del sistema** (bibliotecas de WebKitGTK y del llavero, herramientas de compilación):

```bash
sudo apt update
sudo apt install -y build-essential curl wget file pkg-config git \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev \
  libdbus-1-dev libnotify-bin
```

| Paquete | Por qué |
|---------|---------|
| `build-essential`, `pkg-config` | Compilador de C (SQLite se compila con la aplicación) |
| `libwebkit2gtk-4.1-dev` | Motor web de la interfaz |
| `libssl-dev` | TLS (conexiones IMAP, SMTP, HTTPS) y cifrado de la caché (SQLCipher, lote G4) |
| `libdbus-1-dev` | Llavero del sistema (Secret Service: GNOME Keyring, KWallet) |
| `libnotify-bin` | Notificaciones de correo nuevo (`notify-send`) |
| `libxdo-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev` | Dependencias de Tauri (`libayatana-appindicator3`: icono del área de notificación, lote B3) |

**Rust** (mediante rustup; la versión del proyecto, fijada por `rust-toolchain.toml`, se instala sola en el primer `cargo`):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version
```

**Node.js** (versión LTS reciente, 20 o posterior):

```bash
# por ejemplo con NodeSource, o el gestor que prefieras (nvm, fnm…)
curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash -
sudo apt install -y nodejs
node --version
```

### 2.2 Obtener el código fuente

```bash
mkdir -p ~/proyectos && cd ~/proyectos
git clone https://github.com/Vidub2001/Vortex-Mail.git vortexmail
cd vortexmail
npm install
```

### 2.3 Ejecutar en modo desarrollo

```bash
npm run tauri dev
```

- La primera compilación tarda unos minutos; las siguientes, unos segundos.
- Cualquier cambio en `src/` se recarga al instante; un cambio en el código Rust reinicia la aplicación.
- El terminal debe permanecer abierto; `Ctrl+C` cierra la aplicación.

Solo la interfaz, en un navegador, sin Rust (una maqueta sustituye al núcleo):

```bash
npm run dev
# luego abrir http://localhost:1420
```

Para acceder desde **otra máquina de la red local** (tableta, otro PC…), el servidor debe
escuchar en todas las interfaces, no solo en `localhost`:

```bash
npm run dev -- --host
```

Vite muestra entonces la dirección que hay que usar, del tipo `Network: http://<IP-del-PC>:1420/`
(por ejemplo `http://192.168.1.50:1420`). Si no responde, abrir el puerto 1420 en el
cortafuegos, solo para la red local:

```bash
sudo ufw allow from 192.168.1.0/24 to any port 1420 proto tcp   # adaptar a la red local
```

En el navegador siempre se muestra la **maqueta** (cuentas y mensajes de ejemplo):
las cuentas reales necesitan la aplicación (`npm run tauri dev` o la versión instalada).

### 2.4 Producir un paquete instalable

```bash
npm run tauri build -- --bundles deb
```

Resultado: `target/release/bundle/deb/Vortex Mail_<versión>_amd64.deb`

Instalación:

```bash
sudo apt install ./target/release/bundle/deb/*.deb
```

La aplicación aparece en el menú como «Vortex Mail». Recupera la configuración de la
versión de desarrollo (mismo identificador): no hay nada que volver a importar.

Otros formatos posibles: `--bundles appimage` (un único archivo ejecutable), `--bundles rpm`.
Sin la opción `--bundles`, se generan los formatos indicados en `tauri.conf.json` (`deb` y `appimage`).

---

## 3. Windows

Probado en Windows 11. También funciona en un Windows 10 actualizado.

> **Equipo de empresa**: si la seguridad de Windows la gestiona un departamento de informática, la
> compilación puede quedar bloqueada («Acción bloqueada», `Acceso denegado (os error 5)`): Rust
> ejecuta programas que acaba de crear, y esas políticas lo prohíben. No hay que intentar
> saltárselas. Compila en una máquina personal o en una máquina virtual (véase el anexo).

### 3.1 Requisitos

Unos **6 GB** de espacio en disco para las herramientas, más 3 a 4 GB durante la compilación.

En un **Terminal (PowerShell) de administrador**, un comando cada vez, esperando a que termine
cada uno (pegar varias líneas de golpe puede interrumpir un instalador):

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

| Herramienta | Por qué |
|-------------|---------|
| Build Tools 2022 (compilador C++ MSVC + SDK de Windows) | Compilador y enlazador que usa Rust en Windows |
| Rust (rustup, destino `msvc`; versión fijada por `rust-toolchain.toml`, instalada sola) | Compilación del núcleo |
| Node.js LTS | Compilación de la interfaz |
| Git | Obtención del código fuente |
| Strawberry Perl | Compilación de OpenSSL, que SQLCipher usa para cifrar la caché (lote G4); el Perl incluido con Git no sirve |
| WebView2 | Motor web de la interfaz: **ya presente** en un Windows 10 actualizado y en Windows 11 |

Las Build Tools pueden pedir un reinicio: acéptalo.

Después **cierra y vuelve a abrir** el Terminal (para que se encuentren las nuevas herramientas):

```powershell
rustup set profile minimal
rustup default stable-msvc
rustup --version
node --version
git --version
```

Una sola vez, autoriza los scripts locales de PowerShell (si no, `npm` se rechaza):

```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

### 3.2 Obtener el código fuente

Coloca el proyecto en un disco local (no en una carpeta compartida de red: la compilación sería muy lenta).

```powershell
cd C:\
git clone https://github.com/Vidub2001/Vortex-Mail.git vortexmail
cd vortexmail
npm install
npm install-scripts approve esbuild
npm rebuild esbuild
```

Las dos últimas líneas autorizan el script de instalación de `esbuild` (la herramienta que compila
la interfaz), bloqueado por defecto en las versiones recientes de npm.

El repositorio es público: no se pide ninguna identificación.

### 3.3 Producir el instalador

```powershell
npm run tauri build -- --bundles nsis
```

- Primera compilación: de 5 a 10 minutos. Las siguientes: menos de un minuto.
- Resultado: `C:\vortexmail\target\release\bundle\nsis\Vortex Mail_<versión>_x64-setup.exe`
- También se genera el ejecutable solo: `C:\vortexmail\target\release\vortexmail.exe`

El instalador, en francés, instala Vortex Mail **para el usuario actual, sin derechos de
administrador** (accesos directos en el escritorio y en el menú Inicio, desinstalación desde
«Aplicaciones instaladas»).

Mientras el programa no esté firmado, Windows SmartScreen muestra «Editor desconocido» en el
primer inicio: «Más información» → «Ejecutar de todas formas».

Modo desarrollo, como en Linux: `npm run tauri dev`.

### 3.4 Versión portátil

Vortex Mail sin instalación, por ejemplo en una memoria USB: todos los datos están en una
carpeta `data` junto al programa, nada en el perfil de Windows ni en el almacén de credenciales
de Windows.

Compilar y empaquetar en ZIP, con un solo comando (en la raíz del proyecto):

```powershell
.\scripts\portable.ps1
```

El script compila el ejecutable solo (sin instalador), prepara una carpeta limpia
(`vortexmail.exe` + `portable.txt`, sin carpeta `data`) y genera
`C:\vortexmail\target\VortexMail-portable-<versión>.zip`. Basta con descomprimir ese ZIP donde
se quiera (memoria USB, otro PC) y ejecutar `vortexmail.exe`.

Si PowerShell rechaza el script («la ejecución de scripts está deshabilitada»), autorízalo para
el usuario actual: `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`.

- Lo que activa el modo portátil es la presencia del archivo **`portable.txt`** junto a
  `vortexmail.exe`; sin él, el mismo programa funciona como una instalación normal.
- En el primer inicio, Vortex Mail pide **elegir una contraseña maestra**. Cifra las
  contraseñas y los tokens de las cuentas (`data\secrets.vault`); se pide en cada arranque
  y se cambia en Configuración → Copia de seguridad. **No se puede recuperar**: si se olvida,
  hay que borrar la carpeta `data` y volver a introducir las cuentas (o importar una copia de seguridad).
- Contenido de la carpeta `data`: `config\accounts.json` (cuentas, sin contraseñas),
  `cache.sqlite` (correo, calendario, contactos), `secrets.vault` (cofre cifrado), `webview\`
  (ajustes de la interfaz).
- Para retomar una instalación existente: Configuración → Copia de seguridad → Exportar en la
  instalación, y luego Importar en la versión portátil.
- Requisito en el PC: el motor web **WebView2**, presente de serie en Windows 10 y 11.
- Los adjuntos abiertos pasan por la carpeta temporal del PC, que se vacía en el siguiente arranque.

---

## 4. Pruebas

Desde la raíz del proyecto, tanto en Linux como en Windows:

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # aplicación: caché, enrutador, calendario, cifrado…
cargo test -p imap-lite                           # cliente IMAP, con un falso servidor
cargo test -p mail-core                           # movimientos entre cuentas, composición
```

`cargo test` solo acepta un filtro de nombre cada vez:

```bash
cargo test --manifest-path src-tauri/Cargo.toml crypto::
```

Comprobación de tipos de la interfaz:

```bash
npx tsc --noEmit
```

Lista de licencias de las dependencias (regenera `THIRD_PARTY_LICENSES.txt` y la página «Acerca de»):

```bash
npm run licenses
```

---

## 5. Actualizar una instalación

**Linux**

```bash
cd ~/proyectos/vortexmail
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

Comprueba que `git log` muestra el último commit: si `git pull` ha fallado (identificación,
archivo modificado localmente), la compilación produce la versión **antigua**.

Después ejecuta el nuevo `setup.exe`: sustituye la versión anterior y conserva la configuración.

Versión portátil: `.\scripts\portable.ps1` (véase 3.4), y luego descomprimir el nuevo ZIP sobre la
carpeta anterior; la carpeta `data` (cuentas, ajustes) se conserva.

Para liberar el espacio que ocupa la compilación (2 a 4 GB): `cargo clean` en la raíz del proyecto.

---

## 6. Conviene saber

### Dónde están los datos

| | Linux | Windows |
|---|---|---|
| Cuentas (`accounts.json`, sin contraseñas) | `~/.config/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Caché de correo, calendario y contactos (`cache.sqlite`) | `~/.local/share/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Contraseñas y tokens OAuth | Llavero (GNOME Keyring, «Contraseñas y claves»), servicio `vortexmail` | Almacén de credenciales de Windows, entradas `…vortexmail` |

Los tokens demasiado largos para el almacén de credenciales de Windows se reparten en varias
entradas (`clave#1`, `clave#2`…) y se recomponen automáticamente.

### Nombres que no se deben cambiar

- El identificador `fr.vortexmail.client` (`tauri.conf.json`): determina las carpetas anteriores.
- El nombre del servicio del llavero `vortexmail` (`src-tauri/src/secrets.rs`).

Cambiarlos haría perder cuentas, caché y contraseñas en todas las instalaciones existentes,
salvo que se prevea una migración.

### Recuperación de las instalaciones «FluxMail»

Las primeras versiones se llamaban FluxMail (identificador `fr.fluxmail.client`, servicio del
llavero `fluxmail`). En el primer arranque de una versión renombrada, todo se recupera sin hacer nada:

- las carpetas `fr.fluxmail.client` (cuentas, caché, ajustes de la interfaz) se renombran a
  `fr.vortexmail.client`, salvo si la nueva carpeta ya contiene datos (no se sobrescribe nada);
- cada contraseña o token se copia del servicio `fluxmail` a `vortexmail` la primera vez que se
  lee, y después se borra la entrada antigua;
- los ajustes `fluxmail.*` del almacenamiento de la interfaz pasan a ser `vortex.*`.

### Pasar de una máquina a otra

Configuración → **Copia de seguridad** → Exportar: un archivo `.vxbackup` cifrado con contraseña
(cuentas, contraseñas, conexiones de Outlook.com y Google, ajustes, calendario y contactos locales).
En la otra instalación, Linux o Windows: Configuración → Copia de seguridad → Importar.

### Versión

La versión mostrada y la de los instaladores vienen de `"version"` en
`src-tauri/tauri.conf.json`.

---

## 7. Solución de problemas

| Síntoma | Causa | Solución |
|---------|-------|----------|
| `failed to run custom build command … Acceso denegado (os error 5)` y notificación «Acción bloqueada» (Windows) | Política de seguridad de empresa | Compilar en una máquina personal o una VM (véase el anexo) |
| `npm : No se puede cargar el archivo … npm.ps1` (Windows) | Scripts de PowerShell deshabilitados | `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` |
| `npm warn install-scripts … esbuild` | Script de instalación bloqueado por npm | `npm install-scripts approve esbuild` y luego `npm rebuild esbuild` |
| `Your local changes to the following files would be overwritten by merge: package.json` (git pull) | `package.json` modificado por npm en esta máquina | `git checkout -- package.json` y luego `git pull` |
| Se genera `fluxmail` / `fluxmail.exe` | Código fuente desactualizado (`git pull` fallido) | Corregir el `git pull`, comprobar `git log -1 --oneline`, volver a compilar |
| `la ejecución de scripts está deshabilitada` (`portable.ps1`) | Scripts de PowerShell deshabilitados | `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` |
| `0x800704c7 : Se canceló la operación` (winget) | Instalación interrumpida (líneas pegadas seguidas, ventana de autorización cerrada) | Volver a ejecutar el comando solo |
| `Failed when opening source(s)` / `0x8a15000f` (winget) | Catálogo de winget ausente o red sin DNS | Comprobar el DNS (`Resolve-DnsName github.com`), y luego `winget source reset --force` y `winget source update` |
| `failed to build bundler settings: invalid category` | Categoría desconocida en `tauri.conf.json` | Usar un valor reconocido (`Productivity`) |
| Contraseñas perdidas en cada cierre | Llavero no disponible | Linux: comprobar que GNOME Keyring está en marcha; Windows: comprobar que la versión compilada incluye `windows-native` (`src-tauri/Cargo.toml`) |
| Sin notificaciones (Linux) | Falta `notify-send` | `sudo apt install libnotify-bin` |
| `error: linker 'link.exe' not found` (Windows) | Build Tools ausentes o incompletas | Reinstalar las Build Tools con el componente `VC.Tools.x86.x64` |

---

## 8. Anexo: compilar en WinApps (Windows en un contenedor)

[WinApps](https://github.com/winapps-org/winapps) ejecuta Windows en un contenedor
Docker (imagen `dockur/windows`) en el equipo Linux. Es un entorno ideal para compilar y
probar la versión de Windows sin una máquina adicional.

**Recursos**: en `~/winapps/compose.yaml`, prever al menos:

```yaml
      RAM_SIZE: "8G"
      CPU_CORES: "8"
      DISK_SIZE: "64G"
```

**DNS**: si Windows no accede a Internet por nombre (winget falla, `Resolve-DnsName`
falla aunque `ping 1.1.1.1` responde), dar servidores DNS al contenedor en el
servicio `windows` de `compose.yaml`:

```yaml
    dns:
      - 1.1.1.1
      - 8.8.8.8
```

y luego `docker compose up -d`. Comprobación: `docker exec WinApps cat /etc/resolv.conf` (la línea
`ExtServers` debe mostrar direcciones reales).

**Puerto RDP**: si el escritorio remoto de GNOME ya usa el puerto 3389 del equipo, publicar
el de Windows en el 3390 en `compose.yaml`:

```yaml
      - "127.0.0.1:3390:3389/tcp"
      - "127.0.0.1:3390:3389/udp"
```

e indicárselo a WinApps:

```bash
echo 'RDP_PORT="3390"' >> ~/.config/winapps/winapps.conf
```

**Acceso al escritorio de Windows**: `winapps windows`, o un cliente RDP (Remmina) en `127.0.0.1:3390`
con las credenciales `RDP_USER` / `RDP_PASS` de `~/.config/winapps/winapps.conf`, o el
navegador en http://127.0.0.1:8006.

**Intercambiar archivos**: la carpeta personal de Linux se ve en Windows como
`\\host.lan\Data` (acceso directo «Shared» en el escritorio).

Después, seguir la sección [3. Windows](#3-windows).
