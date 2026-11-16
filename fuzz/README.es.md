# Fuzzing de Vortex Mail

[Français](README.md) · [English](README.en.md) · **Español**

Objetivos de `cargo-fuzz` para todo lo que llega del exterior (lote 8 del plan del proyecto). El fin: que ninguna entrada, ni siquiera una fabricada a propósito, haga entrar en pánico un hilo, desborde la pila o entre en un bucle sin fin.

| Objetivo | Lo que se lee | Código afectado |
| --- | --- | --- |
| `imap_session` | un servidor IMAP que responde cualquier cosa, durante toda una sesión | `crates/imap-lite` |
| `imap_response` | una respuesta IMAP (línea y literales) | `imap-lite/src/response.rs` |
| `imap_date` | una fecha `INTERNALDATE` | `imap-lite/src/date.rs` |
| `mime` | un mensaje recibido, como al mostrarlo, y su primer adjunto | `mimeparse.rs` (crate `mail-parser`), `attachments.rs`, `sanitize.rs` |
| `html` | un cuerpo HTML | `sanitize.rs`, `images.rs` |
| `ical` | una invitación o un calendario CalDAV, su reescritura, sus repeticiones para Outlook.com | `pimfmt.rs`, `o365.rs` |
| `vcard` | una tarjeta de contacto, su reescritura | `pimfmt.rs` |
| `json` | una respuesta de la API de Outlook.com: citas y fichas convertidas, ficha reescrita y releída, motivo de un error | `json.rs`, `o365.rs` |
| `dav_xml` | una respuesta de un servidor CalDAV o CardDAV: elementos, direcciones resueltas y comprobadas, motivo de un error | `xml.rs`, `dav.rs` |
| `url` | una dirección recibida de un servidor o de un mensaje (`Location`, `href`, `@odata.nextLink`): troceado, resolución, sitios de la cuenta | `http.rs`, `dav.rs` |
| `http` | una respuesta HTTP en bruto: cabeceras, longitud, `chunked` | `http.rs`, `oauth.rs` |
| `smtp` | las respuestas de un servidor SMTP, una tras otra | `transport.rs` |
| `backup` | una copia de seguridad o el cofre: sobre (sin cálculo de clave) y después contenido descifrado | `crypto.rs`, `backup.rs` |
| `oauth` | la vuelta de la página de inicio de sesión y el token de identidad | `oauth.rs` |
| `headers` | las cabeceras tomadas de un mensaje recibido para responderlo | `crates/mail-core` |
| `mbox` | un archivo importado, `.eml` o `.mbox`: troceado, reescrito en mbox, releído (lote E8a) | `crates/mail-core` (`mbox.rs`) |
| `smime_verify` | un mensaje firmado con S/MIME: sus dos mitades, luego la firma CMS y la cadena de certificados (lote G2) | `crates/smime` (`parts.rs`, `verify.rs`, `chain.rs`) |
| `smime_decrypt` | un mensaje cifrado con S/MIME: reconocido, devuelto a DER, descifrado con la clave de prueba (lote G2) | `crates/smime` (`ber.rs`, `decrypt.rs`) |
| `smime_cert` | un certificado recibido: resumen, firmante, cifrado posible hacia él (lote G2) | `crates/smime` (`cert.rs`, `encrypt.rs`) |

Los objetivos S/MIME llaman directamente al crate `vortex-smime`, que no tiene ni disco ni red. Los objetivos de la aplicación pasan por `vortexmail_lib::fuzz_api` (`src-tauri/src/lib.rs`), que llama a los lectores como lo hace la aplicación.

## Instalación (una vez)

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
```

La carpeta `dist/` debe existir (`npm run build`), como para cualquier compilación de la aplicación.

## Ejecutar

```bash
scripts/fuzz.sh 60              # prueba: 60 s por objetivo, todos en paralelo
scripts/fuzz.sh 28800           # una noche: 8 h por objetivo
scripts/fuzz.sh 600 ical vcard  # solo estos objetivos
make fuzz DUREE=28800           # lo mismo con make
```

Un solo objetivo, a mano:

```bash
cargo +nightly fuzz run -a ical fuzz/corpus/ical fuzz/seeds/ical
```

`-a` activa las comprobaciones de desbordamiento en los cálculos, desactivadas si no en modo release. Si la compilación con AddressSanitizer da problemas, `SANITIZER=none scripts/fuzz.sh …` (el código de Vortex Mail no tiene ningún bloque `unsafe`; es útil sobre todo para las bibliotecas en C).

Semillas: `fuzz/seeds/<objetivo>/`, y los correos trampa de `tests/corpus/attaques/` para `mime`. Lo que aprende el fuzzer va a `fuzz/corpus/<objetivo>/` (no versionado, se conserva de una noche a otra).

## Cuando un objetivo falla

La entrada se guarda en `fuzz/artifacts/<objetivo>/crash-…` (o `timeout-…`, `oom-…`).

1. Reproducir: `cargo +nightly fuzz run -a <objetivo> fuzz/artifacts/<objetivo>/crash-…`
2. Reducir (opcional): `cargo +nightly fuzz tmin -a <objetivo> fuzz/artifacts/<objetivo>/crash-…`
3. Corregir, con una prueba unitaria que reutilice la entrada.
4. Copiar la entrada en `tests/corpus/fuzz/<objetivo>/`: `scripts/fuzz.sh` la reproduce en cada pasada, y `src-tauri/tests/corpus.rs` la comprueba para `mime`.

Pánicos de `mail-parser`: desde la versión 1.0 (lote 9e), el objetivo `mime` se detiene ante cualquier pánico, también dentro del crate. `mimeparse.rs` lo atraparía en la aplicación, pero hay que comunicarlo a los autores del crate, con la entrada que lo provoca.

Fin del lote 8: 8 h sin fallos en cada objetivo. Lote 9a: cinco objetivos más (`http`, `smtp`, `backup`, `oauth`, `headers`).

El arnés `imap_session` está adaptado de MailCraft (`fuzz/fuzz_targets/imap.rs`, © MailCraft contributors, MIT OR Apache-2.0).
