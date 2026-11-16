// Nom d'une couleur en clair, pour les pastilles de couleur (comptes,
// agendas, texte) : un lecteur d'écran annonçait « #c62828 » (relecture
// complète n° 35).
import { t } from "./i18n";

/** « Rouge », « Bleu foncé », « Gris »… d'après la teinte et la clarté. */
export function colorName(color: string): string {
  if (color === "transparent") return t("Aucune");
  const m = /^#?([0-9a-f]{6})$/i.exec(color.trim());
  if (!m) return color;
  const n = parseInt(m[1], 16);
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => v / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const d = max - min;
  const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
  if (s < 0.15) {
    if (l < 0.15) return t("Noir");
    if (l > 0.9) return t("Blanc");
    return l < 0.45 ? t("Gris foncé") : t("Gris");
  }
  let h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  h = (h * 60 + 360) % 360;
  const base =
    h < 15 || h >= 345
      ? t("Rouge")
      : h < 45
        ? t("Orange")
        : h < 70
          ? t("Jaune")
          : h < 160
            ? t("Vert")
            : h < 200
              ? t("Turquoise")
              : h < 255
                ? t("Bleu")
                : h < 290
                  ? t("Violet")
                  : t("Rose");
  if (l < 0.3) return t("{color} foncé", { color: base });
  if (l > 0.75) return t("{color} clair", { color: base });
  return base;
}
