//! Tests de bout en bout contre un petit serveur IMAP simulé (TCP local).

// Le faux serveur ne vit que dans ce fichier de test : une requête qu'il ne
// sait pas lire doit faire échouer le test, donc paniquer est voulu.
#![expect(clippy::unwrap_used, reason = "faux serveur de test : une panique fait échouer le test, comme voulu")]

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use imap_lite::{base64_encode, parse_response, Client, Error, Response, Token};

const USER: &str = "moi@exemple.fr";
const PASS: &str = "mot \"de\" passé"; // guillemets + accent : force un littéral

#[derive(Clone)]
struct Msg {
    uid: u32,
    flags: Vec<String>,
    body: Vec<u8>,
    /// Numéro de modification (CONDSTORE).
    modseq: u64,
}

struct Mailbox {
    name: &'static str,
    attrs: &'static str,
    next_uid: u32,
    messages: Vec<Msg>,
    /// HIGHESTMODSEQ du dossier.
    modseq: u64,
    /// UID supprimés, avec le MODSEQ de leur suppression (QRESYNC).
    expunged: Vec<(u32, u64)>,
}

impl Mailbox {
    fn new(name: &'static str, attrs: &'static str, next_uid: u32, messages: Vec<Msg>) -> Self {
        let modseq = messages.iter().map(|m| m.modseq).max().unwrap_or(1);
        Mailbox { name, attrs, next_uid, messages, modseq, expunged: Vec::new() }
    }

    /// Nouveau numéro de modification.
    fn bump(&mut self) -> u64 {
        self.modseq += 1;
        self.modseq
    }

    fn expunge_where(&mut self, gone: impl Fn(&Msg) -> bool) {
        let uids: Vec<u32> = self.messages.iter().filter(|m| gone(m)).map(|m| m.uid).collect();
        self.messages.retain(|m| !gone(m));
        for uid in uids {
            let modseq = self.bump();
            self.expunged.push((uid, modseq));
        }
    }
}

fn mail(subject: &str) -> Vec<u8> {
    format!("From: Test <t@example.com>\r\nSubject: {subject}\r\n\r\nCorps de {subject}\r\n").into_bytes()
}

struct Server {
    boxes: Vec<Mailbox>,
    selected: Option<usize>,
    caps: &'static str,
    /// `ENABLE QRESYNC` reçu.
    qresync: bool,
}

impl Server {
    fn new(caps: &'static str) -> Self {
        Server {
            boxes: vec![
                Mailbox::new(
                    "INBOX",
                    "\\HasNoChildren",
                    3,
                    vec![
                        Msg { uid: 1, flags: vec!["\\Seen".into()], body: mail("Premier"), modseq: 4 },
                        Msg { uid: 2, flags: vec![], body: mail("Second"), modseq: 5 },
                    ],
                ),
                Mailbox::new("Envoy&AOk-s", "\\HasNoChildren \\Sent", 1, vec![]),
                Mailbox::new("Archives", "\\HasNoChildren", 10, vec![]),
            ],
            selected: None,
            caps,
            qresync: false,
        }
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.boxes.iter().position(|b| b.name == name)
    }

    fn matches(set: &str, n: u32) -> bool {
        set.split(',').any(|part| match part.split_once(':') {
            Some((a, b)) => {
                let a: u32 = a.parse().unwrap();
                let b = if b == "*" { u32::MAX } else { b.parse().unwrap() };
                (a.min(b)..=a.max(b)).contains(&n)
            }
            None => part.parse::<u32>().unwrap() == n,
        })
    }

    /// Lit une commande complète, littéraux compris (et répond `+` à chacun).
    fn read_command(reader: &mut BufReader<TcpStream>, out: &mut TcpStream) -> Option<Vec<u8>> {
        let mut cmd = Vec::new();
        loop {
            let mut line = Vec::new();
            if reader.read_until(b'\n', &mut line).ok()? == 0 {
                return None;
            }
            while line.last().is_some_and(|c| *c == b'\n' || *c == b'\r') {
                line.pop();
            }
            cmd.extend_from_slice(&line);
            let text = String::from_utf8_lossy(&line).to_string();
            if let (true, Some(open)) = (text.ends_with('}'), text.rfind('{')) {
                let n: usize = text[open + 1..text.len() - 1].parse().unwrap();
                out.write_all(b"+ go ahead\r\n").unwrap();
                let mut lit = vec![0; n];
                reader.read_exact(&mut lit).unwrap();
                cmd.extend_from_slice(b"\r\n");
                cmd.extend_from_slice(&lit);
                continue;
            }
            return Some(cmd);
        }
    }

    fn args(raw: &[u8]) -> Vec<Token> {
        let mut line = b"* ".to_vec();
        line.extend_from_slice(raw);
        match parse_response(&line).unwrap() {
            Response::Data(t) => t,
            other => panic!("{other:?}"),
        }
    }

    #[expect(clippy::too_many_lines, reason = "faux serveur de test : une branche par commande IMAP, plus lisible d'un seul tenant")]
    fn serve(mut self, stream: TcpStream) {
        let mut out = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        write!(out, "* OK [CAPABILITY IMAP4rev1 {}] Faux serveur prêt\r\n", self.caps).unwrap();

        while let Some(cmd) = Self::read_command(&mut reader, &mut out) {
            let text = String::from_utf8_lossy(&cmd).to_string();
            let (tag, rest) = text.split_once(' ').unwrap();
            let tokens = Self::args(&cmd[tag.len() + 1..]);
            let verb = tokens[0].text().unwrap().to_ascii_uppercase();
            let sub = tokens.get(1).and_then(Token::text).unwrap_or_default().to_ascii_uppercase();
            let mut reply = String::new();
            let mut tail: Vec<u8> = Vec::new(); // octets bruts (littéraux FETCH)
            let mut status = "OK done".to_string();

            match (verb.as_str(), sub.as_str()) {
                ("CAPABILITY", _) => {
                    let _ = write!(reply, "* CAPABILITY IMAP4rev1 {}\r\n", self.caps);
                }
                ("LOGIN", _) => {
                    let (u, p) = (tokens[1].text().unwrap(), tokens[2].text().unwrap());
                    status = if u == USER && p == PASS {
                        // `NOUIDPLUS` : serveur ancien, sans MOVE ni UIDPLUS.
                        if self.caps.contains("NOUIDPLUS") {
                            format!("OK [CAPABILITY IMAP4rev1 {}] Connecté", self.caps)
                        } else {
                            format!("OK [CAPABILITY IMAP4rev1 MOVE UIDPLUS {}] Connecté", self.caps)
                        }
                    } else {
                        "NO [AUTHENTICATIONFAILED] Identifiants invalides".into()
                    };
                }
                ("AUTHENTICATE", "XOAUTH2") => {
                    let ok = base64_encode(format!("user={USER}\x01auth=Bearer bon-jeton\x01\x01").as_bytes());
                    let given = tokens.get(2).and_then(Token::text).unwrap_or_default();
                    if given == ok {
                        status = "OK Authentifié".into();
                    } else {
                        // Détail de l'erreur en continuation, comme certains grands fournisseurs.
                        out.write_all(b"+ eyJzdGF0dXMiOiI0MDEifQ==\r\n").unwrap();
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        assert_eq!(line, "\r\n", "réponse vide attendue après l'erreur XOAUTH2");
                        status = "NO AUTHENTICATE failed.".into();
                    }
                }
                ("AUTHENTICATE", "PLAIN") => {
                    out.write_all(b"+ \r\n").unwrap();
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let expected = base64_encode(format!("\0{USER}\0{PASS}").as_bytes());
                    status = if line.trim_end() == expected { "OK Authentifié".into() } else { "NO raté".into() };
                }
                ("LIST", _) => {
                    for b in &self.boxes {
                        let _ = write!(reply, "* LIST ({}) \"/\" \"{}\"\r\n", b.attrs, b.name);
                    }
                }
                // Dossiers, drapeaux et NOOP : acceptés sans effet sur le faux serveur.
                ("CREATE" | "DELETE" | "SUBSCRIBE" | "UNSUBSCRIBE" | "STORE" | "NOOP", _) => {}
                // Sans UIDPLUS : purge tous les messages marqués du dossier.
                ("EXPUNGE", _) => {
                    let b = &mut self.boxes[self.selected.unwrap()];
                    b.expunge_where(|m| m.flags.iter().any(|f| f == "\\Deleted"));
                }
                ("UID", "SEARCH") => {
                    let criteria = tokens[2].text().unwrap().to_ascii_uppercase();
                    let b = &self.boxes[self.selected.unwrap()];
                    let found: Vec<String> = b
                        .messages
                        .iter()
                        .filter(|m| criteria != "DELETED" || m.flags.iter().any(|f| f == "\\Deleted"))
                        .map(|m| m.uid.to_string())
                        .collect();
                    reply += &format!("* SEARCH {}\r\n", found.join(" ")).replace("SEARCH \r", "SEARCH\r");
                }
                // Abonnement : seulement la boîte de réception.
                ("LSUB", _) => reply += "* LSUB () \"/\" INBOX\r\n",
                ("STATUS", _) => {
                    let b = &self.boxes[self.find(&tokens[1].text().unwrap()).unwrap()];
                    let unseen = b.messages.iter().filter(|m| !m.flags.iter().any(|f| f == "\\Seen")).count();
                    let _ = write!(
                        reply,
                        "* STATUS \"{}\" (MESSAGES {} UNSEEN {unseen} UIDNEXT {} UIDVALIDITY 7)\r\n",
                        b.name,
                        b.messages.len(),
                        b.next_uid
                    );
                }
                ("SELECT", _) => match self.find(&tokens[1].text().unwrap()) {
                    Some(i) => {
                        self.selected = Some(i);
                        let b = &self.boxes[i];
                        let _ = write!(
                            reply,
                            "* {} EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n* OK [UIDNEXT {}] ok\r\n",
                            b.messages.len(),
                            b.next_uid
                        );
                        if self.caps.contains("CONDSTORE") {
                            let _ = write!(reply, "* OK [HIGHESTMODSEQ {}] ok\r\n", b.modseq);
                        }
                        status = "OK [READ-WRITE] SELECT terminé".into();
                    }
                    None => status = "NO dossier inconnu".into(),
                },
                ("FETCH", _) | ("UID", "FETCH") => {
                    let by_uid = verb == "UID";
                    let set = tokens[if by_uid { 2 } else { 1 }].text().unwrap();
                    let headers_only = rest.contains("HEADER.FIELDS");
                    // Modificateurs RFC 7162 : `(CHANGEDSINCE n [VANISHED])` après la requête.
                    let modifiers: Vec<String> = tokens
                        .last()
                        .and_then(Token::list)
                        .filter(|l| l.first().is_some_and(|t| t.is_atom("CHANGEDSINCE")))
                        .map_or_default(|l| l.iter().filter_map(Token::text).collect());
                    let since: Option<u64> = modifiers.get(1).map(|n| n.parse().unwrap());
                    let b = &self.boxes[self.selected.unwrap()];
                    if modifiers.iter().any(|m| m == "VANISHED") {
                        assert!(self.qresync, "VANISHED sans ENABLE QRESYNC");
                        let gone: Vec<String> = b
                            .expunged
                            .iter()
                            .filter(|(uid, modseq)| *modseq > since.unwrap() && Self::matches(&set, *uid))
                            .map(|(uid, _)| uid.to_string())
                            .collect();
                        if !gone.is_empty() {
                            let _ = write!(reply, "* VANISHED (EARLIER) {}\r\n", gone.join(","));
                        }
                    }
                    for (i, m) in b.messages.iter().enumerate() {
                        let seq = u32::try_from(i).unwrap() + 1;
                        if !Self::matches(&set, if by_uid { m.uid } else { seq }) {
                            continue;
                        }
                        if since.is_some_and(|n| m.modseq <= n) {
                            continue;
                        }
                        if !rest.contains("BODY") {
                            // États seulement (`(UID FLAGS)`), MODSEQ si CONDSTORE.
                            let modseq = if since.is_some() || self.qresync { format!(" MODSEQ ({})", m.modseq) } else { String::new() };
                            let _ = write!(reply, "* {seq} FETCH (UID {} FLAGS ({}){modseq})\r\n", m.uid, m.flags.join(" "));
                            continue;
                        }
                        let (key, data) = if headers_only {
                            let end = m.body.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                            ("BODY[HEADER.FIELDS (FROM SUBJECT)]", m.body[..end].to_vec())
                        } else {
                            ("BODY[]", m.body.clone())
                        };
                        tail.extend_from_slice(
                            format!(
                                "* {seq} FETCH (UID {} FLAGS ({}) INTERNALDATE \"02-Nov-2026 10:00:00 +0100\" RFC822.SIZE {} {key} {{{}}}\r\n",
                                m.uid,
                                m.flags.join(" "),
                                m.body.len(),
                                data.len()
                            )
                            .as_bytes(),
                        );
                        tail.extend_from_slice(&data);
                        tail.extend_from_slice(b")\r\n");
                    }
                }
                ("UID", "STORE") => {
                    let set = tokens[2].text().unwrap();
                    let op = tokens[3].text().unwrap().to_ascii_uppercase();
                    let flags: Vec<String> = tokens[4].list().unwrap().iter().filter_map(Token::text).collect();
                    let b = &mut self.boxes[self.selected.unwrap()];
                    let mut modseq = b.modseq;
                    for m in b.messages.iter_mut().filter(|m| Self::matches(&set, m.uid)) {
                        modseq += 1;
                        m.modseq = modseq;
                        for f in &flags {
                            if op.starts_with('+') && !m.flags.contains(f) {
                                m.flags.push(f.clone());
                            } else if op.starts_with('-') {
                                m.flags.retain(|x| x != f);
                            }
                        }
                    }
                    b.modseq = modseq;
                }
                ("UID", "MOVE") => {
                    let set = tokens[2].text().unwrap();
                    let dest = self.find(&tokens[3].text().unwrap()).unwrap();
                    let src = self.selected.unwrap();
                    let moving: Vec<Msg> = self.boxes[src].messages.iter().filter(|m| Self::matches(&set, m.uid)).cloned().collect();
                    self.boxes[src].expunge_where(|m| Self::matches(&set, m.uid));
                    for mut m in moving {
                        m.uid = self.boxes[dest].next_uid;
                        m.modseq = self.boxes[dest].bump();
                        self.boxes[dest].next_uid += 1;
                        self.boxes[dest].messages.push(m);
                        reply += "* 1 EXPUNGE\r\n";
                    }
                }
                ("UID", "EXPUNGE") => {
                    let set = tokens[2].text().unwrap();
                    let b = &mut self.boxes[self.selected.unwrap()];
                    b.expunge_where(|m| Self::matches(&set, m.uid) && m.flags.iter().any(|f| f == "\\Deleted"));
                }
                ("APPEND", _) => {
                    let dest = self.find(&tokens[1].text().unwrap()).unwrap();
                    let flags: Vec<String> = tokens[2].list().unwrap().iter().filter_map(Token::text).collect();
                    assert_eq!(tokens[3].text().unwrap(), "02-Nov-2026 09:00:00 +0000", "date interne transmise par APPEND");
                    let body = tokens[4].bytes().unwrap().to_vec();
                    let b = &mut self.boxes[dest];
                    let uid = b.next_uid;
                    b.next_uid += 1;
                    let modseq = b.bump();
                    b.messages.push(Msg { uid, flags, body, modseq });
                    status = format!("OK [APPENDUID 7 {uid}] APPEND terminé");
                }
                ("ENABLE", _) => {
                    let mut enabled = Vec::new();
                    for ext in tokens[1..].iter().filter_map(Token::text) {
                        if self.caps.split(' ').any(|c| c.eq_ignore_ascii_case(&ext)) {
                            self.qresync |= ext.eq_ignore_ascii_case("QRESYNC");
                            enabled.push(ext);
                        }
                    }
                    let _ = write!(reply, "* ENABLED {}\r\n", enabled.join(" "));
                }
                ("IDLE", _) => {
                    out.write_all(b"+ idling\r\n").unwrap();
                    // Mode « événement » : un nouveau message arrive peu après.
                    if self.caps.contains("IDLE-EVENT") {
                        thread::sleep(std::time::Duration::from_millis(150));
                        out.write_all(b"* 3 EXISTS\r\n").unwrap();
                    }
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    assert_eq!(line.trim_end(), "DONE", "IDLE doit se terminer par DONE");
                }
                ("LOGOUT", _) => {
                    write!(out, "* BYE Au revoir\r\n{tag} OK LOGOUT terminé\r\n").unwrap();
                    return;
                }
                ("KILL", _) => return, // coupe la connexion sans répondre
                _ => status = "BAD commande inconnue".into(),
            }
            out.write_all(reply.as_bytes()).unwrap();
            out.write_all(&tail).unwrap();
            write!(out, "{tag} {status}\r\n").unwrap();
        }
    }
}

fn start(caps: &'static str) -> TcpStream {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        Server::new(caps).serve(stream);
    });
    TcpStream::connect(addr).unwrap()
}

#[test]
fn full_session() {
    let mut c = Client::connect(start("AUTH=PLAIN")).unwrap();
    assert!(c.has_capability("AUTH=PLAIN"));
    assert!(!c.has_capability("MOVE"));

    c.login(USER, PASS).unwrap();
    // Capacités mises à jour après authentification.
    assert!(c.has_capability("MOVE") && c.has_capability("UIDPLUS"));

    let list = c.list().unwrap();
    assert_eq!(list.len(), 3);
    let subscribed: Vec<String> = c.lsub().unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(subscribed, vec!["INBOX".to_string()]);
    let sent = list.iter().find(|e| e.has_attribute("\\Sent")).unwrap();
    assert_eq!(sent.decoded_name(), "Envoyés");
    assert_eq!(sent.name, "Envoy&AOk-s");

    let st = c.status("INBOX").unwrap();
    assert_eq!((st.messages, st.unseen, st.uid_next, st.uid_validity), (2, 1, Some(3), Some(7)));

    let sel = c.select("INBOX").unwrap();
    assert_eq!((sel.exists, sel.uid_validity, sel.uid_next, sel.read_only), (2, Some(7), Some(3), false));
    assert_eq!(c.selected(), Some("INBOX"));

    // Liste : en-têtes seulement.
    let items = c.fetch("1:*", "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT)])").unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].uid(), Some(1));
    assert!(items[0].has_flag("\\Seen"));
    assert!(!items[1].has_flag("\\Seen"));
    assert_eq!(items[1].internal_date(), Some(1_793_610_000));
    let header = String::from_utf8_lossy(items[1].body("BODY[HEADER").unwrap()).to_string();
    assert!(header.contains("Subject: Second"));
    assert!(items[1].body("BODY[]").is_none());

    // Message complet.
    let full = c.uid_fetch(&[2], "(UID FLAGS INTERNALDATE BODY.PEEK[])").unwrap();
    assert_eq!(full.len(), 1);
    let body = full[0].body("BODY[]").unwrap().to_vec();
    assert!(String::from_utf8_lossy(&body).contains("Corps de Second"));

    // Ensemble libre « n:* » (synchronisation incrémentale).
    let all = c.uid_fetch_set("1:*", "(UID FLAGS)").unwrap();
    assert_eq!(all.iter().filter_map(imap_lite::FetchItem::uid).collect::<Vec<_>>(), vec![1, 2]);
    let newer = c.uid_fetch_set("2:*", "(UID FLAGS)").unwrap();
    assert_eq!(newer.iter().filter_map(imap_lite::FetchItem::uid).collect::<Vec<_>>(), vec![2]);

    // Marquer comme lu.
    c.uid_store(&[2], "+FLAGS.SILENT (\\Seen)").unwrap();
    assert_eq!(c.status("INBOX").unwrap().unseen, 0);

    // Déplacement serveur (MOVE disponible).
    c.uid_move(&[1], "Archives").unwrap();
    assert_eq!(c.status("INBOX").unwrap().messages, 1);
    assert_eq!(c.status("Archives").unwrap().messages, 1);

    // Dépôt (littéral client) avec flags et date ; UID renvoyé via APPENDUID.
    let uid = c.append("Envoy&AOk-s", &["\\Seen".into(), "\\Recent".into()], Some(1_793_610_000), &body).unwrap();
    assert_eq!(uid, Some(1));
    assert_eq!(c.status("Envoy&AOk-s").unwrap().messages, 1);

    // Suppression : \Deleted puis UID EXPUNGE.
    c.select("INBOX").unwrap();
    c.uid_store(&[2], "+FLAGS.SILENT (\\Deleted)").unwrap();
    c.uid_expunge(&[2]).unwrap();
    assert_eq!(c.status("INBOX").unwrap().messages, 0);

    c.logout().unwrap();
}

#[test]
fn idle_reports_new_mail() {
    let stream = start("IDLE IDLE-EVENT");
    stream.set_read_timeout(Some(std::time::Duration::from_millis(50))).unwrap();
    let mut c = Client::connect(stream).unwrap();
    c.login(USER, PASS).unwrap();
    c.select("INBOX").unwrap();
    let changed = c.idle(std::time::Duration::from_secs(5), || true).unwrap();
    assert!(changed);
    c.noop().unwrap(); // la session reste utilisable après IDLE
}

#[test]
fn idle_times_out_quietly() {
    let stream = start("IDLE");
    stream.set_read_timeout(Some(std::time::Duration::from_millis(50))).unwrap();
    let mut c = Client::connect(stream).unwrap();
    c.login(USER, PASS).unwrap();
    c.select("INBOX").unwrap();
    let t = std::time::Instant::now();
    let changed = c.idle(std::time::Duration::from_millis(300), || true).unwrap();
    assert!(!changed);
    assert!(t.elapsed() < std::time::Duration::from_secs(2));
    // Arrêt demandé : sortie immédiate.
    assert!(!c.idle(std::time::Duration::from_mins(1), || false).unwrap());
    c.noop().unwrap();
}

#[test]
fn authenticate_plain() {
    let mut c = Client::connect(start("AUTH=PLAIN")).unwrap();
    c.authenticate_plain(USER, PASS).unwrap();
    c.noop().unwrap();
}

#[test]
fn folder_management() {
    let mut c = Client::connect(start("")).unwrap();
    c.login(USER, PASS).unwrap();
    assert_eq!(c.delimiter().unwrap(), '/');
    c.create("Projets/Été").unwrap();
    c.subscribe("Projets/Été").unwrap();
    c.unsubscribe("Projets/Été").unwrap();
    c.delete("Projets/Été").unwrap();
    assert_eq!(c.empty_mailbox("INBOX").unwrap(), 2);
}

#[test]
fn authenticate_xoauth2() {
    let mut c = Client::connect(start("AUTH=XOAUTH2 SASL-IR")).unwrap();
    c.authenticate_xoauth2(USER, "bon-jeton").unwrap();
    c.noop().unwrap();

    let mut c = Client::connect(start("AUTH=XOAUTH2 SASL-IR")).unwrap();
    let err = c.authenticate_xoauth2(USER, "jeton-expiré").unwrap_err();
    assert!(matches!(&err, Error::No(t) if t.contains("AUTHENTICATE failed")), "{err}");
}

#[test]
fn wrong_password_is_a_clear_error() {
    let mut c = Client::connect(start("")).unwrap();
    let err = c.login(USER, "faux").unwrap_err();
    assert!(matches!(&err, Error::No(t) if t.contains("Identifiants")), "{err}");
    assert!(!err.is_connection_lost());
}

#[test]
fn unknown_folder_and_bad_command() {
    let mut c = Client::connect(start("")).unwrap();
    c.login(USER, PASS).unwrap();
    assert!(matches!(c.select("Nulle part"), Err(Error::No(_))));
    assert!(matches!(c.run("FROBNICATE"), Err(Error::Bad(_))));
    // La session reste utilisable après une erreur.
    c.noop().unwrap();
}

#[test]
fn lost_connection_is_detected() {
    let mut c = Client::connect(start("")).unwrap();
    c.login(USER, PASS).unwrap();
    let err = c.run("KILL").unwrap_err();
    assert!(err.is_connection_lost(), "{err}");
}

#[test]
fn qresync_incremental_sync() {
    let mut c = Client::connect(start("ENABLE CONDSTORE QRESYNC")).unwrap();
    c.login(USER, PASS).unwrap();
    c.enable_sync_extensions().unwrap();
    assert!(c.qresync() && c.condstore());

    let first = c.select("INBOX").unwrap();
    let known = first.highest_modseq.expect("HIGHESTMODSEQ annoncé");
    assert!(!first.no_modseq);

    // Rien n'a changé : aucune réponse.
    let none = c.uid_fetch_changed("1:*", "(UID FLAGS)", known).unwrap();
    assert!(none.items.is_empty() && none.vanished.is_empty());

    // « Un autre client » : un suivi posé, un message supprimé.
    c.uid_store(&[2], "+FLAGS.SILENT (\\Flagged)").unwrap();
    c.uid_store(&[1], "+FLAGS.SILENT (\\Deleted)").unwrap();
    c.uid_expunge(&[1]).unwrap();

    let again = c.select("INBOX").unwrap();
    let now = again.highest_modseq.unwrap();
    assert!(now > known);
    let changes = c.uid_fetch_changed("1:*", "(UID FLAGS)", known).unwrap();
    assert_eq!(changes.items.len(), 1);
    assert_eq!(changes.items[0].uid(), Some(2));
    assert!(changes.items[0].has_flag("\\Flagged"));
    assert!(changes.items[0].modseq().is_some_and(|m| m > known && m <= now));
    assert_eq!(changes.vanished.iter().collect::<Vec<_>>(), vec![1]);

    // Depuis le nouveau HIGHESTMODSEQ : plus rien.
    let after = c.uid_fetch_changed("1:*", "(UID FLAGS)", now).unwrap();
    assert!(after.items.is_empty() && after.vanished.is_empty());
    c.logout().unwrap();
}

#[test]
fn condstore_without_qresync() {
    // CONDSTORE sans ENABLE annoncé : pas d'ENABLE, pas de VANISHED, CHANGEDSINCE suffit.
    let mut c = Client::connect(start("CONDSTORE")).unwrap();
    c.login(USER, PASS).unwrap();
    c.enable_sync_extensions().unwrap();
    assert!(c.condstore() && !c.qresync());
    let known = c.select("INBOX").unwrap().highest_modseq.unwrap();
    c.uid_store(&[1], "-FLAGS.SILENT (\\Seen)").unwrap();
    let changes = c.uid_fetch_changed("1:*", "(UID FLAGS)", known).unwrap();
    assert_eq!(changes.items.iter().filter_map(imap_lite::FetchItem::uid).collect::<Vec<_>>(), vec![1]);
    assert!(!changes.items[0].has_flag("\\Seen"));
    assert!(changes.vanished.is_empty());
}

#[test]
fn no_condstore_no_modseq() {
    let mut c = Client::connect(start("")).unwrap();
    c.login(USER, PASS).unwrap();
    c.enable_sync_extensions().unwrap();
    assert!(!c.condstore());
    assert_eq!(c.select("INBOX").unwrap().highest_modseq, None);
}

/// Relecture Y5 : sans UIDPLUS, supprimer un message ne purge plus ceux
/// qu'un autre logiciel avait seulement marqués comme supprimés.
#[test]
fn expunge_without_uidplus_spares_messages_marked_by_others() {
    let mut c = Client::connect(start("NOUIDPLUS")).unwrap();
    c.login(USER, PASS).unwrap();
    assert!(!c.has_capability("UIDPLUS"));
    c.select("INBOX").unwrap();
    // Un autre logiciel a marqué le message 1 ; Vortex Mail supprime le 2.
    c.uid_store(&[1], "+FLAGS.SILENT (\\Deleted)").unwrap();
    c.uid_store(&[2], "+FLAGS.SILENT (\\Deleted)").unwrap();
    c.uid_expunge(&[2]).unwrap();
    assert_eq!(c.uid_search_raw("ALL").unwrap(), vec![1], "le message 1 est toujours là");
    assert_eq!(c.uid_search_raw("DELETED").unwrap(), vec![1], "et toujours marqué");
}
