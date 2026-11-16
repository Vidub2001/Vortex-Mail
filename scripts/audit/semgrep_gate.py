#!/usr/bin/env python3
"""Contrôle bloquant des résultats Semgrep (lot 8 de docs/plan-lots.md).

Usage : semgrep_gate.py <semgrep.json> <racine du dépôt>

Échoue :
- si Semgrep n'a rien produit, ou a rencontré une erreur (règle invalide…) ;
- dès la première occurrence d'une règle maison : plus aucune tolérance ;
- si une exception `nosemgrep` ne nomme pas ses règles ou ne donne pas sa
  raison. Forme attendue, sur la ligne signalée ou celle d'avant :
      // nosemgrep: vortex-regle -- raison de l'exception
  Sans nom de règle, Semgrep ignorerait toutes les règles sur cette ligne.
"""
import json
import re
import subprocess
import sys
from pathlib import Path

SOURCES = (".rs", ".ts", ".tsx", ".js", ".mjs")
# Fichiers de test des règles : ils contiennent exprès ce que les règles repèrent.
IGNORED_DIRS = (".semgrep/", "node_modules/", "target/", "dist/")
EXCEPTION = re.compile(r"nosemgrep(?P<rest>.*)$")
MIN_REASON = 10


def tracked_sources(root):
    """Fichiers source suivis par git (ou, à défaut, présents sur le disque)."""
    try:
        out = subprocess.run(
            ["git", "-C", str(root), "ls-files"], check=True, capture_output=True, text=True
        ).stdout.splitlines()
    except (OSError, subprocess.CalledProcessError):
        out = [str(p.relative_to(root)) for p in Path(root).rglob("*") if p.is_file()]
    return [
        f for f in out
        if f.endswith(SOURCES) and not any(f.startswith(d) or f"/{d}" in f for d in IGNORED_DIRS)
    ]


def check_exceptions(root):
    """Exceptions `nosemgrep` : (valides, mal formées), chacune « fichier:ligne : texte »."""
    valid, invalid = [], []
    for name in tracked_sources(root):
        try:
            text = (Path(root) / name).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for number, line in enumerate(text.splitlines(), start=1):
            found = EXCEPTION.search(line)
            if not found:
                continue
            where = f"{name}:{number} : {line.strip()[:160]}"
            rest = found.group("rest")
            rules, _, reason = rest.lstrip(":").partition("--")
            names = [r.strip() for r in rules.split(",") if r.strip()]
            if (
                not rest.startswith(":")
                or not names
                or not all(r.startswith("vortex-") for r in names)
                or len(reason.strip()) < MIN_REASON
            ):
                invalid.append(where)
            else:
                valid.append(where)
    return valid, invalid


def main(results_path, root):
    raw = Path(results_path).read_text(encoding="utf-8") if Path(results_path).exists() else ""
    if not raw.strip():
        sys.exit("✖ Semgrep n'a produit aucun résultat (est-il installé ? voir make audit-tools)")
    data = json.loads(raw)

    failed = False
    # Toute erreur compte, avertissements compris : un fichier analysé en
    # partie (erreur de syntaxe, délai) n'est plus couvert par les règles
    # (relecture complète n° 48).
    fatal = list(data.get("errors", []))
    if fatal:
        failed = True
        print("✖ Semgrep : erreurs d'analyse (fichiers non couverts par les règles)")
        for e in fatal[:10]:
            print(f"  {e.get('level', '?')} {e.get('type')}: {' '.join(str(e.get('message', '')).split())[:300]}")

    # Fichier source sauté pour une raison autre qu'une exclusion voulue.
    voulus = {"semgrepignore_patterns_match", "cli_include_flags_do_not_match", "cli_exclude_flags_match",
              "gitignore_patterns_match", "always_skipped", "wrong_language", "irrelevant_rule", "dotfile",
              "excluded_by_config"}
    sautes = [s for s in (data.get("paths", {}).get("skipped") or [])
              if str(s.get("path", "")).endswith((".rs", ".ts", ".tsx"))
              and str(s.get("reason", "")).lower() not in voulus]
    if sautes:
        failed = True
        print("✖ Semgrep : fichiers source non analysés")
        for s in sautes[:10]:
            print(f"  {s.get('path')} ({s.get('reason')})")

    # Une occurrence par (règle, fichier, ligne) : deux motifs d'une même règle
    # sur la même ligne ne comptent qu'une fois.
    findings = {}
    for r in data.get("results", []):
        rule = r["check_id"].rsplit(".", 1)[-1]
        path = r["path"].removeprefix("./")
        findings[(rule, path, r["start"]["line"])] = " ".join(r.get("extra", {}).get("message", "").split())
    for (rule, path, line), message in sorted(findings.items()):
        failed = True
        print(f"✖ Semgrep {rule} : {path}:{line}")
        print(f"  {message[:400]}")

    valid, invalid = check_exceptions(root)
    for where in invalid:
        failed = True
        print(f"✖ Exception Semgrep sans règle nommée ou sans raison : {where}")
    if invalid:
        print("  forme attendue : // nosemgrep: vortex-regle -- raison de l'exception")

    if failed:
        sys.exit(1)
    print(f"✔ Semgrep : aucune occurrence ({len(valid)} exception(s) justifiée(s))")
    for where in valid:
        print(f"  · {where}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(*sys.argv[1:])
