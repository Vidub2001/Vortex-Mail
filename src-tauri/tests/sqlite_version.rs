//! Garde-fou du lot 0b : le SQLite embarqué doit rester en 3.54.0 ou plus
//! récent (RUSTSEC-2026-0459 / CVE-2026-119375 : mémoire non initialisée
//! renvoyée à l'ouverture d'une base avec un fichier -wal piégé).
//!
//! Exception temporaire du lot G4 : le `SQLCipher` du cache chiffré, imposé par
//! `libsqlite3-sys` 0.39.1 (`sqlcipher-src` =419.0.6), embarque SQLite 3.53.4,
//! et seulement cette version. L'exception est à retirer, avec RUSTSEC-2026-0502
//! dans `deny.toml`, dès qu'une version de `libsqlite3-sys` tire `sqlcipher-src`
//! 420 ou plus (SQLite 3.54.1) : ce test échouera alors de lui-même.

/// Seule version tolérée en dessous de 3.54.0 (lot G4, voir plus haut).
const SQLCIPHER_TOLERE: i32 = 3_053_004;

#[test]
fn sqlite_embarque_corrige() {
    let version = rusqlite::version_number();
    assert!(
        version >= 3_054_000 || version == SQLCIPHER_TOLERE,
        "SQLite {} embarqué : 3.54.0 ou plus récent requis (RUSTSEC-2026-0459)",
        rusqlite::version()
    );
    assert!(version != SQLCIPHER_TOLERE || sqlcipher(), "SQLite 3.53.4 toléré seulement pour SQLCipher (lot G4)");
}

/// Le SQLite embarqué est-il celui de `SQLCipher` ?
fn sqlcipher() -> bool {
    rusqlite::Connection::open_in_memory().is_ok_and(|conn| conn.query_row("PRAGMA cipher_version", [], |r| r.get::<_, String>(0)).is_ok())
}

/// Le jour où `libsqlite3-sys` tire un `SQLCipher` corrigé, l'exception doit partir.
#[test]
fn exception_g4_encore_utile() {
    let version = rusqlite::version_number();
    assert!(
        version < 3_054_000 || !sqlcipher(),
        "SQLCipher embarque maintenant SQLite {} : retirez SQLCIPHER_TOLERE de ce fichier et RUSTSEC-2026-0502 de deny.toml",
        rusqlite::version()
    );
}
