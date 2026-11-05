# Vortex Mail

[Français](README.fr.md) · [English](README.md) · **Español**

Cliente de correo de escritorio multicuenta para Linux y Windows, escrito en Rust (Tauri 2) con una
interfaz React 18 / Fluent UI v9. Correo IMAP/SMTP, calendarios y contactos sincronizados, e
interfaz en francés, inglés y español.

Lo que lo distingue: un mensaje puede archivarse en una carpeta **de cualquier cuenta**, a mano
(arrastrar y soltar, «Mover a») o automáticamente mediante reglas. Los movimientos entre cuentas
quedan registrados en un diario: no se pierde ni se duplica nada si la red se corta a mitad.

> **Estado:** primera versión (0.1.0), usada a diario por su autor. El código fuente se publicará
> aquí próximamente.

## Funciones

**Correo**
- Cuentas IMAP/SMTP: SSL/TLS o STARTTLS; contraseña o inicio de sesión OAuth2 (Outlook.com, cuentas
  personales de O365, Gmail).
- Carpetas unificadas (Bandeja de entrada, Enviados, Borradores…) o cuentas separadas; gestión
  completa de carpetas (crear, renombrar, mover, eliminar, vaciar).
- Lectura: HTML aislado y sin scripts, imágenes remotas bloqueadas por defecto, adjuntos (abrir,
  guardar), invitaciones de calendario añadidas con un clic.
- Redacción: HTML o texto sin formato (formato predeterminado configurable), firmas por cuenta,
  responder y reenviar, adjuntos con arrastrar y soltar, borradores guardados automáticamente,
  sugerencias de destinatarios, **aviso de adjunto olvidado**.
- Mover y eliminar entre cuentas, con diario: un movimiento interrumpido (corte de red, aplicación
  cerrada) se reanuda en el siguiente inicio, sin duplicados ni pérdidas.
- Búsqueda en todas las carpetas de todas las cuentas, incluido el contenido de los mensajes.
- Correo nuevo en tiempo real (IMAP IDLE) y notificaciones de escritorio; sincronización automática
  configurable, incremental en servidores CONDSTORE / QRESYNC (rápida en carpetas grandes).
- **Reglas**: condiciones (remitente, destinatarios, asunto, inicio del mensaje, tiene adjunto) y
  acciones (mover a una carpeta de cualquier cuenta, marcar como leído, marcar con indicador,
  eliminar); se aplican al correo nuevo o a petición; se pueden crear directamente desde un mensaje
  (clic derecho).
- Sin conexión: caché local completa; las acciones se ponen en cola y se repiten cuando vuelve la
  red.

**Calendario y contactos**
- Gmail (OAuth2 de Google, CalDAV/CardDAV), Outlook.com, Mailo/Netcourrier y cualquier servidor
  CalDAV/CardDAV; además, calendarios y libretas de direcciones locales «En este equipo».
- Vistas de semana, mes y año; colores por calendario; números de semana (ISO 8601, semana que
  contiene el 1 de enero o primera semana completa); primer día de la semana y escala de la
  cuadrícula configurables.
- Eventos: repetición (incluidas reglas personalizadas: días de la semana, intervalo, día del mes,
  fin), **recordatorios** (notificaciones de escritorio, sincronizados con los servidores).
- Contactos por cuenta, con nombre y apellidos (orden de visualización configurable).

**Configuración**
- Idioma de la interfaz y de los mensajes de error (francés, inglés, español), tema
  claro/oscuro/del sistema, formatos de fecha y hora, disposición del panel de lectura (a la
  derecha, abajo, oculto).
- **Copia de seguridad** cifrada de toda la configuración (cuentas, contraseñas, ajustes, reglas,
  calendarios y contactos locales) para instalar Vortex Mail en otro equipo.
- **Modo portátil** para Windows: todos los datos junto al ejecutable, protegidos por una
  contraseña maestra.

**Seguridad**
- Ninguna contraseña ni token se escribe en un archivo sin cifrar: todo se guarda en el llavero del
  sistema (GNOME Keyring / Secret Service en Linux, Administrador de credenciales en Windows), o en
  el almacén cifrado en modo portátil.
- Copias de seguridad cifradas (PBKDF2-HMAC-SHA256, ChaCha20, HMAC).

## Primeros pasos

Requisitos: Rust (estable), Node.js 18+ y las dependencias de sistema de Tauri 2 para su plataforma
(WebKitGTK en Linux, WebView2 en Windows).

```bash
git clone <url-del-repositorio> vortexmail
cd vortexmail
npm install
npm run tauri dev
```

La primera ejecución compila todas las dependencias de Rust (unos minutos); las siguientes son
rápidas.

Para trabajar solo en la interfaz, sin Rust, con recarga instantánea en el navegador:

```bash
npm run dev
# después abrir http://localhost:1420 (una maqueta en JavaScript sustituye al backend)
```

Pruebas (lógica principal, protocolo IMAP contra un servidor simulado, aplicación):

```bash
cargo test
```

La compilación final, los instaladores, el ZIP portátil para Windows y la resolución de problemas
se describen en `docs/COMPILATION.md` (en francés).

## Añadir una cuenta

Configuración (icono de engranaje) → Cuentas → «Añadir una cuenta». Para los proveedores conocidos
(Gmail, Outlook.com, Mailo…), los datos del servidor se rellenan automáticamente. Tipos de conexión:

| Opción   | IMAP | SMTP | Funcionamiento |
|----------|------|------|----------------|
| SSL/TLS  | 993  | 465  | cifrado desde el inicio |
| STARTTLS | 143  | 587  | conexión sin cifrar que pasa a TLS |
| Auto     |      |      | SSL/TLS en los puertos 993/465, si no STARTTLS |
| Ninguno  | 143  | 25   | contraseña enviada sin cifrar, no recomendado |

«Probar la configuración» comprueba IMAP y SMTP sin guardar nada.

**OAuth2 (Gmail, Outlook.com):** Vortex Mail no incluye credenciales de cliente OAuth.
Registre su propia aplicación (consola de Google Cloud para Gmail, portal de desarrolladores de
Outlook.com para Outlook.com) e introduzca su ID de cliente (y su secreto, en el caso de Google)
en la configuración de la cuenta; se guardan en el llavero y no se vuelven a mostrar.

## Ubicación de los datos

| | Linux | Windows |
|---|---|---|
| Cuentas (`accounts.json`, sin contraseñas) | `~/.config/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Caché de correo, calendario y contactos (`cache.sqlite`) | `~/.local/share/fr.vortexmail.client/` | `%APPDATA%\fr.vortexmail.client\` |
| Contraseñas y tokens | llavero, servicio `vortexmail` | Administrador de credenciales |
| Modo portátil | — | carpeta `data\` junto al ejecutable |

La caché puede borrarse sin riesgo: se reconstruye desde los servidores. Las instalaciones hechas
con el nombre anterior (FluxMail) se migran automáticamente en el primer inicio.

## Estructura del proyecto

```
crates/imap-lite/        cliente IMAP sin dependencias: protocolo, literales, UTF-7, IDLE, fechas
crates/mail-core/        lógica de negocio, sin interfaz (probada por separado)
  src/model.rs           cuentas, carpetas, mensajes
  src/backend.rs         trait MailBackend (implementado por la demo y por el router IMAP)
  src/ops.rs             movimientos dentro de una cuenta y entre cuentas + diario
  src/compose.rs         construcción de mensajes (MIME, adjuntos, imágenes integradas)
  tests/                 pruebas de movimientos
src-tauri/               capa Tauri (Rust)
  src/router.rs          cuentas IMAP detrás de MailBackend, sincronización, búsqueda
  src/transport.rs       TCP, SSL/TLS, STARTTLS, autenticación IMAP y SMTP, envío
  src/cache.rs           caché SQLite (carpetas, cabeceras, cuerpos, calendarios, contactos)
  src/journal.rs         acciones sin conexión y movimientos pendientes
  src/idle.rs            correo nuevo en tiempo real, notificaciones
  src/oauth.rs           OAuth2 de Outlook.com (correo)
  src/o365.rs            calendario y contactos de Outlook.com
  src/endpoints.rs       direcciones técnicas de los servicios
  src/google.rs          OAuth2 de Google (correo, calendario y contactos de Gmail)
  src/dav.rs, pim.rs     CalDAV / CardDAV, sincronización de calendario y contactos
  src/pimfmt.rs          iCalendar y vCard
  src/secrets.rs         contraseñas y tokens en el llavero (o en el almacén portátil)
  src/crypto.rs          cifrado de las copias de seguridad
  src/backup.rs          exportación / importación de la configuración
  src/migrate.rs         migración de las instalaciones de FluxMail
  src/portable.rs        modo portátil (marcador portable.txt, carpeta data)
  src/windows.rs         creación de ventanas
  src/i18n.rs, locales/  idioma de los mensajes generados por el backend (macro tr!)
  src/commands.rs        comandos llamados desde la interfaz
src/                     interfaz React + Fluent UI
  App.tsx, Popup.tsx     ventana principal, ventanas de mensaje y de redacción
  api.ts                 llamadas al backend (o maqueta fuera de Tauri)
  i18n.ts, locales/      idiomas (el francés es el idioma de origen; en.ts y es.ts)
  general.ts, settings.ts  configuración
  rules.ts               reglas de correo
  calendar.ts, recurrence.ts, reminders.ts  calendario: ocurrencias, repetición, recordatorios
  components/            paneles, listas, redacción, calendario, contactos, configuración
docs/COMPILATION.md      guía de compilación para Linux y Windows (en francés)
```

## Hoja de ruta

Hecho:
- ~~Diario persistente de movimientos entre cuentas; búsqueda en todas las carpetas.~~
- ~~Notificaciones IDLE (correo nuevo en tiempo real).~~
- ~~Redacción y envío (SMTP), responder y reenviar.~~
- ~~Adjuntos: lista, abrir, guardar.~~
- ~~OAuth2 de Outlook.com.~~
- ~~Calendario y contactos (Gmail, Outlook.com, Mailo, CalDAV/CardDAV).~~
- ~~Copia de seguridad y restauración de la configuración; versión para Windows.~~
- ~~Configuración general, reglas, recordatorios, aviso de adjunto, interfaz en tres idiomas.~~
- ~~CONDSTORE / QRESYNC: sincronización incremental de carpetas grandes.~~
- ~~OAuth2 de Google para el correo de Gmail.~~
- ~~Mensajes de error traducidos al inglés y al español.~~
- ~~Versión portátil para Windows (datos junto al ejecutable, contraseña maestra).~~

Próximamente:
1. Correcciones derivadas de la revisión de código de noviembre de 2026 (seguridad, protección de
   datos, zonas horarias del calendario).
2. Funcionamiento en segundo plano: reglas para el correo recibido con la aplicación cerrada, icono
   en la bandeja del sistema.
3. Cuentas profesionales y educativas de O365 (hoy: solo cuentas personales de Outlook.com / O365).
4. S/MIME: firma y cifrado, importación y exportación de certificados.

## Limitaciones conocidas

- El inicio de sesión de Google usa su propia aplicación OAuth; mientras esa aplicación esté en
  modo «prueba» en Google, los tokens de Gmail caducan a los 7 días y aparece un aviso para
  reconectar con un clic.
- Los recordatorios y las reglas solo funcionan mientras Vortex Mail está abierto.
- Todavía no hay pruebas automáticas para la interfaz (la parte en Rust tiene 126 pruebas).

## Contribuir

Las incidencias y las contribuciones son bienvenidas. Todo texto de la interfaz, nuevo o
modificado, pasa por `t()` con sus entradas en `src/locales/en.ts` y `src/locales/es.ts`; los
textos generados por el backend en Rust pasan por `tr!` con sus entradas en
`src-tauri/src/locales/`.

## Licencia

Por definir.
