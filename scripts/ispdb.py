#!/usr/bin/env python3
"""Copie de l'ISPDB de Thunderbird intégrée à Vortex Mail (lot I1).

  scripts/ispdb.py maj [dossier]   rafraîchit src-tauri/ispdb/ispdb.xml depuis le dépôt
                                   thunderbird/autoconfig (cloné ou mis à jour dans
                                   target/ispdb-src, ou le dossier donné)
  scripts/ispdb.py age [jours]     échoue si la copie a plus de `jours` jours (60 par défaut)

La copie ne garde de chaque fournisseur que ce que lit `autoconfig.rs` : domaines, nom,
serveurs IMAP et SMTP, page d'aide. Licence MPL-2.0 recopiée à côté (src-tauri/ispdb/LICENSE).
Quelques fichiers d'origine servent d'amorces au fuzzing (fuzz/seeds/autoconfig/).
"""

import datetime
import pathlib
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET

RACINE = pathlib.Path(__file__).resolve().parent.parent
DEPOT = "https://github.com/thunderbird/autoconfig"
SOURCE = RACINE / "target" / "ispdb-src"
SORTIE = RACINE / "src-tauri" / "ispdb"
AMORCES = RACINE / "fuzz" / "seeds" / "autoconfig"
CHAMPS = ("hostname", "port", "socketType", "username", "authentication")


def recuperer(dossier: pathlib.Path) -> str:
    """Clone ou met à jour le dépôt ; rend l'empreinte du commit lu."""
    if (dossier / ".git").is_dir():
        subprocess.run(["git", "-C", str(dossier), "pull", "--ff-only", "--quiet"], check=True)
    else:
        subprocess.run(["git", "clone", "--depth", "1", "--quiet", DEPOT, str(dossier)], check=True)
    sortie = subprocess.run(["git", "-C", str(dossier), "rev-parse", "HEAD"], check=True, capture_output=True, text=True)
    return sortie.stdout.strip()


def reduire(fournisseur: ET.Element) -> ET.Element | None:
    """Le fournisseur réduit à ce que lit Vortex Mail ; None s'il n'a ni domaine ni serveur IMAP."""
    garde = ET.Element("emailProvider")
    for domaine in fournisseur.findall("domain"):
        if domaine.text and domaine.text.strip():
            ET.SubElement(garde, "domain").text = domaine.text.strip()
    nom = fournisseur.findtext("displayName")
    if nom:
        ET.SubElement(garde, "displayName").text = nom.strip()
    serveurs = 0
    for balise, genre in (("incomingServer", "imap"), ("outgoingServer", "smtp")):
        for serveur in fournisseur.findall(balise):
            if (serveur.get("type") or "").lower() != genre:
                continue
            copie = ET.SubElement(garde, balise, type=genre)
            for champ in CHAMPS:
                for valeur in serveur.findall(champ):
                    if valeur.text and valeur.text.strip():
                        ET.SubElement(copie, champ).text = valeur.text.strip()
            serveurs += genre == "imap"
    for doc in fournisseur.findall("documentation"):
        if (doc.get("url") or "").startswith("https://"):
            ET.SubElement(garde, "documentation", url=doc.get("url"))
            break
    if not garde.findall("domain") or not serveurs:
        return None
    return garde


def maj(dossier: pathlib.Path) -> None:
    commit = recuperer(dossier)
    fichiers = sorted((dossier / "ispdb").glob("*.xml"))
    if len(fichiers) < 100:
        sys.exit(f"ISPDB : {len(fichiers)} fichiers seulement dans {dossier / 'ispdb'}, dépôt inattendu")
    racine = ET.Element("ispdb", source=DEPOT, commit=commit, date=datetime.date.today().isoformat())
    gardes = 0
    for fichier in fichiers:
        try:
            config = ET.parse(fichier).getroot()
        except ET.ParseError as erreur:
            print(f"  ignoré (illisible) : {fichier.name} : {erreur}")
            continue
        fournisseur = config.find("emailProvider")
        reduit = reduire(fournisseur) if fournisseur is not None else None
        if reduit is None:
            continue
        enveloppe = ET.SubElement(racine, "clientConfig", version="1.1")
        enveloppe.append(reduit)
        gardes += 1
    ET.indent(racine, space=" ")
    SORTIE.mkdir(parents=True, exist_ok=True)
    texte = ET.tostring(racine, encoding="unicode")
    entete = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        "<!-- Copie réduite de l'ISPDB de Thunderbird (" + DEPOT + "), licence MPL-2.0 (LICENSE).\n"
        "     Produite par scripts/ispdb.py maj : ne pas modifier à la main. -->\n"
    )
    (SORTIE / "ispdb.xml").write_text(entete + texte + "\n", encoding="utf-8")
    shutil.copyfile(dossier / "LICENSE", SORTIE / "LICENSE")
    AMORCES.mkdir(parents=True, exist_ok=True)
    for fichier in fichiers[:: max(1, len(fichiers) // 24)]:
        shutil.copyfile(fichier, AMORCES / fichier.name)
    taille = (SORTIE / "ispdb.xml").stat().st_size // 1024
    print(f"ISPDB : {gardes} fournisseurs gardés sur {len(fichiers)} fichiers, {taille} Kio, commit {commit[:12]}")


def age(jours: int) -> None:
    debut = (SORTIE / "ispdb.xml").read_text(encoding="utf-8")[:4096]
    repere = 'date="'
    if repere not in debut:
        sys.exit("ISPDB : date de la copie introuvable ; lancer scripts/ispdb.py maj")
    date = datetime.date.fromisoformat(debut.split(repere, 1)[1][:10])
    ecart = (datetime.date.today() - date).days
    if ecart > jours:
        sys.exit(f"ISPDB : copie du {date}, {ecart} jours (plus de {jours}) ; lancer scripts/ispdb.py maj")
    print(f"ISPDB : copie du {date} ({ecart} jours)")


def main() -> None:
    commande = sys.argv[1] if len(sys.argv) > 1 else ""
    if commande == "maj":
        maj(pathlib.Path(sys.argv[2]) if len(sys.argv) > 2 else SOURCE)
    elif commande == "age":
        age(int(sys.argv[2]) if len(sys.argv) > 2 else 60)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
