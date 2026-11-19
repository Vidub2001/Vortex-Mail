# Quality and security checks

[Français](AUDIT.md) · **English** · [Español](AUDIT.es.md)

This document describes the safety net set up in batch 0 of the project plan. Its job is to catch any regression during the following batches.

## In short

```bash
make audit             # every check
make audit-baseline    # snapshot of the accepted cargo-deny advisories
make sonar             # send the analysis to SonarQube
make hooks             # Semgrep before every commit
```

`make audit` passes on the current code. Since batch 8, Semgrep and Clippy are **blocking with no tolerance**: a single warning makes the command fail, and an exception is written in the code with its reason. For dependencies (cargo-deny, npm audit) and the corpus, defects already known remain **temporarily accepted**: each one is listed with the finding it carries and the batch that will fix it; a **new** defect makes the command fail.

## Installing the tools (once)

```bash
rustup component add clippy
cargo install --locked cargo-deny
pipx install semgrep
```

`make audit-tools` checks that they are all present. Python 3.11 or later is also required.

## First run

The cargo-deny snapshot depends on the build machine. Create it once, then commit it. Since batch F4, a new advisory is not added automatically: `make audit-baseline` reports it and fails; to accept it on purpose, run `make audit-baseline ACCEPTER_NOUVEAUX=1`, then write its reason in `deny.toml`:

```bash
make audit-baseline
git add deny.toml
make audit
```

## What `make audit` checks

| Check | Tool | Accepted findings |
| --- | --- | --- |
| Known vulnerabilities, abandoned crates, licences, origin of Rust dependencies | `cargo deny` | "temporarily accepted" block of `deny.toml` |
| Known vulnerabilities of JavaScript dependencies (high or critical) | `npm audit` | `[[npm]]` section of `security/accepted.toml` |
| Project rules drawn from the code review | `semgrep` | none: blocking (exceptions justified in the code, see below) |
| Rust code formatting (`rustfmt.toml`, batch 10e) | `cargo fmt --check` | none: blocking; `cargo fmt` fixes it |
| Rust code quality (`pedantic` mode and project rules, batch 10) | `cargo clippy` | none: blocking (exceptions justified in the code, see below) |
| Unwinding panics (safety net of `mimeparse.rs`, batch 10e) | `make panic-unwind` | none: `panic = "abort"` refused in `Cargo.toml` files, `.cargo/config.toml` and `RUSTFLAGS` |
| Pinned fuzzing dependencies (`fuzz/Cargo.lock` up to date, batch J4) | `make fuzz-lock` | none: `cargo metadata --locked` in `fuzz/`; `cargo metadata` in `fuzz/` completes it, updating nothing else |
| Corpus of booby-trapped mails and data-loss scenarios | `corpus_check.py` and `cargo test --test corpus` | `etat` field of `tests/corpus/manifest.toml` |

Detailed reports are written to `target/audit/`.

## Project Semgrep rules

They live in `.semgrep/rules/`, each with a test file. `make semgrep-test` checks that they catch what they should; `make semgrep` (and therefore `make audit`) runs it before analysing the code.

**Blocking since batch 8:** a single match makes `make audit` fail, with no tolerance list. If a case is truly harmless, the exception is written in the code, on the reported line or the one before, with the rule name and its reason:

```rust
// nosemgrep: vortex-extension-denylist -- list of types recognised by their content, not a denylist
```

A `nosemgrep` without a rule name (it would silence every rule) or without a reason of at least ten characters makes the check fail. `make audit` prints the list of exceptions in place. Since batch F4, any Semgrep analysis error (warnings included) and any source file skipped for a reason other than an intended exclusion also make the check fail: a partially read file is no longer covered by the rules. The two exceptions in the compose toolbar concern functions that only accept HTML built by the toolbar itself (`ToolbarHtml` type).

`make hooks` installs a versioned git hook (`scripts/hooks/pre-commit`): Semgrep runs before every commit that touches Rust or TypeScript code. `git commit --no-verify` skips it for a single commit.

| Rule | Catches | Review | Batch |
| --- | --- | --- | --- |
| `vortex-shell-command` | `Command::new("cmd")` and other command interpreters, including by path (`/bin/sh`, `zsh`, `mshta`…) | S1 | 2, 8e |
| `vortex-extension-denylist` | extension denylists (`".exe"`…) | S1 | 2 |
| `vortex-auth-redirect` | `Authorization` headers sent again after a redirect | A1 | 5 |
| `vortex-notify-send-args` | sender and subject passed to `notify-send` | S5 | 5 |
| `vortex-raw-html-sink` | `innerHTML =` and equivalents (`srcDoc`, `createContextualFragment`, `insertHTML`…), outside the allowed module `src/safe-html.ts`; three justified exceptions | S2, S3, S4 | 4, 8e |
| `vortex-html-reparse` | sanitised HTML put back into a `DOMParser` and parsed again | S3 | 4 |

## Clippy

`make clippy` (and therefore `make audit`) runs Clippy on the whole workspace, tests included, with `-D warnings`: a single warning blocks. The list of tolerated warnings (`security/clippy-baseline.json`) disappeared in batch 8c, once all 501 warnings had been dealt with.

**Project rules (batch 10).** They are no longer passed on the command line by the `Makefile`: they are written once in `[workspace.lints]` of the root `Cargo.toml`, based on the [BeautifulRust](https://github.com/morganlinton/BeautifulRust) configuration (`lints/lints.toml`), and every crate inherits them (`[lints] workspace = true`). A plain `cargo clippy` therefore applies them, in the editor as in `make audit`. Beyond `pedantic` mode:

| Family | Rules | Why |
| --- | --- | --- |
| Panics | `unwrap_used`, `panic_in_result_fn`, `missing_panics_doc`, `missing_assert_message` | a panic is announced (`expect("reason")`, `# Panics` section) |
| Errors | `missing_errors_doc` | every public function returning a `Result` says when it fails |
| Exceptions | `allow_attributes`, `allow_attributes_without_reason` | `#[expect(…, reason)]` only, never `#[allow]` |
| `unsafe` | `undocumented_unsafe_blocks`, `multiple_unsafe_ops_per_block`, `unsafe_op_in_unsafe_fn` | guard rails; Vortex Mail has no `unsafe` block |
| API shape | `unreachable_pub`, `missing_debug_implementations`, `missing_docs`, `fn_params_excessive_bools`, `struct_excessive_bools` | `pub(crate)` by default; every public type has `Debug` and documentation |
| Shared pointers | `clone_on_ref_ptr`, `redundant_clone`, `rc_buffer` | `Arc::clone(&x)` shows sharing, not copying |
| Conversions | `as_conversions`, `cast_possible_truncation`, `cast_sign_loss` | `From`, `try_from`; no silent `as` |
| Debugging leftovers | `dbg_macro`, `todo`, `print_stdout`, `print_stderr` | logging goes through `src-tauri/src/diag.rs` (see below) |
| Idioms | `string_add`, `wildcard_imports`, `rust_2018_idioms` | |

Left out, as in BeautifulRust: `module_name_repetitions`, `must_use_candidate`, `module_inception`, `enum_glob_use`; and `exhaustive_enums`, which targets published libraries. `clippy.toml` allows `unwrap`, `expect`, `dbg!` and printing in tests (`#[test]` and `#[cfg(test)]` modules). The fake IMAP server of the integration tests (`crates/imap-lite/tests/fake_server.rs`), outside these cases, carries a file-level `#![expect(clippy::unwrap_used, …)]`: a request it cannot read must make the test fail. `rust-version = "1.100"` is declared in every crate (`Option::map_or_default`), and `rust-toolchain.toml` pins the version used (1.100.0) on Linux, on Windows and in CI: a new Rust release can add lints, so upgrading is done in a separate batch.

**Exceptions.** When a warning is intended (a computation from the `SHA-256` specification with its one-letter names, a Tauri command that necessarily receives its arguments by value…), the exception is written next to the code, with its reason:

```rust
#[expect(clippy::many_single_char_names, reason = "names from the specification (FIPS 180-4)")]
```

`expect` rather than `allow` (refused by `clippy::allow_attributes`): the day the warning no longer fires, `expect` reports it, and the obsolete exception is removed. An exception without a reason is itself refused (`clippy::allow_attributes_without_reason`). To list them all: `git grep -n "expect(clippy"`.

**Two intended departures from the BeautifulRust rules**, justified in the code and in the [threat model](MODELE-MENACE.en.md):

- **Poisoned locks.** The rule asks to stop the application when a lock is "poisoned" (a panic happened while it was held). Vortex Mail takes the lock back (`guard` in `router.rs`, `lock` in `auth.rs`), because the protected data is replaced in one piece; IMAP connections, for their part, are closed and reopened. In return: under these locks, no multi-step change outside a transaction.
- **`catch_unwind`.** Used once, around the `mail-parser` crate (`guarded` in `mimeparse.rs`): a message that makes it panic is shown as unreadable, instead of stopping the thread that was reading it. This safety net only works if panics unwind the stack: `make panic-unwind` refuses `panic = "abort"`.

**Logging.** Since `print_stderr` is refused, everything goes through `diag::report` (`src-tauri/src/diag.rs`), which replaces control and layout characters (escape sequences, line breaks, display reversal) and cuts messages at 2,000 characters: error messages from servers can neither forge fake log lines nor drive the terminal.

## Regression corpus

The `tests/corpus/` folder contains two kinds of entries:

- **`attaques/`**: one `.eml` mail per attack from the code review (booby-trapped attachments, trackers in quoted mails, scripts, links, malformed invitations…). The payloads are inert: the targeted addresses are under `.invalid`, and the test script only sets a marker.
- **`manifest.toml`**: for each mail, the expected behaviour once the defect is fixed. The file also describes, step by step, each **data-loss scenario** (draft and message with the same number, interrupted move…).

An entry moves from `accepte-temporairement` to `corrige` when an automated test checks its expected behaviour. Since batch 9b, it names that test, and `corpus_check.py` (so `make audit`) refuses a `corrige` entry without a test, or whose test does not exist. Since batch F4, a commented-out test, one marked `#[ignore]`, or `test.skip` / `test.todo` on the interface side, does not count:

```toml
tests = [
  "src-tauri/src/secrets.rs::damaged_vault_falls_back_to_its_copy",      # Rust test: #[test] fn …
  "tests/frontend/calendar.test.mjs::F10 : une journée entière le jour…", # interface test: test("…")
]
```

## When a batch fixes a defect

In the same commit as the fix:

1. lower the counter, or remove the entry, in `security/accepted.toml` (`make audit` reports what disappeared);
2. move the corpus entry to `corrige`, naming its test (`tests = ["path::function"]`);
3. run `make audit-baseline` again if cargo-deny advisories disappeared.

Since batch 8, Semgrep (batch 8b) and Clippy (batch 8c) are blocking with no tolerance at all.

## SonarQube

SonarQube Community Build runs in an LXC container on the author's Proxmox cluster. It serves as a technical-debt dashboard; it is not a safeguard.

- **TypeScript / React frontend**: analysed natively.
- **Rust**: the Community Build does not analyse it. Clippy warnings are imported in the generic format (`sonar.externalIssuesReportPaths`).

The address and the token are **never** written in the repository. They come from the environment:

```bash
export SONAR_HOST_URL=http://sonarqube.example.lan:9000
export SONAR_TOKEN=…        # SonarQube › My Account › Security › Generate Token
make sonar
```

`make sonar` uses `sonar-scanner` if it is installed. Otherwise, it goes through the `sonarsource/sonar-scanner-cli` Docker image. The project is called `vortexmail`.

Since batch J8c, `make sonar` also sends the coverage of the interface tests (`make front-coverage`, lcov), and Jenkins runs it on `main` after each merge, without ever blocking the build (docs/CI.md, § 7). The sources also include `crates/smime`, and the tests `tests/frontend` and `tests/ui`.
