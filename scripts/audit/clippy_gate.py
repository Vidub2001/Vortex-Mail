#!/usr/bin/env python3
"""Alertes Clippy : relevé, contrôle de non-régression et export SonarQube.

Entrée : la sortie de `cargo clippy --message-format=json` (une ligne JSON par
message de cargo).

  clippy_gate.py baseline <clippy.json> <baseline.json>
      Enregistre les alertes actuelles comme « acceptées temporairement ».
  clippy_gate.py check <clippy.json> <baseline.json>
      Échoue si une alerte n'est pas dans le relevé, ou en cas d'erreur de
      compilation. Les alertes disparues sont signalées (relevé à rafraîchir).
  clippy_gate.py sonar <clippy.json> <sortie.json>
      Convertit les alertes au format générique de SonarQube
      (sonar.externalIssuesReportPaths).

Une alerte est identifiée par (lint, fichier, message) et comptée : on ne
dépend pas des numéros de ligne, qui bougent à chaque modification.
"""
import json
import re
import sys
from collections import Counter
from pathlib import Path


def diagnostics(path):
    """Messages du compilateur portant sur le code du dépôt (pas les dépendances)."""
    seen = set()
    for line in Path(path).read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("reason") != "compiler-message":
            continue
        d = msg.get("message") or {}
        level = d.get("level")
        if level not in ("warning", "error"):
            continue
        spans = [s for s in d.get("spans", []) if s.get("is_primary")]
        span = spans[0] if spans else None
        file = span["file_name"] if span else ""
        if file.startswith("/") or ".cargo/registry" in file:
            continue  # code d'une dépendance
        code = (d.get("code") or {}).get("code") or ""
        text = d.get("message", "")
        if not code and text.startswith(("warning: ", "aborting", "could not compile")):
            continue
        if not code and "warning" in text and "emitted" in text:
            continue  # récapitulatif « N warnings emitted »
        # --all-targets compile parfois le même fichier deux fois (lib et tests) :
        # on ne garde qu'un exemplaire de chaque message au même endroit.
        key = (code, file, span["line_start"] if span else 0, span["column_start"] if span else 0, text)
        if key in seen:
            continue
        seen.add(key)
        yield {
            "level": level,
            "code": code,
            "file": file,
            "line_start": span["line_start"] if span else 1,
            "line_end": span["line_end"] if span else 1,
            "message": text,
        }


def normalize(key):
    """Chiffres neutralisés : « too many lines (126/100) » reste la même alerte
    quand la fonction gagne une ligne."""
    return re.sub(r"\d+", "N", key)


def fingerprint(d):
    return normalize(f"{d['code'] or d['level']}|{d['file']}|{d['message']}")


def baseline(clippy, out):
    diags = list(diagnostics(clippy))
    errors = [d for d in diags if d["level"] == "error"]
    if errors:
        for d in errors[:20]:
            print(f"  ✖ {d['file']}:{d['line_start']} {d['message']}")
        sys.exit("Le code ne compile pas : corrige ces erreurs avant de faire le relevé.")
    counts = Counter(fingerprint(d) for d in diags)
    data = {
        "_commentaire": "Alertes Clippy acceptées temporairement (relevé du lot 0). "
        "Régénérer avec `make audit-baseline` après correction ; le lot 8 rend Clippy bloquant.",
        "total": sum(counts.values()),
        "alertes": dict(sorted(counts.items())),
    }
    Path(out).write_text(json.dumps(data, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    by_lint = Counter((d["code"] or d["level"]) for d in diags)
    print(f"Relevé Clippy : {data['total']} alertes acceptées temporairement → {out}")
    for lint, n in by_lint.most_common(10):
        print(f"  {n:5}  {lint}")


def check(clippy, base):
    base_path = Path(base)
    if not base_path.exists():
        sys.exit(f"{base} absent : lance d'abord `make audit-baseline`.")
    accepted = Counter()
    for key, count in json.loads(base_path.read_text(encoding="utf-8"))["alertes"].items():
        accepted[normalize(key)] += count
    diags = list(diagnostics(clippy))
    errors = [d for d in diags if d["level"] == "error"]
    current = Counter(fingerprint(d) for d in diags)
    new = current - accepted
    gone = accepted - current
    if errors:
        print("✖ Clippy : erreurs de compilation")
        for d in errors[:20]:
            print(f"  {d['file']}:{d['line_start']} {d['message']}")
    if new:
        print(f"✖ Clippy : {sum(new.values())} nouvelle(s) alerte(s) hors relevé")
        shown = 0
        for d in diags:
            if new.get(fingerprint(d), 0) > 0 and shown < 40:
                print(f"  {d['file']}:{d['line_start']} [{d['code']}] {d['message']}")
                shown += 1
    if errors or new:
        sys.exit(1)
    msg = f"✔ Clippy : {sum(current.values())} alerte(s), toutes dans le relevé"
    if gone:
        msg += f" ({sum(gone.values())} corrigée(s) : `make audit-baseline` pour mettre le relevé à jour)"
    print(msg)


def sonar(clippy, out):
    diags = [d for d in diagnostics(clippy) if d["file"]]
    rules, issues = {}, []
    for d in diags:
        rule = d["code"] or f"rustc::{d['level']}"
        if rule not in rules:
            url = ""
            if rule.startswith("clippy::"):
                url = f"https://rust-lang.github.io/rust-clippy/master/#{rule.removeprefix('clippy::')}"
            rules[rule] = {
                "id": rule,
                "name": rule,
                "description": f"Alerte Rust {rule}. {url}".strip(),
                "engineId": "clippy",
                "cleanCodeAttribute": "CONVENTIONAL",
                "impacts": [{
                    "softwareQuality": "RELIABILITY" if d["level"] == "error" else "MAINTAINABILITY",
                    "severity": "HIGH" if d["level"] == "error" else "LOW",
                }],
            }
        issues.append({
            "ruleId": rule,
            "primaryLocation": {
                "message": d["message"],
                "filePath": d["file"],
                # Lignes seulement : les colonnes de rustc et de SonarQube
                # n'ont pas la même origine, autant ne pas risquer un rejet.
                "textRange": {"startLine": d["line_start"], "endLine": max(d["line_end"], d["line_start"])},
            },
        })
    Path(out).write_text(json.dumps({"rules": list(rules.values()), "issues": issues}, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"Export SonarQube : {len(issues)} alerte(s), {len(rules)} règle(s) → {out}")


if __name__ == "__main__":
    if len(sys.argv) != 4 or sys.argv[1] not in ("baseline", "check", "sonar"):
        sys.exit(__doc__)
    {"baseline": baseline, "check": check, "sonar": sonar}[sys.argv[1]](sys.argv[2], sys.argv[3])
