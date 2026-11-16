#!/usr/bin/env python3
"""Vérifie la cohérence du corpus de non-régression (tests/corpus).

- chaque .eml de attaques/ est déclaré dans manifest.toml, et inversement ;
- chaque .eml se lit comme un mail, et porte la pièce jointe annoncée ;
- chaque entrée a un lot, un attendu et un état valide ;
- chaque scénario a un identifiant unique et des étapes ;
- chaque entrée « corrige » nomme au moins un test, qui existe (lot 9b) :
  « chemin::fonction » pour un test Rust (#[test] fn fonction), « chemin::titre »
  pour un test de l'interface (test("titre", …)).

Usage : corpus_check.py <dossier du corpus>
"""
import email
import re
import sys
import tomllib
from email import policy
from pathlib import Path

ETATS = {"accepte-temporairement", "corrige"}


def test_exists(repo, ref):
    """Le test désigné par « chemin::nom » existe-t-il ?"""
    path, sep, name = ref.partition("::")
    if not sep or not name:
        return False
    file = repo / path
    if not file.is_file():
        return False
    # Commentaires retirés : un test mis en commentaire n'existe plus
    # (relecture complète n° 48).
    text = strip_comments(file.read_text(encoding="utf-8"))
    if path.endswith(".rs"):
        # La fonction doit être précédée de #[test] (attributs intermédiaires
        # admis, sauf #[ignore] : un test ignoré ne vérifie rien).
        found = re.search(r"#\[test\]((?:\s*#\[[^\]]*\])*)\s*fn " + re.escape(name) + r"\(", text)
        return found is not None and "#[ignore" not in found.group(1)
    found = re.search(r"\btest(\.\w+)?\(\s*([\"'`])" + re.escape(name) + r"\2", text)
    # test.skip(…) et test.todo(…) ne vérifient rien non plus.
    return found is not None and found.group(1) not in (".skip", ".todo")


def strip_comments(text):
    """Retire les commentaires // et /* … */ (assez précis pour des tests)."""
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    return re.sub(r"(?m)^\s*//.*$", "", text)


def check_tests(repo, label, entry, errors):
    tests = entry.get("tests", [])
    if entry.get("etat") == "corrige" and not tests:
        errors.append(f"{label} : état « corrige » sans test nommé (tests = [\"chemin::fonction\"])")
    for ref in tests:
        if not test_exists(repo, ref):
            errors.append(f"{label} : test introuvable « {ref} »")


def main(root):
    root = Path(root)
    manifest = tomllib.loads((root / "manifest.toml").read_text(encoding="utf-8"))
    attaques = manifest.get("attaque", [])
    scenarios = manifest.get("scenario", [])
    errors = []

    on_disk = {p.name for p in (root / "attaques").glob("*.eml")}
    declared = [a.get("fichier", "") for a in attaques]
    for name in sorted(on_disk - set(declared)):
        errors.append(f"attaques/{name} n'est pas déclaré dans manifest.toml")
    for name in sorted({d for d in declared if declared.count(d) > 1}):
        errors.append(f"{name} est déclaré plusieurs fois")

    repo = root.resolve().parent.parent
    for a in attaques:
        name = a.get("fichier", "?")
        check_tests(repo, name, a, errors)
        for key in ("relecture", "lot", "attendu", "etat"):
            if key not in a:
                errors.append(f"{name} : champ « {key} » manquant")
        if a.get("etat") not in ETATS:
            errors.append(f"{name} : état « {a.get('etat')} » inconnu")
        path = root / "attaques" / name
        if not path.exists():
            errors.append(f"{name} : fichier absent")
            continue
        msg = email.message_from_bytes(path.read_bytes(), policy=policy.default)
        if msg["From"] is None or msg["Subject"] is None:
            errors.append(f"{name} : en-têtes From ou Subject absents")
        if msg.defects:
            errors.append(f"{name} : mail mal formé ({msg.defects[0].__class__.__name__})")
        wanted = a.get("piece_jointe")
        if wanted is not None:
            names = [p.get_filename() for p in msg.iter_attachments()]
            if wanted not in names:
                errors.append(f"{name} : pièce jointe {wanted!r} absente (trouvé {names})")

    ids = [s.get("id") for s in scenarios]
    for s in scenarios:
        sid = s.get("id", "?")
        check_tests(repo, f"scénario {sid}", s, errors)
        if ids.count(sid) > 1:
            errors.append(f"scénario {sid} déclaré plusieurs fois")
        for key in ("lot", "titre", "etapes", "attendu", "etat"):
            if key not in s:
                errors.append(f"scénario {sid} : champ « {key} » manquant")
        if s.get("etat") not in ETATS:
            errors.append(f"scénario {sid} : état « {s.get('etat')} » inconnu")
        if not s.get("etapes"):
            errors.append(f"scénario {sid} : aucune étape")

    if errors:
        print("✖ Corpus :")
        for e in errors:
            print(f"  {e}")
        sys.exit(1)

    pending = sum(1 for x in attaques + scenarios if x.get("etat") == "accepte-temporairement")
    tested = sum(1 for x in attaques + scenarios if x.get("tests"))
    print(f"✔ Corpus : {len(attaques)} mail(s) piégé(s), {len(scenarios)} scénario(s) de perte de données, "
          f"{pending} accepté(s) temporairement, {tested} vérifié(s) par un test nommé")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
