// Petites opérations sur du texte, en temps linéaire (lot L2) : elles
// remplacent des expressions régulières qui revenaient en arrière caractère
// par caractère (`/[).,;]+$/` sur « )))))…x » essaie chaque position, soit un
// temps quadratique sur un texte venu d'un mail).

/**
 * Identifiant aléatoire en hexadécimal (`bytes` octets), tiré par le générateur
 * du système : jamais `Math.random`, prévisible (lot L2, alerte SonarQube S2245).
 */
export function randomId(bytes = 16): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (b) => b.toString(16).padStart(2, "0")).join("");
}

/** `s` sans les caractères de `chars` qui le terminent. */
export function trimEndOf(s: string, chars: string): string {
  let end = s.length;
  while (end > 0 && chars.includes(s[end - 1])) end--;
  return s.slice(0, end);
}

/** Forme d'une adresse (`x@y.z`), sans espace : contrôle de saisie, pas une validation complète. */
export function isPlausibleEmail(address: string): boolean {
  if (/\s/.test(address)) return false;
  const at = address.indexOf("@");
  if (at <= 0 || at !== address.lastIndexOf("@")) return false;
  const domain = address.slice(at + 1);
  const dot = domain.lastIndexOf(".");
  return dot > 0 && dot < domain.length - 1;
}

/** Adresse entre chevrons à la fin d'une saisie (`Nom <a@b.fr>`), sinon `null`. */
export function angleAddress(part: string): string | null {
  const s = part.trimEnd();
  if (!s.endsWith(">")) return null;
  const open = s.lastIndexOf("<");
  if (open < 0) return null;
  const inner = s.slice(open + 1, -1);
  return inner && !inner.includes(">") ? inner : null;
}
