# Fuzzing Vortex Mail

[Français](README.md) · **English** · [Español](README.es.md)

`cargo-fuzz` targets for everything that comes from outside (batch 8 of the project plan). The goal: no input, even one crafted on purpose, may make a thread panic, overflow the stack or loop forever.

| Target | What is read | Code involved |
| --- | --- | --- |
| `imap_session` | an IMAP server answering anything, over a whole session | `crates/imap-lite` |
| `imap_response` | an IMAP response (line and literals) | `imap-lite/src/response.rs` |
| `imap_date` | an `INTERNALDATE` date | `imap-lite/src/date.rs` |
| `mime` | a received message, as when displayed, and its first attachment | `mimeparse.rs` (`mail-parser` crate), `attachments.rs`, `sanitize.rs` |
| `html` | an HTML body | `sanitize.rs`, `images.rs` |
| `ical` | an invitation or a CalDAV calendar, its rewrite, its recurrences for Outlook.com | `pimfmt.rs`, `o365.rs` |
| `vcard` | a contact card, its rewrite | `pimfmt.rs` |
| `json` | a response from the Outlook.com API: events and contacts converted, contact rewritten then read again, reason of an error | `json.rs`, `o365.rs` |
| `dav_xml` | a response from a CalDAV or CardDAV server: elements, resolved and checked addresses, reason of an error | `xml.rs`, `dav.rs` |
| `url` | an address received from a server or a message (`Location`, `href`, `@odata.nextLink`): splitting, resolution, account sites | `http.rs`, `dav.rs` |
| `http` | a raw HTTP response: headers, length, `chunked` | `http.rs`, `oauth.rs` |
| `smtp` | the responses of an SMTP server, one after the other | `transport.rs` |
| `backup` | a backup or the vault: envelope (without key derivation), then decrypted content | `crypto.rs`, `backup.rs` |
| `oauth` | the return of the sign-in page and the identity token | `oauth.rs` |
| `headers` | the headers taken from a received message to reply to it | `crates/mail-core` |
| `mbox` | an imported file, `.eml` or `.mbox`: split, rewritten as mbox, read again (batch E8a) | `crates/mail-core` (`mbox.rs`) |
| `smime_verify` | an S/MIME signed message: its two halves, then the CMS signature and the certificate chain (batch G2) | `crates/smime` (`parts.rs`, `verify.rs`, `chain.rs`) |
| `smime_decrypt` | an S/MIME encrypted message: recognised, turned back into DER, decrypted with the test key (batch G2) | `crates/smime` (`ber.rs`, `decrypt.rs`) |
| `smime_cert` | a received certificate: summary, signer, possible encryption to it (batch G2) | `crates/smime` (`cert.rs`, `encrypt.rs`) |

The S/MIME targets call the `vortex-smime` crate directly, which has neither disk nor network. The application targets go through `vortexmail_lib::fuzz_api` (`src-tauri/src/lib.rs`), which calls the parsers the same way the application does.

## Installation (once)

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
```

The `dist/` folder must exist (`npm run build`), as for any build of the application.

## Running

```bash
scripts/fuzz.sh 60              # trial: 60 s per target, all in parallel
scripts/fuzz.sh 28800           # one night: 8 h per target
scripts/fuzz.sh 600 ical vcard  # only these targets
make fuzz DUREE=28800           # the same through make
```

A single target, by hand:

```bash
cargo +nightly fuzz run -a ical fuzz/corpus/ical fuzz/seeds/ical
```

`-a` turns on arithmetic overflow checks, otherwise off in release mode. If building with AddressSanitizer causes trouble, use `SANITIZER=none scripts/fuzz.sh …` (Vortex Mail's code has no `unsafe` block; the sanitizer is mostly useful for C libraries).

Seeds: `fuzz/seeds/<target>/`, plus the booby-trapped mails of `tests/corpus/attaques/` for `mime`. What the fuzzer learns goes to `fuzz/corpus/<target>/` (not versioned, kept from one night to the next).

## When a target crashes

The input is saved in `fuzz/artifacts/<target>/crash-…` (or `timeout-…`, `oom-…`).

1. Replay: `cargo +nightly fuzz run -a <target> fuzz/artifacts/<target>/crash-…`
2. Minimise (optional): `cargo +nightly fuzz tmin -a <target> fuzz/artifacts/<target>/crash-…`
3. Fix, with a unit test that reuses the input.
4. Copy the input to `tests/corpus/fuzz/<target>/`: `scripts/fuzz.sh` replays it on every run, and `src-tauri/tests/corpus.rs` checks it for `mime`.

`mail-parser` panics: since version 1.0 (batch 9e), the `mime` target stops on any panic, inside the crate too. `mimeparse.rs` would catch it in the application, but it must be reported to the crate's authors, with the input that caused it.

End of batch 8: 8 h without a crash on every target. Batch 9a: five more targets (`http`, `smtp`, `backup`, `oauth`, `headers`).

The `imap_session` harness is adapted from MailCraft (`fuzz/fuzz_targets/imap.rs`, © MailCraft contributors, MIT OR Apache-2.0).
