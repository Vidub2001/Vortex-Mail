import type { MessageSummary } from "./types";
import { formatDate, formatTime } from "./general";
import { t, locale } from "./i18n";

const weekday = new Intl.DateTimeFormat(locale(), { weekday: "short" });
const longWeekday = new Intl.DateTimeFormat(locale(), { weekday: "long" });

function startOfDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** Date affichée dans la liste : heure, jour + heure, ou date. */
export function listDate(ts: number): string {
  const d = new Date(ts * 1000);
  const today = startOfDay(new Date());
  const day = startOfDay(d);
  if (day === today) return formatTime(d);
  if (today - day < 6 * 86_400_000) return `${weekday.format(d)} ${formatTime(d)}`;
  return formatDate(d);
}

/** Date seule (validité d'un certificat). */
export function dayDate(ts: number): string {
  return formatDate(new Date(ts * 1000));
}

export function longDate(ts: number): string {
  const d = new Date(ts * 1000);
  const s = `${longWeekday.format(d)} ${formatDate(d)} ${formatTime(d)}`;
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export interface DateGroup {
  label: string;
  items: MessageSummary[];
}

/** Regroupe une liste déjà triée (récent → ancien) par tranche de dates. */
export function groupByDate(list: MessageSummary[], now: Date = new Date()): DateGroup[] {
  const today = startOfDay(now);
  // Minuit d'il y a `n` jours, d'après les dates du calendrier : la veille
  // d'un changement d'heure dure 23 ou 25 h (relecture F15).
  const daysAgo = (n: number) => {
    const d = new Date(today);
    d.setDate(d.getDate() - n);
    return d.getTime();
  };
  const label = (ts: number) => {
    const d = startOfDay(new Date(ts * 1000));
    if (d >= today) return t("Aujourd'hui");
    if (d >= daysAgo(1)) return t("Hier");
    if (d >= daysAgo(6)) return t("Cette semaine");
    if (d >= daysAgo(13)) return t("La semaine dernière");
    return t("Plus ancien");
  };
  const groups: DateGroup[] = [];
  for (const m of list) {
    const l = label(m.date);
    const last = groups[groups.length - 1];
    if (last && last.label === l) last.items.push(m);
    else groups.push({ label: l, items: [m] });
  }
  return groups;
}

/** Taille lisible : « 812 o », « 18 Ko », « 2,4 Mo ». */
export function fileSize(bytes: number): string {
  if (bytes < 1024) return t("{n} o", { n: bytes });
  if (bytes < 1024 * 1024) return t("{n} Ko", { n: Math.round(bytes / 1024) });
  return t("{n} Mo", { n: (bytes / (1024 * 1024)).toLocaleString(locale(), { maximumFractionDigits: 1 }) });
}

/** Images distantes dans le HTML (src, background, url() des styles) ? */
export function hasRemoteImages(html: string): boolean {
  return (
    /(?:^|[\s"'])(?:src|background)\s*=\s*["']?\s*(?:https?:)?\/\//i.test(html) ||
    /url\(\s*["']?\s*(?:https?:)?\/\//i.test(html)
  );
}

/**
 * Corps HTML encapsulé pour l'iframe : pas de script. Images distantes
 * toujours bloquées par la CSP : celles que l'utilisateur autorise sont
 * téléchargées par le backend (jamais vers le réseau local) et intégrées en
 * `data:` ; ce qui n'a pas été intégré (`srcset`, `poster`, adresse locale…)
 * reste bloqué (relecture complète n° 20). `allowRemoteImages` ne change
 * plus la CSP ; il reste pour les appelants.
 * Pré-résolution DNS coupée, comme dans Thunderbird : sinon les adresses des
 * liens seraient résolues à l'affichage, et le mail se signalerait ouvert.
 */
export function wrapBody(html: string, allowRemoteImages = false, dark = false): string {
  void allowRemoteImages;
  const img = "data: cid:";
  // Thème sombre : fond et texte par défaut inversés ; les couleurs explicites du message restent.
  const colors = dark
    ? "color: #e4e6eb; background: #1d1e22; color-scheme: dark;"
    : "color: #242424; background: #fff;";
  const link = dark ? "#6cb1ef" : "#1a76c7";
  return `<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src ${img};">
<meta http-equiv="x-dns-prefetch-control" content="off">
<base target="_blank">
<style>
  body { font-family: "Segoe UI", system-ui, -apple-system, "Noto Sans", sans-serif; font-size: 14px;
         line-height: 1.5; ${colors} margin: 0; padding: 4px 2px 24px; overflow-wrap: anywhere; }
  a { color: ${link}; }
  img { max-width: 100%; height: auto; }
</style></head><body>${html}</body></html>`;
}

/**
 * Corps en texte brut pour l'iframe (mode d'affichage « Texte ») : tout est
 * échappé, aucune balise du message n'est interprétée.
 */
export function wrapText(text: string, dark = false): string {
  const escaped = text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const body = `<div style="white-space: pre-wrap; font-family: inherit;">${escaped}</div>`;
  return wrapBody(body, false, dark);
}
