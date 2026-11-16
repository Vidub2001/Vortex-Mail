#!/usr/bin/env python3
"""Relevé des avis de sécurité Rust acceptés temporairement.

Lit la sortie de `cargo deny --format json check advisories` et réécrit le bloc
« acceptés temporairement » de deny.toml (entre les marqueurs >>> et <<<).
Les avis qui ne s'appliquent plus disparaissent du bloc. Un avis nouveau n'y
entre PAS : il est signalé et le relevé échoue, pour qu'il soit traité
(relecture complète n° 48). Pour l'accepter délibérément :
`--accept-new` (make audit-baseline ACCEPTER_NOUVEAUX=1), avec la raison
« accepté à traiter », à réécrire. Chaque entrée garde le nom de la
crate et le titre de l'avis, pour qu'on sache ce qu'on accepte. Une raison
réécrite à la main est conservée d'un relevé à l'autre.

Usage : deny_baseline.py [--accept-new] <deny.json> <deny.toml>
        deny_baseline.py --clear <deny.toml>   (vide le bloc avant un nouveau relevé,
                                                 sinon les avis déjà acceptés ne
                                                 seraient plus signalés)
"""
import json
import re
import sys
from pathlib import Path

START = "# >>> acceptés temporairement"
END = "# <<< acceptés temporairement"
REASONS = Path("target/audit/deny-reasons.json")
DEFAULT_REASON = "accepté à traiter, raison à écrire"
ENTRY = re.compile(r'id\s*=\s*"([^"]+)"\s*,\s*reason\s*=\s*"((?:[^"\\]|\\.)*)"')
ID = re.compile(r"\b(RUSTSEC-\d{4}-\d{4}|GHSA(?:-[0-9a-z]{4}){3})\b")


def advisories(path):
    found = {}
    for line in Path(path).read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        fields = msg.get("fields", {})
        adv = fields.get("advisory") or {}
        ident = adv.get("id")
        if not ident:
            m = ID.search(json.dumps(fields))
            ident = m.group(1) if m else None
        if not ident:
            continue
        kind = fields.get("code", "")
        package = adv.get("package", "")
        title = adv.get("title") or fields.get("message", "")
        found[ident] = (kind, package, " ".join(str(title).split()))
    return found


def main(deny_json, deny_toml, accept_new=False):
    toml_path = Path(deny_toml)
    text = toml_path.read_text(encoding="utf-8")
    if START not in text or END not in text:
        sys.exit(f"Marqueurs du bloc introuvables dans {deny_toml}")
    head, rest = text.split(START, 1)
    block, tail = rest.split(END, 1)
    if deny_json is None:
        # Avant un nouveau relevé : on garde de côté les raisons écrites.
        reasons = {m.group(1): m.group(2) for m in ENTRY.finditer(block)}
        REASONS.parent.mkdir(parents=True, exist_ok=True)
        REASONS.write_text(json.dumps(reasons, ensure_ascii=False, indent=1), encoding="utf-8")
        found = {}
    else:
        reasons = json.loads(REASONS.read_text(encoding="utf-8")) if REASONS.exists() else {}
        found = advisories(deny_json)
        new = {i: v for i, v in found.items() if i not in reasons}
        if not accept_new:
            found = {i: v for i, v in found.items() if i in reasons}
    lines = []
    for ident, (kind, package, title) in sorted(found.items()):
        label = f"{package} : {title}" if package else title
        lines.append(f"    # {kind} — {label}"[:160])
        reason = reasons.get(ident, DEFAULT_REASON)
        lines.append(f'    {{ id = "{ident}", reason = "{reason}" }},')
    block_head = START + " (généré par `make audit-baseline` ; les raisons peuvent être réécrites)\n"
    body = "\n".join(lines) + ("\n" if lines else "")
    toml_path.write_text(head + block_head + body + "    " + END + tail, encoding="utf-8")
    if deny_json is None:
        return
    print(f"Relevé cargo-deny : {len(found)} avis accepté(s) temporairement → {deny_toml}")
    for ident, (kind, package, _) in sorted(found.items()):
        print(f"  {ident}  {kind:15} {package}")
    if new and not accept_new:
        print(f"✖ {len(new)} avis nouveau(x), non accepté(s) : mettre à jour la crate, ou relancer avec ACCEPTER_NOUVEAUX=1")
        for ident, (kind, package, title) in sorted(new.items()):
            print(f"  {ident}  {kind:15} {package} : {title[:100]}")
        sys.exit(1)


if __name__ == "__main__":
    args = sys.argv[1:]
    accept = "--accept-new" in args
    args = [a for a in args if a != "--accept-new"]
    if len(args) != 2:
        sys.exit(__doc__)
    if args[0] == "--clear":
        main(None, args[1])
    else:
        main(args[0], args[1], accept_new=accept)
