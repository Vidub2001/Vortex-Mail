# Controles de calidad y seguridad

[Français](AUDIT.md) · [English](AUDIT.en.md) · **Español**

Este documento describe la red de seguridad creada en el lote 0 del plan del proyecto. Su función es detectar cualquier regresión durante los lotes siguientes.

## En resumen

```bash
make audit             # todos los controles
make audit-baseline    # registro de los avisos aceptados de cargo-deny
make sonar             # envío del análisis a SonarQube
make hooks             # Semgrep antes de cada commit
```

`make audit` pasa en verde con el código actual. Desde el lote 8, Semgrep y Clippy son **bloqueantes sin tolerancia**: la menor alerta hace fallar el comando, y cada excepción se escribe en el código con su motivo. Para las dependencias (cargo-deny, npm audit) y el corpus, los defectos ya conocidos siguen **aceptados temporalmente**: cada uno figura con el hallazgo que lleva y el lote que lo corregirá; un defecto **nuevo** hace fallar el comando.

## Instalación de las herramientas (una vez)

```bash
rustup component add clippy
cargo install --locked cargo-deny
pipx install semgrep
```

`make audit-tools` comprueba que estén todas presentes. También hace falta Python 3.11 o posterior.

## Primera ejecución

El registro de cargo-deny depende de la máquina de compilación. Hay que crearlo una vez y luego versionarlo. Desde el lote F4, un aviso nuevo no entra de oficio: `make audit-baseline` lo señala y falla; para aceptarlo deliberadamente, `make audit-baseline ACCEPTER_NOUVEAUX=1`, y luego escribir su motivo en `deny.toml`:

```bash
make audit-baseline
git add deny.toml
make audit
```

## Lo que comprueba `make audit`

| Control | Herramienta | Hallazgos aceptados |
| --- | --- | --- |
| Vulnerabilidades conocidas, crates abandonados, licencias, procedencia de las dependencias Rust | `cargo deny` | bloque «aceptados temporalmente» de `deny.toml` |
| Vulnerabilidades conocidas de las dependencias JavaScript (gravedad alta o crítica) | `npm audit` | sección `[[npm]]` de `security/accepted.toml` |
| Reglas propias surgidas de la revisión de código | `semgrep` | ninguno: bloqueante (excepciones justificadas en el código, véase más abajo) |
| Formato del código Rust (`rustfmt.toml`, lote 10e) | `cargo fmt --check` | ninguno: bloqueante; `cargo fmt` lo corrige |
| Calidad del código Rust (modo `pedantic` y reglas del proyecto, lote 10) | `cargo clippy` | ninguno: bloqueante (excepciones justificadas en el código, véase más abajo) |
| Pánicos que desenrollan la pila (red de `mimeparse.rs`, lote 10e) | `make panic-unwind` | ninguno: `panic = "abort"` rechazado en los `Cargo.toml`, `.cargo/config.toml` y `RUSTFLAGS` |
| Corpus de correos trampa y de escenarios de pérdida de datos | `corpus_check.py` y `cargo test --test corpus` | campo `etat` de `tests/corpus/manifest.toml` |

Los informes detallados se escriben en `target/audit/`.

## Reglas propias de Semgrep

Están en `.semgrep/rules/`, cada una con un archivo de prueba. `make semgrep-test` comprueba que detectan lo que deben detectar; `make semgrep` (y por tanto `make audit`) lo hace antes de analizar el código.

**Bloqueante desde el lote 8:** la menor coincidencia hace fallar `make audit`, sin lista de tolerancia. Si un caso es realmente inofensivo, la excepción se escribe en el código, en la línea señalada o en la anterior, con el nombre de la regla y su motivo:

```rust
// nosemgrep: vortex-extension-denylist -- lista de tipos reconocidos por su contenido, no una lista negra
```

Un `nosemgrep` sin nombre de regla (haría ignorar todas las reglas) o sin un motivo de al menos diez caracteres hace fallar el control. `make audit` muestra la lista de excepciones vigentes. Desde el lote F4, cualquier error de análisis de Semgrep (advertencias incluidas) y cualquier archivo fuente omitido por un motivo distinto de una exclusión deliberada también hacen fallar el control: un archivo leído en parte ya no está cubierto por las reglas. Las dos excepciones de la barra de herramientas de redacción afectan a funciones que solo aceptan HTML construido por la propia barra (tipo `ToolbarHtml`).

`make hooks` instala un gancho de git versionado (`scripts/hooks/pre-commit`): Semgrep se ejecuta antes de cada commit que toca código Rust o TypeScript. `git commit --no-verify` permite saltárselo puntualmente.

| Regla | Detecta | Revisión | Lote |
| --- | --- | --- | --- |
| `vortex-shell-command` | `Command::new("cmd")` y otros intérpretes de comandos, también por ruta (`/bin/sh`, `zsh`, `mshta`…) | S1 | 2, 8e |
| `vortex-extension-denylist` | lista negra de extensiones (`".exe"`…) | S1 | 2 |
| `vortex-auth-redirect` | cabeceras `Authorization` reenviadas tras una redirección | A1 | 5 |
| `vortex-notify-send-args` | remitente y asunto pasados a `notify-send` | S5 | 5 |
| `vortex-raw-html-sink` | `innerHTML =` y equivalentes (`srcDoc`, `createContextualFragment`, `insertHTML`…), fuera del módulo autorizado `src/safe-html.ts`; tres excepciones justificadas | S2, S3, S4 | 4, 8e |
| `vortex-html-reparse` | HTML saneado introducido en un `DOMParser` y analizado de nuevo | S3 | 4 |

## Clippy

`make clippy` (y por tanto `make audit`) ejecuta Clippy en todo el espacio de trabajo, pruebas incluidas, con `-D warnings`: la menor alerta bloquea. El registro de alertas toleradas (`security/clippy-baseline.json`) desapareció en el lote 8c, una vez tratadas las 501 alertas.

**Reglas del proyecto (lote 10).** El `Makefile` ya no las pasa por línea de comandos: están escritas de una vez por todas en `[workspace.lints]` del `Cargo.toml` raíz, a partir de la configuración de [BeautifulRust](https://github.com/morganlinton/BeautifulRust) (`lints/lints.toml`), y cada crate las hereda (`[lints] workspace = true`). Un simple `cargo clippy` las aplica, tanto en el editor como en `make audit`. Además del modo `pedantic`:

| Familia | Reglas | Por qué |
| --- | --- | --- |
| Pánicos | `unwrap_used`, `panic_in_result_fn`, `missing_panics_doc`, `missing_assert_message` | un pánico se anuncia (`expect("motivo")`, sección `# Panics`) |
| Errores | `missing_errors_doc` | toda función pública que devuelve un `Result` dice cuándo falla |
| Excepciones | `allow_attributes`, `allow_attributes_without_reason` | solo `#[expect(…, reason)]`, nunca `#[allow]` |
| `unsafe` | `undocumented_unsafe_blocks`, `multiple_unsafe_ops_per_block`, `unsafe_op_in_unsafe_fn` | salvaguardas; Vortex Mail no tiene ningún bloque `unsafe` |
| Forma de las API | `unreachable_pub`, `missing_debug_implementations`, `missing_docs`, `fn_params_excessive_bools`, `struct_excessive_bools` | `pub(crate)` por defecto; todo tipo público tiene `Debug` y documentación |
| Punteros compartidos | `clone_on_ref_ptr`, `redundant_clone`, `rc_buffer` | `Arc::clone(&x)` muestra que se comparte, no que se copia |
| Conversiones | `as_conversions`, `cast_possible_truncation`, `cast_sign_loss` | `From`, `try_from`; nada de `as` silencioso |
| Restos de depuración | `dbg_macro`, `todo`, `print_stdout`, `print_stderr` | el registro pasa por `src-tauri/src/diag.rs` (véase más abajo) |
| Modismos | `string_add`, `wildcard_imports`, `rust_2018_idioms` | |

Quedan descartadas, como en BeautifulRust: `module_name_repetitions`, `must_use_candidate`, `module_inception`, `enum_glob_use`; y `exhaustive_enums`, pensada para bibliotecas publicadas. `clippy.toml` admite `unwrap`, `expect`, `dbg!` y las salidas por pantalla en las pruebas (`#[test]` y módulos `#[cfg(test)]`). El falso servidor IMAP de las pruebas de integración (`crates/imap-lite/tests/fake_server.rs`), fuera de esos casos, lleva un `#![expect(clippy::unwrap_used, …)]` al principio del archivo: una petición que no sabe leer debe hacer fallar la prueba. `rust-version = "1.100"` se declara en cada crate (`Option::map_or_default`), y `rust-toolchain.toml` fija la versión usada (1.100.0) en Linux, en Windows y en la CI: una versión nueva de Rust puede añadir lints, por eso la actualización se hace en un lote aparte.

**Excepciones.** Si una alerta es deliberada (un cálculo de la especificación `SHA-256` con sus nombres de una letra, un comando Tauri que recibe necesariamente sus argumentos por valor…), la excepción se escribe junto al código, con su motivo:

```rust
#[expect(clippy::many_single_char_names, reason = "nombres de la especificación (FIPS 180-4)")]
```

`expect` en lugar de `allow` (rechazado por `clippy::allow_attributes`): el día en que la alerta deja de producirse, `expect` lo señala y la excepción inútil desaparece. Una excepción sin motivo también se rechaza (`clippy::allow_attributes_without_reason`). Para encontrarlas todas: `git grep -n "expect(clippy"`.

**Dos desviaciones deliberadas de las reglas de BeautifulRust**, justificadas en el código y en el [modelo de amenazas](MODELE-MENACE.es.md):

- **Cerrojos envenenados.** La regla pide detener la aplicación cuando un cerrojo está «envenenado» (se produjo un pánico mientras estaba tomado). Vortex Mail recupera el cerrojo (`guard` en `router.rs`, `lock` en `auth.rs`), porque los datos protegidos se sustituyen de una sola vez; las conexiones IMAP, en cambio, se cierran y se vuelven a abrir. Como contrapartida: bajo esos cerrojos, ninguna modificación en varios pasos fuera de una transacción.
- **`catch_unwind`.** Una sola vez, alrededor del crate `mail-parser` (`guarded` en `mimeparse.rs`): un mensaje que lo hace entrar en pánico se muestra como ilegible, en lugar de detener el hilo que lo leía. Esta red solo funciona si los pánicos desenrollan la pila: `make panic-unwind` rechaza `panic = "abort"`.

**Registro.** Como `print_stderr` está rechazado, todo pasa por `diag::report` (`src-tauri/src/diag.rs`), que sustituye los caracteres de control y de maquetación (secuencias de escape, saltos de línea, inversión de la visualización) y corta los mensajes a 2000 caracteres: los mensajes de error de los servidores no pueden fabricar líneas de registro falsas ni controlar el terminal.

## Corpus de no regresión

La carpeta `tests/corpus/` contiene dos tipos de entradas:

- **`attaques/`**: un correo `.eml` por cada ataque de la revisión (adjuntos trampa, rastreadores en correos citados, scripts, enlaces, invitaciones malformadas…). Las cargas son inertes: las direcciones objetivo están en `.invalid`, y el script de prueba se limita a colocar un marcador.
- **`manifest.toml`**: para cada correo, el comportamiento esperado una vez corregido el defecto. El archivo describe también, paso a paso, cada **escenario de pérdida de datos** (borrador y mensaje con el mismo número, movimiento interrumpido…).

Una entrada pasa de `accepte-temporairement` a `corrige` cuando una prueba automática verifica su comportamiento esperado. Desde el lote 9b, nombra esa prueba, y `corpus_check.py` (y por tanto `make audit`) rechaza una entrada `corrige` sin prueba, o cuya prueba no existe. Desde el lote F4, una prueba comentada, marcada con `#[ignore]`, o `test.skip` / `test.todo` en la interfaz, no cuenta:

```toml
tests = [
  "src-tauri/src/secrets.rs::damaged_vault_falls_back_to_its_copy",      # prueba Rust: #[test] fn …
  "tests/frontend/calendar.test.mjs::F10 : une journée entière le jour…", # prueba de la interfaz: test("…")
]
```

## Cuando un lote corrige un defecto

En el mismo commit que la corrección:

1. bajar el contador, o retirar la entrada, en `security/accepted.toml` (`make audit` señala lo que ha desaparecido);
2. pasar la entrada del corpus a `corrige`, nombrando su prueba (`tests = ["ruta::funcion"]`);
3. volver a ejecutar `make audit-baseline` si han desaparecido avisos de cargo-deny.

Desde el lote 8, Semgrep (lote 8b) y Clippy (lote 8c) son bloqueantes sin ninguna tolerancia.

## SonarQube

SonarQube Community Build se ejecuta en un contenedor LXC del clúster Proxmox del autor. Sirve de panel de la deuda técnica; no es una salvaguarda.

- **Frontend TypeScript / React**: analizado de forma nativa.
- **Rust**: la Community Build no lo analiza. Las alertas de Clippy se importan en el formato genérico (`sonar.externalIssuesReportPaths`).

La dirección y el token **nunca** se escriben en el repositorio. Vienen del entorno:

```bash
export SONAR_HOST_URL=http://sonarqube.example.lan:9000
export SONAR_TOKEN=…        # SonarQube › Mi cuenta › Seguridad › Generar token
make sonar
```

`make sonar` usa `sonar-scanner` si está instalado. Si no, pasa por la imagen Docker `sonarsource/sonar-scanner-cli`. El proyecto se llama `vortexmail`.
