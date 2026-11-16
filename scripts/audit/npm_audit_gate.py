#!/usr/bin/env python3
"""Filtre `npm audit --json` : échoue sur tout avis de gravité haute ou critique
qui n'est pas accepté dans security/accepted.toml (section [[npm]]).

Usage : npm_audit_gate.py <npm-audit.json> <accepted.toml>
"""
import json
import sys
import tomllib
from pathlib import Path

BLOCKING = {"high", "critical"}


def main(report_path, accepted_path):
    raw = Path(report_path).read_text(encoding="utf-8").strip()
    if not raw:
        sys.exit("✖ npm audit n'a rien renvoyé (réseau ? npm installé ?)")
    data = json.loads(raw)
    if "error" in data:
        sys.exit(f"✖ npm audit : {data['error'].get('summary', data['error'])}")

    accepted = {e["avis"] for e in tomllib.loads(Path(accepted_path).read_text(encoding="utf-8")).get("npm", [])}

    blocking, tolerated, minor = [], [], 0
    for name, vuln in data.get("vulnerabilities", {}).items():
        for via in vuln.get("via", []):
            if not isinstance(via, dict):
                continue  # vulnérabilité héritée d'un autre paquet, déjà listée
            sev = via.get("severity", vuln.get("severity", ""))
            url = via.get("url", "")
            line = f"{name} [{sev}] {via.get('title', '')} {url}"
            if sev not in BLOCKING:
                minor += 1
            elif url in accepted:
                tolerated.append(line)
            else:
                blocking.append(line)

    if blocking:
        print("✖ npm audit : avis bloquants")
        for line in blocking:
            print(f"  {line}")
        print("  → `npm audit fix`, ou acceptation motivée dans security/accepted.toml")
        sys.exit(1)
    msg = f"✔ npm audit : aucun avis haut ou critique non accepté"
    if tolerated:
        msg += f" ({len(tolerated)} accepté(s) temporairement)"
    if minor:
        msg += f", {minor} avis de gravité faible ou moyenne (npm audit pour le détail)"
    print(msg)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(*sys.argv[1:])
