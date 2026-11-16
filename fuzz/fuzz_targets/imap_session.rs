//! Un serveur IMAP qui répond n'importe quels octets, sur toute une session :
//! connexion, dossiers, synchronisation, déplacement, envoi, IDLE.
//!
//! Adapté de MailCraft (`fuzz/fuzz_targets/imap.rs`, © MailCraft
//! contributors, MIT OR Apache-2.0).
#![no_main]

use std::io::{Cursor, Read, Write};
use std::time::Duration;

/// Lit les octets du « serveur » ; ce que le client écrit ne va nulle part.
struct Server(Cursor<Vec<u8>>);

impl Read for Server {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Server {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(mut imap) = imap_lite::Client::connect(Server(Cursor::new(data.to_vec()))) else { return };
    let _ = imap.enable_sync_extensions();
    let _ = imap.login("ada", "secret");
    let _ = imap.list();
    let _ = imap.status_many(&["INBOX".to_string(), "Sent".to_string()]);
    let _ = imap.select("INBOX");
    let _ = imap.uid_fetch_set("1:*", "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER])");
    let _ = imap.uid_fetch_changed("1:*", "(UID FLAGS)", 5);
    let _ = imap.uid_fetch(&[1], "(UID BODY.PEEK[])");
    let _ = imap.uid_search("TEXT", "café");
    let _ = imap.uid_store(&[1, 2], "+FLAGS.SILENT (\\Seen)");
    let _ = imap.uid_move(&[1, 2, 3], "Archive");
    let _ = imap.uid_expunge(&[7]);
    let _ = imap.append("Sent", &["\\Seen".to_string()], Some(0), b"Subject: x\r\n\r\n");
    let _ = imap.idle(Duration::from_millis(1), || false);
    let _ = imap.logout();
});
