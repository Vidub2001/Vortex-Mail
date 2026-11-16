#!/usr/bin/env python3
"""Vérifie que les outils de `make audit` sont installés ; n'installe rien."""
import shutil
import subprocess
import sys

if sys.version_info < (3, 11):
    sys.exit("Python 3.11 ou plus récent est requis (module tomllib).")

TOOLS = [
    ("cargo", ["cargo", "--version"], "https://rustup.rs"),
    ("cargo clippy", ["cargo", "clippy", "--version"], "rustup component add clippy"),
    ("cargo fmt", ["cargo", "fmt", "--version"], "rustup component add rustfmt"),
    ("cargo deny", ["cargo", "deny", "--version"], "cargo install --locked cargo-deny"),
    ("npm", ["npm", "--version"], "sudo apt install npm"),
    ("semgrep", ["semgrep", "--version"], "pipx install semgrep"),
]

missing = []
for name, cmd, hint in TOOLS:
    if shutil.which(cmd[0]) is None:
        missing.append((name, hint))
        continue
    try:
        subprocess.run(cmd, check=True, capture_output=True, timeout=60)
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
        missing.append((name, hint))

if missing:
    print("Outils manquants pour make audit :")
    for name, hint in missing:
        print(f"  - {name:13} → {hint}")
    sys.exit(1)
