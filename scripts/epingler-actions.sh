#!/usr/bin/env bash
# Épingle chaque action GitHub sur l'empreinte (SHA) de son étiquette :
#   uses: actions/checkout@v5   devient   uses: actions/checkout@<sha> # v5
# Une étiquette peut être déplacée par l'auteur de l'action (ou par un attaquant
# qui a pris son compte) ; une empreinte de commit, non.
# Relancer après avoir changé une étiquette « # vX » pour mettre à jour l'empreinte.
# Prérequis : gh connecté (gh auth login).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
for f in .github/workflows/*.yml; do
  grep -oE 'uses: [A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+@[^ ]+( # [^ ]+)?' "$f" | sort -u | while read -r line; do
    ref=${line#uses: }
    action=${ref%%@*}
    rest=${ref#*@}
    if [[ $rest == *" # "* ]]; then tag=${rest##* # }; else tag=$rest; fi
    sha=$(gh api "repos/$action/commits/$tag" --jq .sha)
    [[ $sha =~ ^[0-9a-f]{40}$ ]] || { echo "Empreinte introuvable pour $action@$tag" >&2; exit 1; }
    new="uses: $action@$sha # $tag"
    if [[ $line != "$new" ]]; then
      python3 - "$f" "$line" "$new" <<'PY'
import sys
path, old, new = sys.argv[1:]
text = open(path, encoding="utf-8").read()
open(path, "w", encoding="utf-8").write(text.replace(old + "\n", new + "\n"))
PY
      echo "$f : $action@$tag -> ${sha:0:12}"
    fi
  done
done
git --no-pager diff --stat -- .github/workflows
