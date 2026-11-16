// Fenêtres indépendantes : lecture d'un message, rédaction.
// Dans Tauri : vraies fenêtres (WebviewWindow) et événements de l'application.
// Dans un navigateur (aperçu) : fenêtres surgissantes et BroadcastChannel.
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { api, runningInTauri } from "./api";
import { t } from "./i18n";
import type { ComposeInit } from "./compose";
import type { AttachmentInput, Draft, SendReport } from "./types";

/** Ce qu'une fenêtre secondaire signale à la fenêtre principale. */
export type WindowEvent =
  | { type: "sent"; report: SendReport; accountId: string; answeredId: number | null }
  | { type: "moved"; ids: number[] }
  | { type: "flags"; ids: number[] }
  | { type: "draft" }
  /** Message confié au backend, envoyé en arrière-plan (lot E1). */
  | { type: "sending"; accountId: string }
  /** Envoi en arrière-plan échoué : émis par le backend seul, avec le message à rouvrir. */
  | {
      type: "sendFailed";
      error: string;
      accountId: string;
      draft: Draft;
      attachments: AttachmentInput[];
      answeredId: number | null;
      draftId: number | null;
      /** Fenêtre de rédaction d'où vient le message (sa copie de secours). */
      window: string;
    };

const EVENT = "vortex://changed";
const channel = !runningInTauri && "BroadcastChannel" in window ? new BroadcastChannel("vortex") : null;

/**
 * Prévient la fenêtre principale. Dans Tauri, par le backend, qui vérifie la
 * forme de l'événement : les fenêtres secondaires ne peuvent pas en émettre
 * elles-mêmes (lot 9c).
 */
export async function notifyMain(event: WindowEvent): Promise<void> {
  if (runningInTauri) await api.notifyMain(event).catch(() => {});
  else channel?.postMessage(event);
}

/** Réglages que la fenêtre principale confie aux fenêtres qu'elle ouvre. */
const SHARED_KEYS = ["vortex.general", "vortex.mail", "vortex.theme"];

function settingsSnapshot(): string {
  const out: Record<string, string> = {};
  try {
    for (const key of SHARED_KEYS) {
      const value = localStorage.getItem(key);
      if (value !== null) out[key] = value;
    }
  } catch {
    // stockage indisponible : la fenêtre s'ouvrira avec les réglages par défaut
  }
  return JSON.stringify(out);
}

/**
 * Fenêtre secondaire, avant d'afficher quoi que ce soit : reprend les
 * réglages et le brouillon de départ confiés par la fenêtre qui l'a ouverte.
 * Elle a son propre stockage, séparé de celui de la fenêtre principale.
 */
export async function bootstrapSecondaryWindow(params: URLSearchParams): Promise<void> {
  if (!runningInTauri) return;
  const boot = await api.windowBootstrap();
  try {
    const settings = JSON.parse(boot.settings || "{}") as Record<string, unknown>;
    for (const key of SHARED_KEYS) {
      const value = settings[key];
      if (typeof value === "string") localStorage.setItem(key, value);
    }
    const key = params.get("key");
    if (key && boot.init) localStorage.setItem(`vortex.compose.${key}`, boot.init);
  } catch {
    // réglages illisibles : ceux par défaut
  }
}

export function onWindowEvent(handler: (event: WindowEvent) => void): () => void {
  if (runningInTauri) {
    const unlisten = listen<WindowEvent>(EVENT, (e) => handler(e.payload));
    return () => {
      unlisten.then((f) => f());
    };
  }
  const onMessage = (e: MessageEvent<WindowEvent>) => handler(e.data);
  channel?.addEventListener("message", onMessage);
  return () => channel?.removeEventListener("message", onMessage);
}

async function open(
  label: string,
  query: string,
  title: string,
  width: number,
  height: number,
  sources: number[] = [],
  init: string | null = null,
) {
  const url = `index.html?${query}`;
  if (!runningInTauri) {
    window.open(`/?${query}`, label, `popup,width=${width},height=${height}`);
    return;
  }
  // Stockage séparé : réglages et brouillon de départ passent par le backend.
  await api.openWindow(label, url, title, width, height, sources, init, settingsSnapshot());
}

/** Ouvre (ou ramène au premier plan) la fenêtre d'un message. */
export function openMessageWindow(id: number, subject: string): Promise<void> {
  return open(`message-${id}`, `view=message&id=${id}`, subject || t("Message"), 980, 760);
}

/**
 * Ouvre une fenêtre de rédaction ; le brouillon de départ passe par le
 * backend (stockage local dans l'aperçu navigateur).
 */
export function openComposeWindow(init: ComposeInit): Promise<void> {
  const key = `${Date.now()}-${init.key}`;
  if (!runningInTauri) {
    try {
      localStorage.setItem(`vortex.compose.${key}`, JSON.stringify(init));
    } catch {
      // stockage indisponible : la fenêtre s'ouvrira avec un message vide
    }
  }
  const title =
    init.mode === "new" ? t("Nouveau message") : init.subject ? init.subject : t("Répondre");
  // Seuls les messages transférés peuvent fournir des pièces jointes à cette
  // fenêtre : le backend refuse les autres (relecture du 10 novembre 2026).
  const sources = [...new Set(init.attachments.flatMap((a) => (a.messageId == null ? [] : [a.messageId])))];
  return open(
    `compose-${key.replace(/[^a-zA-Z0-9-]/g, "")}`,
    `view=compose&key=${encodeURIComponent(key)}`,
    title,
    1060,
    800,
    sources,
    JSON.stringify(init),
  );
}

/** Reprend un brouillon enregistré dans sa propre fenêtre de rédaction. */
export function openDraftWindow(id: number, subject: string): Promise<void> {
  return open(`compose-draft-${id}`, `view=compose&draft=${id}`, subject || t("Brouillon"), 1060, 800);
}

/**
 * Avant la fermeture de cette fenêtre (bouton ✕ du système) : `handler` est
 * attendu (ex. enregistrer le brouillon), puis la fenêtre se ferme.
 */
export function onCloseRequested(handler: () => Promise<void>): () => void {
  if (!runningInTauri) return () => {};
  const unlisten = getCurrentWebviewWindow().onCloseRequested(async () => {
    await handler();
  });
  return () => {
    unlisten.then((f) => f());
  };
}

/** Brouillon transmis à cette fenêtre de rédaction (lu une seule fois). */
/** Brouillons déjà repris par cette fenêtre (voir `takeComposeInit`). */
const taken = new Map<string, ComposeInit | null>();

/**
 * Reprend le brouillon confié par la fenêtre qui a ouvert celle-ci, et le
 * retire du stockage. Peut être appelé plusieurs fois pour la même clé : en
 * mode développement, React (StrictMode) exécute deux fois l'initialisation,
 * et le second appel trouvait le stockage déjà vidé — la réponse s'ouvrait
 * alors comme un message vide, sans destinataire ni citation.
 */
export function takeComposeInit(key: string): ComposeInit | null {
  if (taken.has(key)) return taken.get(key) ?? null;
  let init: ComposeInit | null = null;
  try {
    const raw = localStorage.getItem(`vortex.compose.${key}`);
    localStorage.removeItem(`vortex.compose.${key}`);
    init = raw ? (JSON.parse(raw) as ComposeInit) : null;
  } catch {
    init = null;
  }
  taken.set(key, init);
  return init;
}

export async function closeThisWindow(): Promise<void> {
  if (runningInTauri) await getCurrentWebviewWindow().close();
  else window.close();
}

export async function setThisWindowTitle(title: string): Promise<void> {
  if (runningInTauri) await getCurrentWebviewWindow().setTitle(title).catch(() => {});
  else document.title = title;
}

/**
 * Clic sur un lien réécrit du corps d'un message (lot 4b) : le backend le
 * renvoie à la fenêtre d'où il vient (`windows.rs`, événement `vortex-link`).
 */
export function onLinkRequest(handler: (url: string) => void): () => void {
  if (!runningInTauri) return () => {};
  const unlisten = getCurrentWebviewWindow().listen<string>("vortex-link", (e) => handler(e.payload));
  return () => {
    unlisten.then((f) => f());
  };
}
