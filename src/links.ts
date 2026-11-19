// Liens du corps des messages (lot 4b, relecture S6) : jamais ouverts dans le
// volet de lecture, toujours dans le navigateur externe, avec une alerte quand
// le texte affiché annonce un autre site que l'adresse réelle.
//
// Le corps est affiché dans une iframe sans script ; le moteur web de Linux
// n'y déclenche aucun gestionnaire d'événement, même posé par la fenêtre
// parente. Les liens sont donc réécrits au chargement (`target="_blank"`,
// adresse réelle en info-bulle) et le clic arrive au backend comme une
// demande de nouvelle fenêtre (`on_new_window`, `windows.rs`), toujours
// refusée : `http(s)` part dans le navigateur, les liens à traiter par
// l'interface (lien trompeur, `mailto:`) passent par l'hôte réservé
// `LINK_HOST` et reviennent à la fenêtre.
//
// Module sans autre dépendance que `text.ts` : testé par `node --test tests/frontend/`.
import { trimEndOf } from "./text";

/** Hôte réservé des liens renvoyés à l'interface ; même valeur que `windows.rs`. */
export const LINK_HOST = "link.vortex.invalid";

/** Extensions de fichier courantes : « rapport.pdf » n'est pas un site. */
const FILE_EXTENSIONS = new Set([
  "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "txt", "csv", "rtf",
  "zip", "rar", "jpg", "jpeg", "png", "gif", "svg", "webp", "html", "htm", "eml", "ics", "vcf",
  "exe", "msi", "mp3", "mp4", "mov",
]);

/** Nom d'hôte normalisé (minuscules, Punycode, sans « www. »), ou `null`. */
function normalizeHost(host: string): string | null {
  try {
    return new URL(`http://${host}`).hostname.replace(/^www\./, "");
  } catch {
    return null;
  }
}

/**
 * Site annoncé par le texte d'un lien, si ce texte ressemble à une adresse
 * (« https://banque.fr/compte », « www.banque.fr », « banque.fr ») ; sinon `null`.
 */
export function shownHost(text: string): string | null {
  // Caractères qui ne dessinent rien (U+200B, trait d'union conditionnel,
  // marques de sens d'écriture…) retirés, formes équivalentes ramenées à la
  // même (NFKC), point final ignoré : sinon « www.banque\u200b.fr » ne
  // ressemblait plus à une adresse, et l'alerte se taisait (relecture du
  // 10 novembre 2026).
  const s = trimEndOf(
    text
      .normalize("NFKC")
      // Sélecteurs de variante (U+FE00 à U+FE0F et suivants) par leur propriété :
      // écrits en intervalle après U+3164, ils se combinaient avec lui (lot L2).
      .replace(/[\p{Cf}\p{Variation_Selector}\u034F\u115F\u1160\u3164\uFFA0]/gu, "")
      .trim()
      .toLowerCase(),
    ".",
  );
  // Plus long qu'une adresse réelle : pas une adresse, et l'expression
  // ci-dessous n'est jamais appliquée à un texte démesuré (lot L2).
  if (s.length > 2048) return null;
  const m =
    /^(?:([a-z][a-z0-9+.-]*):\/\/)?(?:[^\s/@:]+@)?((?:[\p{L}\p{N}-]+\.)+([\p{L}]{2,}|xn--[a-z0-9-]+))(?::\d+)?(?:[/?#]\S*)?$/u.exec(s);
  if (!m) return null;
  const [, scheme, host, tld] = m;
  // Sans « http:// » ni « www. », un nom de fichier n'est pas une adresse.
  if (!scheme && !host.startsWith("www.") && FILE_EXTENSIONS.has(tld)) return null;
  return normalizeHost(host);
}

/**
 * Le texte du lien annonce un autre site que celui où il mène : renvoie le
 * site annoncé (pour l'alerte), sinon `null`. Un sous-domaine du site annoncé
 * est accepté (« banque.fr » → « www.client.banque.fr ») ; l'inverse non
 * (« banque.example » affiché, « example » réel : c'est l'attaque).
 */
export function misleadingHost(text: string, url: URL): string | null {
  const shown = shownHost(text);
  if (!shown) return null;
  const real = normalizeHost(url.hostname);
  if (real === null) return shown;
  if (real === shown || real.endsWith(`.${shown}`)) return null;
  return shown;
}

/** Contenu d'un lien `mailto:` (RFC 6068). */
export interface Mailto {
  to: string;
  cc: string;
  subject: string;
  body: string;
}

/** Texte de corps repris d'un lien : borné, une page de texte suffit. */
const MAX_MAILTO_BODY = 10_000;

/**
 * Destinataires, objet et texte d'un lien `mailto:` : le chemin et `to=`,
 * `cc`, `subject`, `body` (RFC 6068 §2 ; relecture complète n° 23). `bcc`
 * est ignoré : un message ne doit pas ajouter de copie cachée à l'insu de
 * l'utilisateur ; `body` devient du texte, jamais du HTML.
 */
export function parseMailto(url: URL): Mailto {
  const decode = (s: string) => {
    try {
      return decodeURIComponent(s);
    } catch {
      return s;
    }
  };
  const oneLine = (s: string) => s.replace(/[\r\n]/g, " ").trim();
  const list = (values: string[]) => values.map(oneLine).filter(Boolean).join(", ");
  const q = url.searchParams;
  const to = list([decode(url.pathname), ...q.getAll("to")]);
  const cc = list(q.getAll("cc"));
  const subject = oneLine(q.get("subject") ?? "");
  const body = (q.get("body") ?? "").replace(/\r\n/g, "\n").slice(0, MAX_MAILTO_BODY);
  return { to, cc, subject, body };
}

/** Ce que doit faire un clic sur ce lien. */
export type LinkAction =
  | { kind: "open"; url: string }
  | { kind: "confirm"; url: string; shown: string; real: string }
  | ({ kind: "mailto" } & Mailto)
  | { kind: "ignore" };

/** Décide du sort d'un lien à partir de son `href` brut et de son texte. */
export function linkAction(href: string, text: string): LinkAction {
  let url: URL;
  try {
    url = new URL(href.trim());
  } catch {
    return { kind: "ignore" };
  }
  if (url.protocol === "mailto:") return { kind: "mailto", ...parseMailto(url) };
  if (url.protocol !== "http:" && url.protocol !== "https:") return { kind: "ignore" };
  // Un message ne fabrique pas lui-même un lien interne.
  if (url.hostname === LINK_HOST) return { kind: "ignore" };
  const shown = misleadingHost(text, url);
  return shown ? { kind: "confirm", url: url.href, shown, real: url.hostname } : { kind: "open", url: url.href };
}

/**
 * Adresse à mettre dans le lien affiché (`null` : lien désactivé) et texte
 * de son info-bulle (l'adresse réelle).
 */
export function rewriteLink(action: LinkAction): { href: string; title: string } | null {
  const internal = (path: string, params: Record<string, string>) =>
    `https://${LINK_HOST}/${path}?${new URLSearchParams(params).toString()}`;
  switch (action.kind) {
    case "open":
      return { href: action.url, title: action.url };
    case "confirm":
      return { href: internal("suspect", { url: action.url, shown: action.shown }), title: action.url };
    case "mailto":
      return {
        href: internal("mailto", { to: action.to, cc: action.cc, subject: action.subject, body: action.body }),
        title: `mailto:${action.to}`,
      };
    default:
      return null;
  }
}

/** Lien réécrit, renvoyé par le backend après un clic : ce qu'il faut faire. */
export function linkRequest(raw: string): LinkAction {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return { kind: "ignore" };
  }
  if (url.hostname !== LINK_HOST) return { kind: "ignore" };
  const q = url.searchParams;
  if (url.pathname === "/mailto") {
    return { kind: "mailto", to: q.get("to") ?? "", cc: q.get("cc") ?? "", subject: q.get("subject") ?? "", body: q.get("body") ?? "" };
  }
  if (url.pathname === "/suspect") {
    try {
      const target = new URL(q.get("url") ?? "");
      if (target.protocol === "http:" || target.protocol === "https:") {
        return { kind: "confirm", url: target.href, shown: q.get("shown") ?? "", real: target.hostname };
      }
    } catch {
      // adresse illisible : rien à faire
    }
  }
  return { kind: "ignore" };
}

const XLINK = "http://www.w3.org/1999/xlink";

/** Réécrit les liens d'un corps de message affiché (voir l'en-tête du module). */
export function prepareLinks(doc: Document): void {
  // Tout élément qui porte une adresse (`href`, `xlink:href`) : les liens SVG,
  // et les éléments MathML qu'un moteur web rend cliquables (relecture
  // complète n° 21). Seuls `a` et `area` restent des liens, réécrits ; les
  // autres perdent leur adresse.
  for (const a of doc.querySelectorAll<Element>("[*|href]")) {
    const href = a.getAttribute("href") ?? a.getAttributeNS(XLINK, "href");
    if (href === null) continue;
    if (a.localName !== "a" && a.localName !== "area") {
      a.removeAttribute("href");
      a.removeAttributeNS(XLINK, "href");
      continue;
    }
    a.removeAttributeNS(XLINK, "href");
    const link = rewriteLink(linkAction(href, a.textContent ?? ""));
    if (!link) {
      a.removeAttribute("href");
      a.removeAttribute("target");
      continue;
    }
    a.setAttribute("href", link.href);
    a.setAttribute("target", "_blank");
    a.setAttribute("rel", "noopener noreferrer");
    a.setAttribute("title", link.title);
  }
}
