//! Découverte des serveurs d'agenda et de contacts par le DNS (lot G3,
//! RFC 6764 §3) : enregistrements `SRV` `_caldavs._tcp` et `_carddavs._tcp` du
//! domaine de l'adresse, et `TXT` du même nom pour le chemin (`path=`).
//!
//! La question passe par le résolveur du système, comme dans Thunderbird :
//! - Linux et macOS : un petit client DNS sur UDP, sans dépendance, qui pose la
//!   question aux serveurs de `/etc/resolv.conf` (module `udp`) ;
//! - Windows : `DnsQuery_W` de `dnsapi.dll` (lot G3b, module `win`), qui
//!   suit la configuration réseau de Windows (DHCP, VPN, plusieurs cartes).
//!
//! La réponse n'est pas authentifiée (pas de DNSSEC) : seuls servent les
//! serveurs en HTTPS (pas `_caldav` en clair) dont le nom appartient au même
//! domaine que l'adresse ; le mot de passe ne part de toute façon que vers les
//! sites du compte (`dav::Scope`), avec vérification du certificat.

#[cfg(not(windows))]
use self::udp as system;
#[cfg(windows)]
use self::win as system;

/// Enregistrements lus au plus dans une réponse.
const MAX_RECORDS: usize = 16;

const TYPE_TXT: u16 = 16;
const TYPE_SRV: u16 = 33;

/// Un enregistrement `SRV` : priorité, poids, port, serveur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Srv {
    pub priority: u16,
    pub weight: u16,
    pub port: u16,
    pub target: String,
}

/// Le chemin d'une chaîne `TXT` (`path=/dav/`), s'il est utilisable.
fn txt_path(text: &str) -> Option<String> {
    let path = text.strip_prefix("path=")?;
    (path.starts_with('/') && !path.contains(['\r', '\n', ' '])).then(|| path.to_string())
}

/// Serveurs `_caldavs._tcp` (`calendar`) ou `_carddavs._tcp` du domaine, par priorité puis
/// poids, avec le chemin annoncé par `TXT` ; vide si le DNS ne dit rien.
pub(crate) fn dav_servers(domain: &str, calendar: bool) -> Vec<(Srv, Option<String>)> {
    let service = if calendar { "_caldavs._tcp" } else { "_carddavs._tcp" };
    let name = format!("{service}.{}", domain.trim().trim_end_matches('.'));
    let mut servers = system::srv(&name);
    // Cible « . » : service explicitement absent (RFC 2782).
    servers.retain(|s| !s.target.is_empty() && s.target != "." && s.port != 0);
    servers.sort_by(|a, b| a.priority.cmp(&b.priority).then(b.weight.cmp(&a.weight)));
    if servers.is_empty() {
        return Vec::new();
    }
    let path = system::path(&name);
    servers.into_iter().map(|s| (s, path.clone())).collect()
}

/// Client DNS sur UDP, posé aux serveurs de `/etc/resolv.conf` (sans ce fichier,
/// en mode portable par exemple : rien, on devine alors comme avant).
#[cfg(not(windows))]
mod udp {
    use std::net::{SocketAddr, UdpSocket};
    use std::time::Duration;

    use super::{txt_path, Srv, MAX_RECORDS, TYPE_SRV, TYPE_TXT};

    /// Attente d'une réponse.
    const TIMEOUT: Duration = Duration::from_secs(2);
    /// Taille d'une réponse lue au plus.
    const MAX_RESPONSE: usize = 4096;
    /// Résolveurs essayés au plus.
    const MAX_RESOLVERS: usize = 3;
    /// Sauts de compression suivis au plus dans un nom.
    const MAX_POINTERS: usize = 16;

    const CLASS_IN: u16 = 1;

    /// Enregistrements `SRV` de `name`.
    pub(super) fn srv(name: &str) -> Vec<Srv> {
        ask(name, TYPE_SRV).map_or_default(|(response, id)| parse_srv(&response, id))
    }

    /// Chemin annoncé par l'enregistrement `TXT` de `name`.
    pub(super) fn path(name: &str) -> Option<String> {
        ask(name, TYPE_TXT).and_then(|(response, id)| parse_path(&response, id))
    }

    /// Question DNS pour `name` (type `qtype`), avec l'identifiant `id` et la récursion demandée.
    fn query(id: u16, name: &str, qtype: u16) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(32 + name.len());
        out.extend_from_slice(&id.to_be_bytes());
        // Récursion demandée ; une question.
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        for label in name.trim_end_matches('.').split('.') {
            let len = u8::try_from(label.len()).ok().filter(|l| (1..=63).contains(l))?;
            if !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                return None;
            }
            out.push(len);
            out.extend_from_slice(label.as_bytes());
        }
        out.push(0);
        out.extend_from_slice(&qtype.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        Some(out)
    }

    fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
        let pair = bytes.get(at..at.checked_add(2)?)?;
        Some(u16::from_be_bytes([*pair.first()?, *pair.get(1)?]))
    }

    /// Un nom à la position `at` (compression comprise) et la position qui le suit.
    fn name_at(message: &[u8], mut at: usize) -> Option<(String, usize)> {
        let mut labels: Vec<String> = Vec::new();
        let mut end = None;
        let mut jumps = 0;
        loop {
            let len = *message.get(at)?;
            match len {
                0 => {
                    return Some((labels.join("."), end.unwrap_or(at.checked_add(1)?)));
                }
                l if l & 0xc0 == 0xc0 => {
                    jumps += 1;
                    if jumps > MAX_POINTERS {
                        return None;
                    }
                    let pointer = usize::from(u16_at(message, at)? & 0x3fff);
                    end.get_or_insert(at.checked_add(2)?);
                    at = pointer;
                }
                l if l < 64 => {
                    let start = at.checked_add(1)?;
                    let label = message.get(start..start.checked_add(usize::from(l))?)?;
                    labels.push(String::from_utf8_lossy(label).into_owned());
                    if labels.len() > 127 {
                        return None;
                    }
                    at = start.checked_add(usize::from(l))?;
                }
                _ => return None,
            }
        }
    }

    /// Les données (`RDATA`) des réponses de type `qtype` à la question `id`.
    fn answers(message: &[u8], id: u16, qtype: u16) -> Option<Vec<(usize, usize)>> {
        if u16_at(message, 0)? != id {
            return None;
        }
        let flags = u16_at(message, 2)?;
        // Une réponse (QR), sans erreur (RCODE 0).
        if flags & 0x8000 == 0 || flags & 0x000f != 0 {
            return Some(Vec::new());
        }
        let questions = u16_at(message, 4)?;
        let count = u16_at(message, 6)?;
        let mut at = 12usize;
        for _ in 0..questions {
            at = name_at(message, at)?.1.checked_add(4)?;
        }
        let mut out = Vec::new();
        for _ in 0..count.min(u16::try_from(MAX_RECORDS).unwrap_or(u16::MAX)) {
            at = name_at(message, at)?.1;
            let rtype = u16_at(message, at)?;
            let length = usize::from(u16_at(message, at.checked_add(8)?)?);
            let data = at.checked_add(10)?;
            if data.checked_add(length)? > message.len() {
                return None;
            }
            if rtype == qtype {
                out.push((data, length));
            }
            at = data.checked_add(length)?;
        }
        Some(out)
    }

    /// Les enregistrements `SRV` d'une réponse.
    fn parse_srv(message: &[u8], id: u16) -> Vec<Srv> {
        let Some(found) = answers(message, id, TYPE_SRV) else { return Vec::new() };
        found
            .into_iter()
            .filter_map(|(at, _)| {
                let target = name_at(message, at.checked_add(6)?)?.0;
                Some(Srv { priority: u16_at(message, at)?, weight: u16_at(message, at + 2)?, port: u16_at(message, at + 4)?, target })
            })
            .collect()
    }

    /// Le chemin d'un enregistrement `TXT` (`path=/dav/`), s'il y en a un.
    fn parse_path(message: &[u8], id: u16) -> Option<String> {
        for (at, length) in answers(message, id, TYPE_TXT)? {
            let data = message.get(at..at.checked_add(length)?)?;
            let mut rest = data;
            while let Some((&len, tail)) = rest.split_first() {
                let text = tail.get(..usize::from(len))?;
                if let Some(path) = std::str::from_utf8(text).ok().and_then(txt_path) {
                    return Some(path);
                }
                rest = tail.get(usize::from(len)..)?;
            }
        }
        None
    }

    /// Résolveurs du système (`nameserver` de `/etc/resolv.conf`).
    fn resolvers() -> Vec<SocketAddr> {
        let Ok(text) = std::fs::read_to_string("/etc/resolv.conf") else { return Vec::new() };
        text.lines()
            .filter_map(|l| l.trim().strip_prefix("nameserver"))
            .filter_map(|ip| ip.trim().split('%').next().and_then(|ip| ip.parse::<std::net::IpAddr>().ok()))
            .map(|ip| SocketAddr::new(ip, 53))
            .take(MAX_RESOLVERS)
            .collect()
    }

    /// La réponse brute du premier résolveur qui répond à la question `name` de type `qtype`.
    fn ask(name: &str, qtype: u16) -> Option<(Vec<u8>, u16)> {
        let id = u16::from_be_bytes(crate::oauth::random_bytes::<2>().ok()?);
        let question = query(id, name, qtype)?;
        for resolver in resolvers() {
            let local: SocketAddr = if resolver.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }.parse().ok()?;
            let Ok(socket) = UdpSocket::bind(local) else { continue };
            if socket.set_read_timeout(Some(TIMEOUT)).is_err() || socket.connect(resolver).is_err() || socket.send(&question).is_err() {
                continue;
            }
            let mut buffer = vec![0u8; MAX_RESPONSE];
            if let Ok(n) = socket.recv(&mut buffer) {
                buffer.truncate(n);
                return Some((buffer, id));
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Réponse `SRV` fabriquée : `_caldavs._tcp.exemple.fr` → `dav.exemple.fr:8443`.
        fn srv_response(id: u16) -> Vec<u8> {
            let mut m = query(id, "_caldavs._tcp.exemple.fr", TYPE_SRV).unwrap();
            // Réponse, une réponse.
            m[2] = 0x81;
            m[3] = 0x80;
            m[7] = 1;
            // Nom : pointeur vers la question (position 12).
            m.extend_from_slice(&[0xc0, 0x0c]);
            m.extend_from_slice(&TYPE_SRV.to_be_bytes());
            m.extend_from_slice(&CLASS_IN.to_be_bytes());
            m.extend_from_slice(&[0, 0, 0x0e, 0x10]);
            let mut rdata = vec![0, 10, 0, 5, 0x20, 0xfb];
            rdata.extend_from_slice(b"\x03dav\x07exemple\x02fr\x00");
            m.extend_from_slice(&u16::try_from(rdata.len()).unwrap().to_be_bytes());
            m.extend_from_slice(&rdata);
            m
        }

        #[test]
        fn question_et_reponse_srv() {
            let q = query(0x1234, "_caldavs._tcp.exemple.fr", TYPE_SRV).unwrap();
            assert_eq!(&q[..2], &[0x12, 0x34], "identifiant");
            assert!(query(1, "a b.fr", TYPE_SRV).is_none(), "nom refusé");
            let servers = parse_srv(&srv_response(0x1234), 0x1234);
            assert_eq!(servers, vec![Srv { priority: 10, weight: 5, port: 8443, target: "dav.exemple.fr".into() }]);
            assert!(parse_srv(&srv_response(0x1234), 0x4321).is_empty(), "autre identifiant : ignorée");
        }

        #[test]
        fn chemin_txt() {
            let mut m = query(7, "_caldavs._tcp.exemple.fr", TYPE_TXT).unwrap();
            m[2] = 0x81;
            m[3] = 0x80;
            m[7] = 1;
            m.extend_from_slice(&[0xc0, 0x0c]);
            m.extend_from_slice(&TYPE_TXT.to_be_bytes());
            m.extend_from_slice(&CLASS_IN.to_be_bytes());
            m.extend_from_slice(&[0, 0, 0, 60]);
            let rdata = b"\x0bv=caldav1.0\x0apath=/dav/";
            m.extend_from_slice(&u16::try_from(rdata.len()).unwrap().to_be_bytes());
            m.extend_from_slice(rdata);
            assert_eq!(parse_path(&m, 7).as_deref(), Some("/dav/"));
            assert_eq!(txt_path("path=/a b"), None, "chemin avec espace refusé");
            assert_eq!(txt_path("path=dav"), None, "chemin relatif refusé");
        }

        #[test]
        fn reponses_piegees() {
            // Boucle de compression, longueurs qui dépassent : ni panique ni boucle sans fin.
            let mut looping = srv_response(1);
            let at = looping.len() - 16;
            looping[at] = 0xc0;
            looping[at + 1] = u8::try_from(at).unwrap_or(0);
            let _ = parse_srv(&looping, 1);
            let mut short = srv_response(1);
            short.truncate(short.len() - 3);
            assert!(parse_srv(&short, 1).is_empty(), "réponse tronquée");
            for cut in 0..srv_response(1).len() {
                let _ = parse_srv(&srv_response(1)[..cut], 1);
            }
        }
    }
}

/// Résolveur de Windows : `DnsQuery_W` de `dnsapi.dll` (lot G3b), comme Thunderbird.
///
/// Seul code `unsafe` de Vortex Mail : un appel à l'API du système et la
/// lecture de la liste qu'elle rend, libérée par `DnsFree` (garde `Records`).
/// Chaque bloc ne fait qu'une opération et dit pourquoi elle est sûre.
#[cfg(windows)]
mod win {
    use std::mem::offset_of;
    use std::ptr;

    use windows_sys::Win32::NetworkManagement::Dns::{DnsFree, DnsQuery_W, DNS_RECORDW, DNS_TXT_DATAW};

    use super::{txt_path, Srv, MAX_RECORDS, TYPE_SRV, TYPE_TXT};

    /// Longueur lue au plus d'un nom ou d'une chaîne, en unités UTF-16.
    const MAX_TEXT: usize = 1024;

    /// Liste d'enregistrements rendue par `DnsQuery_W`, libérée en fin de portée.
    struct Records(*mut DNS_RECORDW);

    impl Drop for Records {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: la liste vient de DnsQuery_W et n'est libérée qu'ici, une seule
                // fois ; 1 = DnsFreeRecordList, la libération prévue pour ces listes.
                unsafe { DnsFree(self.0.cast_const().cast(), 1) };
            }
        }
    }

    /// Les enregistrements de type `qtype` pour `name`, demandés au résolveur de Windows.
    fn records(name: &str, qtype: u16) -> Option<Records> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut first: *mut DNS_RECORDW = ptr::null_mut();
        // SAFETY: `wide` est terminée par un zéro et vit pendant tout l'appel ; `first`
        // reçoit la liste, confiée aussitôt à `Records` pour DnsFree. 0 = DNS_QUERY_STANDARD
        // (cache et serveurs du système) ; ni paramètres étendus ni champ réservé.
        let status = unsafe { DnsQuery_W(wide.as_ptr(), qtype, 0, ptr::null_mut(), ptr::addr_of_mut!(first).cast(), ptr::null_mut()) };
        let list = Records(first);
        (status == 0).then_some(list)
    }

    /// Les enregistrements de la liste (adresse et copie), au plus `MAX_RECORDS`.
    fn walk(list: &Records) -> Vec<(*const DNS_RECORDW, DNS_RECORDW)> {
        let mut out = Vec::new();
        let mut at = list.0.cast_const();
        while !at.is_null() && out.len() < MAX_RECORDS {
            // SAFETY: `at` est le début de la liste ou le `pNext` d'un de ses enregistrements ;
            // la liste reste allouée tant que `list` est empruntée.
            let record = unsafe { at.read() };
            let next = record.pNext.cast_const();
            out.push((at, record));
            at = next;
        }
        out
    }

    /// Une chaîne UTF-16 terminée par un zéro, rendue par `DnsQuery_W`.
    fn wide(text: *const u16) -> Option<String> {
        if text.is_null() {
            return None;
        }
        let mut units = Vec::new();
        for i in 0..MAX_TEXT {
            // SAFETY: chaîne terminée par un zéro, dans la liste encore allouée ; la lecture
            // s'arrête au zéro, donc ne dépasse pas la chaîne.
            let unit = unsafe { text.wrapping_add(i).read() };
            if unit == 0 {
                return String::from_utf16(&units).ok();
            }
            units.push(unit);
        }
        None
    }

    /// Enregistrements `SRV` de `name`.
    pub(super) fn srv(name: &str) -> Vec<Srv> {
        let Some(list) = records(name, TYPE_SRV) else { return Vec::new() };
        walk(&list)
            .into_iter()
            // La liste peut aussi contenir des CNAME suivis en chemin.
            .filter(|(_, record)| record.wType == TYPE_SRV)
            .filter_map(|(_, record)| {
                // SAFETY: enregistrement de type SRV : c'est la partie `SRV` de l'union qui est remplie.
                let data = unsafe { record.Data.SRV };
                let target = wide(data.pNameTarget.cast_const())?;
                Some(Srv { priority: data.wPriority, weight: data.wWeight, port: data.wPort, target })
            })
            .collect()
    }

    /// Chemin annoncé par l'enregistrement `TXT` de `name`.
    pub(super) fn path(name: &str) -> Option<String> {
        let list = records(name, TYPE_TXT)?;
        for (at, record) in walk(&list) {
            if record.wType != TYPE_TXT {
                continue;
            }
            // SAFETY: enregistrement de type TXT : c'est la partie `TXT` de l'union qui est remplie.
            let count = unsafe { record.Data.TXT.dwStringCount };
            // `pStringArray` est déclaré avec une case mais en compte `dwStringCount`, à la
            // suite dans l'enregistrement alloué : on y accède depuis l'adresse de
            // l'enregistrement, pas depuis la copie.
            let strings = at.wrapping_byte_add(offset_of!(DNS_RECORDW, Data) + offset_of!(DNS_TXT_DATAW, pStringArray)).cast::<*mut u16>();
            for i in 0..usize::try_from(count).unwrap_or(0).min(MAX_RECORDS) {
                // SAFETY: case `i` < `dwStringCount` du tableau de l'enregistrement, encore alloué.
                let text = unsafe { strings.wrapping_add(i).read() };
                if let Some(path) = wide(text.cast_const()).as_deref().and_then(txt_path) {
                    return Some(path);
                }
            }
        }
        None
    }
}
