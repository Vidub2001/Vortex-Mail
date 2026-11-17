import {
  FluentProvider,
  Link,
  Spinner,
  Toast,
  ToastBody,
  ToastTitle,
  Toaster,
  useId,
  useToastController,
} from "@fluentui/react-components";
import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { ChoiceDialog, ConfirmDialog, PromptDialog } from "./components/Dialogs";
import { ReconnectBar } from "./components/ReconnectBar";
import { listen } from "@tauri-apps/api/event";
import { api, runningInTauri } from "./api";
import { backups, clearBackup } from "./drafts";
import { useMailSettings } from "./settings";
import { useGeneral } from "./general";
import { useReminders } from "./reminders";
import { announceArrivals, newRule, runInboxRules, type Rule } from "./rules";
import { answerInvitation, applyInvitation, eventWhen } from "./invitation";
import { announceInvitations, newInvitations } from "./invites";
import { failedInit, newDraft, replyDraft } from "./compose";
import { onWindowEvent, openComposeWindow, openDraftWindow, openMessageWindow, type WindowEvent } from "./windows";
import { isUnified, members, mergeLists, unifiedFolder, unifiedId } from "./unified";
import { ActionBar } from "./components/ActionBar";
import { StatusBar } from "./components/StatusBar";
import { darkTheme, lightTheme, useThemeMode } from "./theme";
import { locale, t } from "./i18n";
import { FolderPane } from "./components/FolderPane";
import { MessageList, type FilterMode, type SortMode } from "./components/MessageList";
import { NavRail } from "./components/NavRail";
import { ReadingPane } from "./components/ReadingPane";
import type {
  Account,
  MailChanged,
  AttachmentInfo,
  Folder,
  FolderListing,
  FolderRole,
  Message,
  MessageSummary,
  MoveReport,
  SendReport,
} from "./types";

/** Erreur due à l'absence de réseau (et non à un compte mal configuré) ? */
function isNetworkError(message: string): boolean {
  return /» introuvable|” not found|” no encontrado|connexion à .* impossible|could not connect|no se puede conectar|connexion fermée|connection closed|conexión cerrada|unreachable|timed out|délai|network|error de red|résolution|failed to lookup|erreur réseau/i.test(
    message,
  );
}

/** État du réseau signalé par le système (NetworkManager). */
function useBrowserOnline(): boolean {
  const [online, setOnline] = useState(() => navigator.onLine);
  useEffect(() => {
    const on = () => setOnline(true);
    const off = () => setOnline(false);
    window.addEventListener("online", on);
    window.addEventListener("offline", off);
    return () => {
      window.removeEventListener("online", on);
      window.removeEventListener("offline", off);
    };
  }, []);
  return online;
}

/** Garde l'ancienne liste (même référence) si rien n'a changé : React ne redessine rien. */
function sameList(old: MessageSummary[], next: MessageSummary[]): MessageSummary[] {
  if (old.length !== next.length) return next;
  for (let i = 0; i < old.length; i++) {
    const a = old[i];
    const b = next[i];
    if (
      a.id !== b.id ||
      a.date !== b.date ||
      a.subject !== b.subject ||
      a.preview !== b.preview ||
      a.fromName !== b.fromName ||
      a.hasAttachments !== b.hasAttachments ||
      a.flags.seen !== b.flags.seen ||
      a.flags.flagged !== b.flags.flagged ||
      a.flags.answered !== b.flags.answered
    ) {
      return next;
    }
  }
  return old;
}

// Chargés à leur première ouverture (lot E6) : le démarrage n'attend ni
// l'agenda, ni les contacts, ni les paramètres.
const CalendarView = lazy(() => import("./components/CalendarView").then((m) => ({ default: m.CalendarView })));
const ContactsView = lazy(() => import("./components/ContactsView").then((m) => ({ default: m.ContactsView })));
const SettingsDialog = lazy(() => import("./components/SettingsDialog").then((m) => ({ default: m.SettingsDialog })));

/** Pendant le chargement d'une vue : un indicateur discret au milieu. */
function Loading() {
  return (
    <div className="view-loading">
      <Spinner size="small" />
    </div>
  );
}

export default function App() {
  const theme = useThemeMode();
  return (
    <FluentProvider theme={theme.dark ? darkTheme : lightTheme} className={theme.dark ? "provider dark" : "provider light"}>
      <Shell theme={theme} />
    </FluentProvider>
  );
}

function Shell({ theme }: { theme: ReturnType<typeof useThemeMode> }) {
  const toasterId = useId("toaster");
  const { dispatchToast } = useToastController(toasterId);

  const [accounts, setAccounts] = useState<Account[]>([]);
  /** Comptes connus, lus par les fonctions mémorisées (noms dans les messages d'erreur). */
  const accountsRef = useRef<Account[]>([]);
  accountsRef.current = accounts;
  const [folders, setFolders] = useState<Folder[]>([]);
  const [folderId, setFolderId] = useState<string | null>(null);
  const [messages, setMessages] = useState<MessageSummary[]>([]);
  const [syncingList, setSyncingList] = useState(false);
  const [selectedIds, setSelectedIds] = useState<number[]>([]);
  const [current, setCurrent] = useState<Message | null>(null);
  const [loadingMessage, setLoadingMessage] = useState(false);
  const [search, setSearch] = useState("");
  /** Recherche dans tous les dossiers : résultats affichés à la place du dossier. */
  const [searchResults, setSearchResults] = useState<MessageSummary[] | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [searching, setSearching] = useState(false);
  const searchSeq = useRef(0);
  /** Recherche lancée automatiquement après une pause dans la saisie. */
  const searchTimer = useRef<number | undefined>(undefined);
  /** Messages concernés par des actions faites hors connexion, en attente du réseau. */
  const [pendingActions, setPendingActions] = useState(0);
  /** Modifie la liste affichée, dossier et résultats de recherche. */
  const patchList = useCallback((fn: (list: MessageSummary[]) => MessageSummary[]) => {
    setMessages(fn);
    setSearchResults((list) => (list ? fn(list) : list));
  }, []);
  const [sort, setSort] = useState<SortMode>("date-desc");
  const [filter, setFilter] = useState<FilterMode>("all");
  const [draggingIds, setDraggingIds] = useState<number[]>([]);
  const [accountErrors, setAccountErrors] = useState<Record<string, string>>({});
  /** Aucun serveur joignable pour raison réseau : mode hors ligne. */
  const [networkDown, setNetworkDown] = useState(false);
  const browserOnline = useBrowserOnline();
  const offline = !browserOnline || networkDown;
  const offlineRef = useRef(offline);
  offlineRef.current = offline;

  /** Erreurs par compte ; si tous les comptes échouent faute de réseau, on passe hors ligne. */
  const applyErrors = useCallback((errors: { accountId: string; message: string }[], accountCount: number) => {
    setAccountErrors(Object.fromEntries(errors.map((e) => [e.accountId, e.message])));
    setNetworkDown(
      errors.length > 0 && errors.length >= accountCount && errors.every((e) => isNetworkError(e.message)),
    );
  }, []);
  const [foldersLoading, setFoldersLoading] = useState(true);
  const [settingsOpen, setSettingsOpen] = useState(false);
  /** Paramètres déjà ouverts une fois : la fenêtre reste montée ensuite, comme avant le lot E6. */
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  useEffect(() => {
    if (settingsOpen) setSettingsLoaded(true);
  }, [settingsOpen]);
  const anchorId = useRef<number | null>(null);
  /** Dossier affiché : les réponses arrivées pour un autre dossier sont ignorées. */
  const folderRef = useRef<string | null>(null);
  /** Vue des dossiers : unifiée (par défaut) ou comptes séparés ; retenue d'un lancement à l'autre. */
  const [unifiedView, setUnifiedView] = useState<boolean>(() => {
    try {
      return localStorage.getItem("vortex.unifiedView") !== "0";
    } catch {
      return true;
    }
  });
  const unifiedViewRef = useRef(unifiedView);
  unifiedViewRef.current = unifiedView;
  /** Dossiers connus, lus par les fonctions mémorisées (dossiers unifiés). */
  const foldersRef = useRef<Folder[]>([]);
  foldersRef.current = folders;

  const notify = useCallback(
    (title: string, body?: string, intent: "success" | "error" | "info" = "success") =>
      dispatchToast(
        <Toast>
          <ToastTitle>{title}</ToastTitle>
          {body && <ToastBody>{body}</ToastBody>}
        </Toast>,
        // Erreurs affichées plus longtemps : elles demandent souvent une action.
        { intent, timeout: intent === "error" ? 8000 : 6000 },
      ),
    [dispatchToast],
  );

  const fail = useCallback((e: unknown) => notify(t("Erreur"), String(e), "error"), [notify]);

  /** Fenêtre de saisie ou de confirmation en cours (dossiers). */
  const [dialog, setDialog] = useState<JSX.Element | null>(null);

  // Le menu du moteur web (Précédent, Recharger, Inspecter…) n'a rien à faire ici ;
  // il reste dans les champs de saisie pour copier-coller.
  useEffect(() => {
    const block = (e: MouseEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest("input, textarea, [contenteditable='true']")) return;
      e.preventDefault();
    };
    window.addEventListener("contextmenu", block);
    return () => window.removeEventListener("contextmenu", block);
  }, []);

  // ---------------------------------------------------------- largeur des colonnes

  const [widths, setWidths] = useState<{ folder: number; list: number }>(() => {
    try {
      const saved = JSON.parse(localStorage.getItem("vortex.paneWidths") ?? "null");
      if (saved && typeof saved.folder === "number" && typeof saved.list === "number") return saved;
    } catch {
      // valeurs par défaut
    }
    return { folder: 250, list: 400 };
  });

  useEffect(() => {
    try {
      localStorage.setItem("vortex.paneWidths", JSON.stringify(widths));
    } catch {
      // stockage indisponible : la largeur vaut pour cette session
    }
  }, [widths]);

  // Disposition (Paramètres › Général) : volet de lecture à droite, en bas ou masqué.
  const { layout, keepInTray } = useGeneral();

  // Zone de notification (lot B3) : réglage transmis au backend, qui cache la
  // fenêtre au lieu de la fermer ; « Nouveau message » depuis le menu de l'icône.
  useEffect(() => {
    api.setKeepInTray(keepInTray).catch(() => {
      // backend indisponible (aperçu navigateur) : sans objet
    });
  }, [keepInTray]);
  const [listHeight, setListHeight] = useState<number>(() => {
    try {
      const h = Number(localStorage.getItem("vortex.listHeight"));
      if (h >= 120) return h;
    } catch {
      // valeur par défaut
    }
    return 320;
  });

  useEffect(() => {
    try {
      localStorage.setItem("vortex.listHeight", String(listHeight));
    } catch {
      // stockage indisponible : la hauteur vaut pour cette session
    }
  }, [listHeight]);

  /** Glisser une séparation pour régler la largeur d'une colonne (ou la hauteur de la liste). */
  const startResize = (which: "folder" | "list" | "height") => (e: ReactMouseEvent) => {
    e.preventDefault();
    const vertical = which === "height";
    const origin = vertical ? e.clientY : e.clientX;
    const start = vertical ? listHeight : widths[which];
    const [min, max] = which === "folder" ? [160, 480] : which === "list" ? [260, 900] : [120, window.innerHeight - 200];
    const move = (ev: MouseEvent) => {
      const v = Math.round(Math.min(max, Math.max(min, start + (vertical ? ev.clientY : ev.clientX) - origin)));
      if (vertical) setListHeight(v);
      else setWidths((cur) => ({ ...cur, [which]: v }));
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      document.body.classList.remove("resizing", "resizing-rows");
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    document.body.classList.add("resizing");
    if (vertical) document.body.classList.add("resizing-rows");
  };

  /** Arborescence depuis le cache local : immédiat. */
  const loadCachedFolders = useCallback(async () => {
    try {
      const listing = await api.listFolders();
      setFolders(listing.folders);
      return listing.folders;
    } catch (e) {
      fail(e);
      return [];
    }
  }, [fail]);

  /** Synchronisation de l'arborescence avec les serveurs (réseau). */
  const reloadFolders = useCallback(async () => {
    setFoldersLoading(true);
    try {
      const listing = await api.syncFolders();
      setFolders(listing.folders);
      applyErrors(listing.errors, new Set(listing.folders.map((f) => f.accountId)).size);
      return listing.folders;
    } catch (e) {
      fail(e);
      return [];
    } finally {
      setFoldersLoading(false);
    }
  }, [fail]);

  /**
   * Liste d'un dossier : d'abord le cache (immédiat), puis la synchronisation.
   * Un dossier unifié réunit les listes de ses dossiers membres, triées par date.
   */
  const reloadMessages = useCallback(
    async (id: string) => {
      const sources = isUnified(id) ? members(id, foldersRef.current).map((f) => f.id) : [id];
      try {
        const cached = await Promise.all(sources.map((s) => api.listMessages(s)));
        if (folderRef.current === id) setMessages((old) => sameList(old, mergeLists(cached)));
      } catch (e) {
        fail(e);
      }
      setSyncingList(true);
      try {
        // Un seul appel : les comptes se synchronisent en parallèle côté application.
        let results: { messages: MessageSummary[]; error: string | null }[];
        try {
          results = await api.syncMessagesMany(sources);
        } catch (e) {
          results = await Promise.all(
            sources.map(async (s) => ({ messages: await api.listMessages(s).catch(() => []), error: String(e) })),
          );
        }
        if (folderRef.current !== id) return;
        // Liste inchangée : pas de nouveau rendu.
        setMessages((old) => sameList(old, mergeLists(results.map((r) => r.messages))));
        // Connexions à refaire : déjà signalées par le bandeau « Reconnecter », pas de doublon ici.
        // Seulement celles du courrier : un agenda à reconnecter ne masque pas les erreurs du courrier (A3).
        const pending = new Set(
          (await api.authPending().catch(() => [])).filter((p) => p.scope !== "pim").map((p) => p.user.toLowerCase()),
        );
        const errors = results.flatMap((r, i) => {
          if (!r.error) return [];
          const f = foldersRef.current.find((x) => x.id === sources[i]);
          const account = accountsRef.current.find((a) => a.id === f?.accountId);
          if (account && pending.has(account.email.trim().toLowerCase())) return [];
          const name = account?.name ?? f?.accountId ?? sources[i];
          return [sources.length > 1 ? t("{account} : {error}", { account: name, error: r.error }) : r.error];
        });
        // Hors ligne : la liste en cache suffit, pas d'alerte à chaque cycle.
        if (errors.length > 0 && !offlineRef.current && !errors.every(isNetworkError)) {
          notify(t("Synchronisation impossible"), errors.join("\n"), "error");
        }
      } finally {
        if (folderRef.current === id) setSyncingList(false);
      }
    },
    [fail, notify],
  );

  /**
   * Garde le dossier courant s'il existe encore. Sinon : la boîte de réception
   * unifiée s'il y a plusieurs comptes, la boîte de réception du compte sinon.
   */
  const pickFolder = useCallback((f: Folder[]) => {
    setFolderId((current) => {
      const keep = isUnified(current)
        ? unifiedViewRef.current && members(current, f).length > 0
        : !!current && f.some((x) => x.id === current);
      if (keep) return current;
      const inboxes = f.filter((x) => x.role === "inbox");
      if (unifiedViewRef.current && new Set(inboxes.map((x) => x.accountId)).size > 1) return unifiedId("inbox");
      return inboxes[0]?.id ?? f[0]?.id ?? null;
    });
  }, []);

  // Chargement initial : tout depuis le cache, puis synchronisation en arrière-plan.
  const reloadAll = useCallback(async () => {
    try {
      setAccounts(await api.listAccounts());
    } catch (e) {
      fail(e);
    }
    pickFolder(await loadCachedFolders());
    pickFolder(await reloadFolders());
  }, [fail, loadCachedFolders, reloadFolders, pickFolder]);

  useEffect(() => {
    reloadAll();
  }, [reloadAll]);

  useEffect(() => {
    folderRef.current = folderId;
    searchSeq.current++;
    setSearchResults(null);
    setSearchQuery("");
    setSearching(false);
    setSelectedIds([]);
    setCurrent(null);
    setMessages([]);
    setSyncingList(false);
    if (folderId) reloadMessages(folderId);
  }, [folderId, reloadMessages]);

  // ---------------------------------------------------------- hors connexion

  /** Messages restant à télécharger pour la lecture hors connexion (null : inconnu). */
  const [offlinePending, setOfflinePending] = useState<number | null>(null);
  const prefetching = useRef(false);

  /**
   * Télécharge en arrière-plan le contenu des messages (sauf corbeille et
   * indésirables), par petites étapes pour laisser la main à l'interface.
   * `resync` : resynchroniser aussi chaque dossier et compléter les plus anciens.
   */
  /**
   * Rejoue les actions faites hors connexion et reprend les déplacements
   * interrompus ; renvoie le nombre d'opérations menées à bien.
   */
  const flushPending = useCallback(async () => {
    try {
      const report = await api.flushPending();
      setPendingActions(report.remaining);
      return report.done;
    } catch {
      return 0;
    }
  }, []);

  const runPrefetch = useCallback(async (resync: boolean) => {
    if (prefetching.current) return;
    prefetching.current = true;
    try {
      for (;;) {
        const report = await api.prefetchStep(resync);
        setOfflinePending(report.remaining);
        if (report.done) break;
        await new Promise((r) => setTimeout(r, report.fetched > 0 ? 150 : 50));
      }
    } catch {
      // Hors ligne ou serveur indisponible : nouvelle tentative au prochain cycle.
    } finally {
      prefetching.current = false;
    }
  }, []);

  // Retour du réseau signalé par le système : on resynchronise tout de suite.
  useEffect(() => {
    const back = async () => {
      await flushPending();
      reloadFolders();
      if (folderRef.current) reloadMessages(folderRef.current);
      runPrefetch(false);
    };
    window.addEventListener("online", back);
    return () => window.removeEventListener("online", back);
  }, [flushPending, reloadFolders, reloadMessages, runPrefetch]);

  /** Règles de classement sur les nouveaux messages des boîtes de réception. */
  // `announce` : notifier les nouveaux messages restés dans les boîtes (lot B4) ;
  // pas au premier passage, qui découvre ce qui est arrivé pendant la fermeture.
  const runRulesRef = useRef<(announce?: boolean) => Promise<void>>(async () => {});
  runRulesRef.current = async (announce = true) => {
    const report = await runInboxRules(foldersRef.current);
    if (!report) return;
    if (announce) announceArrivals(report.arrivals, accountsRef.current);
    if (report.matched > 0) {
      reloadFolders();
      if (folderRef.current) reloadMessages(folderRef.current);
    }
    const failures = report.failed.filter((f) => !isNetworkError(f));
    if (failures.length > 0) notify(t("Règles : classement incomplet"), failures.slice(0, 3).join("\n"), "error");
  };

  // « Appliquer maintenant » (Paramètres › Règles) : listes et compteurs à jour.
  useEffect(() => {
    const refresh = () => {
      reloadFolders();
      if (folderRef.current) reloadMessages(folderRef.current);
    };
    window.addEventListener("vortex:rules-applied", refresh);
    return () => window.removeEventListener("vortex:rules-applied", refresh);
  }, [reloadFolders, reloadMessages]);

  // Premier tour complet peu après le démarrage, puis toutes les 15 minutes.
  useEffect(() => {
    const first = window.setTimeout(async () => {
      // Actions hors connexion et déplacements interrompus de la session précédente.
      await flushPending();
      await runRulesRef.current(false);
      runPrefetch(true);
      syncPimRef.current();
    }, 5_000);
    const full = window.setInterval(() => {
      runPrefetch(true);
      // Contacts et agenda : relus entièrement, donc moins souvent que le courrier.
      syncPimRef.current();
    }, 15 * 60_000);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(full);
    };
  }, [flushPending, runPrefetch]);

  // Synchronisation automatique (fréquence réglée dans Paramètres › Courrier), puis contenu des nouveaux messages.
  const { syncMinutes } = useMailSettings();
  useEffect(() => {
    const timer = window.setInterval(async () => {
      await flushPending();
      reloadFolders();
      if (folderRef.current) await reloadMessages(folderRef.current);
      await runRulesRef.current();
      runPrefetch(false);
    }, Math.max(1, syncMinutes) * 60_000);
    return () => window.clearInterval(timer);
  }, [flushPending, reloadFolders, reloadMessages, runPrefetch, syncMinutes]);

  // Ouvre le message quand un seul est sélectionné, et le marque comme lu.
  const single = selectedIds.length === 1 ? selectedIds[0] : null;
  useEffect(() => {
    if (single === null) {
      setCurrent(null);
      setLoadingMessage(false);
      return;
    }
    let cancelled = false;
    setLoadingMessage(true);
    api
      .getMessage(single)
      .then(async (m) => {
        if (cancelled) return;
        setCurrent(m);
        // Aperçu recalculé sur le message entier (le backend met aussi son cache à jour).
        patchList((list) => list.map((x) => (x.id === m.id && x.preview !== m.preview ? { ...x, preview: m.preview } : x)));
        if (!m.flags.seen) {
          // Affichage mis à jour tout de suite ; le serveur suit en arrière-plan.
          patchList((list) => list.map((x) => (x.id === m.id ? { ...x, flags: { ...x.flags, seen: true } } : x)));
          setFolders((fs) => fs.map((f) => (f.id === m.folderId ? { ...f, unread: Math.max(0, f.unread - 1) } : f)));
          api.setSeen([m.id], true).then(refreshPending, fail);
        }
      })
      .catch(fail)
      .finally(() => !cancelled && setLoadingMessage(false));
    return () => {
      cancelled = true;
    };
  }, [single, fail]);

  /** Liste affichée : recherche, filtre puis tri. */
  const visible = useMemo(() => {
    const q = search.trim().toLowerCase();
    // Résultats de recherche : déjà filtrés par les serveurs (le corps compris).
    let list = searchResults ?? (q
      ? messages.filter((m) =>
          [m.subject, m.fromName, m.fromEmail, m.preview].some((s) => s.toLowerCase().includes(q)),
        )
      : messages);
    if (filter === "unread") list = list.filter((m) => !m.flags.seen);
    else if (filter === "flagged") list = list.filter((m) => m.flags.flagged);
    else if (filter === "attachments") list = list.filter((m) => m.hasAttachments);
    const byText = (a: string, b: string) => a.localeCompare(b, locale(), { sensitivity: "base" });
    if (sort === "date-asc") list = [...list].sort((a, b) => a.date - b.date);
    else if (sort === "from") list = [...list].sort((a, b) => byText(a.fromName || a.fromEmail, b.fromName || b.fromEmail) || b.date - a.date);
    else if (sort === "subject") list = [...list].sort((a, b) => byText(a.subject, b.subject) || b.date - a.date);
    return list;
  }, [messages, searchResults, search, filter, sort]);

  const folderById = useMemo(() => new Map(folders.map((f) => [f.id, f])), [folders]);
  const currentFolder = isUnified(folderId) ? unifiedFolder(folderId, folders) : folderId ? folderById.get(folderId) : undefined;
  const currentAccount = accounts.find((a) => a.id === currentFolder?.accountId);
  const selectedMessages = visible.filter((m) => selectedIds.includes(m.id));

  const select = (id: number, mode: "single" | "toggle" | "range") => {
    if (mode === "toggle") {
      setSelectedIds((ids) => (ids.includes(id) ? ids.filter((x) => x !== id) : [...ids, id]));
      anchorId.current = id;
    } else if (mode === "range" && anchorId.current !== null) {
      const a = visible.findIndex((m) => m.id === anchorId.current);
      const b = visible.findIndex((m) => m.id === id);
      if (a < 0 || b < 0) return setSelectedIds([id]);
      const [from, to] = a < b ? [a, b] : [b, a];
      setSelectedIds(visible.slice(from, to + 1).map((m) => m.id));
    } else {
      setSelectedIds([id]);
      anchorId.current = id;
    }
  };

  const afterMove = async (report: MoveReport, where: string, ids: number[]) => {
    const n = report.moved.length;
    const queued = report.queued ?? 0;
    if (queued > 0) {
      notify(
        queued > 1
          ? t("{n} déplacements en attente", { n: queued })
          : t("{n} déplacement en attente", { n: queued }),
        t("Hors connexion : fait automatiquement au retour du réseau."),
        "info",
      );
      refreshPending();
    }
    const failedIds = new Set(report.failed.map((f) => f.id));
    setSearchResults((list) => (list ? list.filter((m) => !ids.includes(m.id) || failedIds.has(m.id)) : list));
    if (n > 0) {
      const detail =
        report.crossAccount > 0
          ? report.crossAccount > 1
            ? t("{n} copiés vers un autre compte puis supprimés de la source", { n: report.crossAccount })
            : t("{n} copié vers un autre compte puis supprimé de la source", { n: report.crossAccount })
          : t("Déplacement effectué sur le serveur");
      notify(
        n > 1 ? t("{n} éléments déplacés vers {where}", { n, where }) : t("{n} élément déplacé vers {where}", { n, where }),
        detail,
      );
    }
    if (report.failed.length > 0) {
      notify(t("{n} échec(s)", { n: report.failed.length }), report.failed.map((f) => f.reason).join("\n"), "error");
    }
    setSelectedIds([]);
    setCurrent(null);
    await reloadFolders();
    if (folderId) await reloadMessages(folderId);
  };

  const describeFolder = (id: string) => {
    const f = folderById.get(id);
    const a = accounts.find((x) => x.id === f?.accountId);
    return f ? t("« {folder} » ({account})", { folder: f.name, account: a?.name ?? f.accountId }) : id;
  };

  const moveTo = (targetId: string, ids: number[]) =>
    api.moveMessages(ids, targetId).then((r) => afterMove(r, describeFolder(targetId), ids)).catch(fail);

  const moveIdsToRole = (ids: number[], role: FolderRole, label: string) =>
    api.moveToRole(ids, role).then((r) => afterMove(r, label, ids)).catch(fail);

  const moveToRole = (role: FolderRole, label: string) => moveIdsToRole(selectedIds, role, label);

  // ---------------------------------------------------------- recherche dans tous les dossiers

  /** Entrée dans la recherche : d'abord le cache (immédiat), puis les serveurs. */
  const runSearch = async (raw: string) => {
    window.clearTimeout(searchTimer.current);
    const q = raw.trim();
    if (q.length < 2) return;
    const seq = ++searchSeq.current;
    setSearchQuery(q);
    setSearching(true);
    setSelectedIds([]);
    try {
      const local = await api.searchLocal(q);
      if (seq !== searchSeq.current) return;
      setSearchResults(local);
      const server = await api.searchMessages(q);
      if (seq !== searchSeq.current) return;
      const found = new Set(server.results.map((m) => m.id));
      setSearchResults([...server.results, ...local.filter((m) => !found.has(m.id))].sort((a, b) => b.date - a.date));
      if (server.errors.length > 0) {
        const network = server.errors.every((e) => isNetworkError(e.message));
        const name = (id: string) => accounts.find((a) => a.id === id)?.name ?? id;
        notify(
          network ? t("Recherche limitée à cet ordinateur") : t("Recherche incomplète"),
          network
            ? t("Serveur injoignable : seuls les messages enregistrés ici ont été parcourus.")
            : server.errors.map((e) => t("{account} : {error}", { account: name(e.accountId), error: e.message })).join("\n"),
          "info",
        );
      }
    } catch (e) {
      fail(e);
    } finally {
      if (seq === searchSeq.current) setSearching(false);
    }
  };

  const closeSearch = () => {
    searchSeq.current++;
    setSearchResults(null);
    setSearchQuery("");
    setSearching(false);
    setSearch("");
    setSelectedIds([]);
  };

  // ---------------------------------------------------------- actions hors connexion

  const refreshPending = () => {
    api.pendingCount().then(setPendingActions, () => {});
  };

  // ---------------------------------------------------------- actions au survol d'un message

  const quickSeen = async (m: MessageSummary) => {
    const seen = !m.flags.seen;
    await api.setSeen([m.id], seen).catch(fail);
    refreshPending();
    patchList((list) => list.map((x) => (x.id === m.id ? { ...x, flags: { ...x.flags, seen } } : x)));
    setFolders((fs) =>
      fs.map((f) => (f.id === m.folderId ? { ...f, unread: Math.max(0, f.unread + (seen ? -1 : 1)) } : f)),
    );
  };

  const quickFlag = async (m: MessageSummary) => {
    const flagged = !m.flags.flagged;
    await api.setFlagged([m.id], flagged).catch(fail);
    refreshPending();
    patchList((list) => list.map((x) => (x.id === m.id ? { ...x, flags: { ...x.flags, flagged } } : x)));
  };

  /**
   * Supprimer : les brouillons demandent confirmation (bouton de la barre,
   * poubelle au survol), les autres messages partent directement à la corbeille.
   */
  const deleteIds = (ids: number[]) => {
    const isDraft = (id: number) => {
      const m = (searchResults ?? messages).find((x) => x.id === id);
      return !!m && folderById.get(m.folderId)?.role === "drafts";
    };
    // Dans « Éléments supprimés » ou les indésirables : suppression
    // définitive après confirmation, comme Thunderbird ; le déplacement vers
    // la corbeille ne faisait rien, sans le dire (relecture complète n° 40).
    const inBin = (id: number) => {
      const m = (searchResults ?? messages).find((x) => x.id === id);
      const role = m ? folderById.get(m.folderId)?.role : undefined;
      return role === "trash" || role === "junk";
    };
    const final = ids.filter(inBin);
    if (final.length > 0) {
      const others = ids.filter((id) => !inBin(id));
      const n = final.length;
      setDialog(
        <ConfirmDialog
          title={n > 1 ? t("Supprimer définitivement {n} éléments ?", { n }) : t("Supprimer définitivement ?")}
          message={
            n > 1
              ? t("Ces {n} éléments seront supprimés définitivement du serveur. Cette action est irréversible.", { n })
              : t("Cet élément sera supprimé définitivement du serveur. Cette action est irréversible.")
          }
          confirm={t("Supprimer")}
          onConfirm={async () => {
            const removed = await api.deleteForever(final);
            notify(removed > 1 ? t("{n} éléments supprimés", { n: removed }) : t("{n} élément supprimé", { n: removed }));
            setSearchResults((list) => (list ? list.filter((m) => !final.includes(m.id)) : list));
            if (others.length > 0) await moveIdsToRole(others, "trash", t("Éléments supprimés"));
            setSelectedIds([]);
            setCurrent(null);
            await reloadFolders();
            if (folderId) await reloadMessages(folderId);
          }}
          onClose={() => setDialog(null)}
        />,
      );
      return;
    }
    const drafts = ids.filter(isDraft).length;
    if (drafts === 0) {
      moveIdsToRole(ids, "trash", t("Éléments supprimés"));
      return;
    }
    const n = ids.length;
    setDialog(
      <ConfirmDialog
        title={
          drafts === 1 && n === 1
            ? t("Supprimer le brouillon ?")
            : n > 1
              ? t("Supprimer {n} éléments ?", { n })
              : t("Supprimer {n} élément ?", { n })
        }
        message={
          drafts === n
            ? n > 1
              ? t("Ces {n} brouillons seront déplacés dans « Éléments supprimés ».", { n })
              : t("Ce brouillon sera déplacé dans « Éléments supprimés ».")
            : drafts > 1
              ? n - drafts > 1
                ? t("{drafts} brouillons et {others} autres messages seront déplacés dans « Éléments supprimés ».", { drafts, others: n - drafts })
                : t("{drafts} brouillons et {others} autre message seront déplacés dans « Éléments supprimés ».", { drafts, others: n - drafts })
              : n - drafts > 1
                ? t("{drafts} brouillon et {others} autres messages seront déplacés dans « Éléments supprimés ».", { drafts, others: n - drafts })
                : t("{drafts} brouillon et {others} autre message seront déplacés dans « Éléments supprimés ».", { drafts, others: n - drafts })
        }
        confirm={t("Supprimer")}
        onConfirm={async () => {
          await moveIdsToRole(ids, "trash", t("Éléments supprimés"));
        }}
        onClose={() => setDialog(null)}
      />,
    );
  };

  const quickDelete = (m: MessageSummary) => deleteIds([m.id]);
  /** Message (affiché) rangé dans « Éléments supprimés ». */
  const inTrash = (id: number) => {
    const m = (searchResults ?? messages).find((x) => x.id === id);
    return !!m && folderById.get(m.folderId)?.role === "trash";
  };

  /** Restaurer : chaque message retourne dans son dossier d'origine, sinon dans la boîte de réception. */
  const restoreIds = (ids: number[]) =>
    api
      .restoreMessages(ids)
      .then(async ({ report, toInbox }) => {
        const n = report.moved.length + (report.queued ?? 0);
        if (n > 0) {
          notify(
            n > 1 ? t("{n} éléments restaurés", { n }) : t("{n} élément restauré", { n }),
            toInbox > 0 ? t("{n} sans dossier d'origine connu : remis dans la boîte de réception", { n: toInbox }) : undefined,
          );
        }
        if ((report.queued ?? 0) > 0) refreshPending();
        if (report.failed.length > 0) {
          notify(t("{n} échec(s)", { n: report.failed.length }), report.failed.map((f) => f.reason).join("\n"), "error");
        }
        const failedIds = new Set(report.failed.map((f) => f.id));
        setSearchResults((list) => (list ? list.filter((m) => !ids.includes(m.id) || failedIds.has(m.id)) : list));
        setSelectedIds([]);
        setCurrent(null);
        await reloadFolders();
        if (folderId) await reloadMessages(folderId);
      })
      .catch(fail);

  /** Clic droit › Restaurer : comme Supprimer, toute la sélection qui contient le message. */
  const menuRestore = (m: MessageSummary) => restoreIds(selectedIds.includes(m.id) ? selectedIds : [m.id]);

  /** Clic droit › Supprimer : toute la sélection si le message en fait partie, sinon lui seul. */
  const menuDelete = (m: MessageSummary) => deleteIds(selectedIds.includes(m.id) ? selectedIds : [m.id]);

  /** Règle préremplie depuis un message (clic droit), ouverte dans Paramètres › Règles. */
  const [ruleDraft, setRuleDraft] = useState<Rule | null>(null);
  const createRule = (m: MessageSummary, from: "sender" | "subject") => {
    const rule = newRule(m.accountId);
    if (from === "sender") {
      rule.name = m.fromName || m.fromEmail;
      rule.conditions = [{ field: "from", op: "contains", value: m.fromEmail }];
    } else {
      // Objet sans « RE : », « TR : »… pour viser toute la conversation.
      const subject = m.subject.replace(/^\s*((re|tr|fw|fwd|aw|wg|sv)\s*:\s*)+/i, "").trim();
      rule.name = subject;
      rule.conditions = [{ field: "subject", op: "contains", value: subject }];
    }
    setRuleDraft(rule);
    setSettingsOpen(true);
  };

  const selectAll = (all: boolean) => {
    setSelectedIds(all ? visible.map((m) => m.id) : []);
    anchorId.current = null;
  };

  // ---------------------------------------------------------- barre de gauche

  /** Vue affichée : courrier, agenda ou contacts. */
  const [view, setView] = useState<"mail" | "calendar" | "contacts">("mail");
  /** Incrémenté à chaque synchronisation des contacts et de l'agenda. */
  const [pimVersion, setPimVersion] = useState(0);
  // Rappels des rendez-vous (notifications), d'après les agendas en cache.
  useReminders(pimVersion, notify);
  const [pimSyncing, setPimSyncing] = useState(false);
  const pimRunning = useRef(false);

  /** Relit carnets et agendas sur les serveurs (CardDAV/CalDAV). */
  const syncPim = async (manual = false) => {
    if (pimRunning.current) return;
    pimRunning.current = true;
    setPimSyncing(true);
    try {
      const report = await api.pimSync();
      // Invitations déposées dans l'agenda par le serveur (lot H1b) : signalées une fois.
      api
        .events()
        .then((events) => {
          const fresh = newInvitations(events, accounts.map((a) => a.email));
          announceInvitations(fresh, eventWhen);
          for (const e of fresh.slice(0, 3)) notify(t("Nouvelle invitation : {title}", { title: e.summary || t("(sans titre)") }), eventWhen(e));
        })
        .catch(() => {});
      const shown = report.errors.filter((e) => manual || !isNetworkError(e.message));
      if (shown.length > 0) {
        const name = (id: string) => accounts.find((a) => a.id === id)?.name ?? id;
        notify(t("Contacts et agenda"), shown.map((e) => t("{account} : {error}", { account: name(e.accountId), error: e.message })).join("\n"), "error");
      }
    } catch (e) {
      if (manual) fail(e);
    } finally {
      pimRunning.current = false;
      setPimSyncing(false);
      setPimVersion((v) => v + 1);
    }
  };
  const syncPimRef = useRef(syncPim);
  syncPimRef.current = syncPim;

  // Connexion ou déconnexion Google depuis les paramètres : relecture immédiate.
  useEffect(() => {
    const again = () => syncPimRef.current(true);
    window.addEventListener("vortex:pim-sync", again);
    return () => window.removeEventListener("vortex:pim-sync", again);
  }, []);

  const openView = (next: "calendar" | "contacts") => {
    setView(next);
    syncPimRef.current();
  };

  /** Compte affiché seul (clic sur son avatar dans le bandeau), sinon tous. */
  const [focusAccount, setFocusAccount] = useState<string | null>(null);

  const showUnified = () => {
    setView("mail");
    setFocusAccount(null);
    if (!unifiedView) changeUnifiedView(true);
    setSearch("");
    setFolderId(unifiedId("inbox"));
  };

  /** Vue du compte seul : ses dossiers uniquement, sans les dossiers unifiés. */
  const showAccount = (accountId: string) => {
    const inbox = folders.find((f) => f.accountId === accountId && f.role === "inbox");
    setSearch("");
    setView("mail");
    setFocusAccount(accountId);
    if (unifiedView) changeUnifiedView(false);
    if (inbox) setFolderId(inbox.id);
  };

  const allSeen = selectedMessages.length > 0 && selectedMessages.every((m) => m.flags.seen);
  const allFlagged = selectedMessages.length > 0 && selectedMessages.every((m) => m.flags.flagged);

  const toggleSeen = async () => {
    await api.setSeen(selectedIds, !allSeen).catch(fail);
    refreshPending();
    patchList((list) =>
      list.map((m) => (selectedIds.includes(m.id) ? { ...m, flags: { ...m.flags, seen: !allSeen } } : m)),
    );
    reloadFolders();
  };

  const toggleFlag = async () => {
    await api.setFlagged(selectedIds, !allFlagged).catch(fail);
    refreshPending();
    patchList((list) =>
      list.map((m) => (selectedIds.includes(m.id) ? { ...m, flags: { ...m.flags, flagged: !allFlagged } } : m)),
    );
  };

  // Touche Suppr : vers les éléments supprimés du compte de chaque message.
  // Seulement dans la vue Mail, sans paramètres ni fenêtre de dialogue ouverts :
  // la sélection de mails reste en mémoire dans l'agenda ou les contacts, et
  // Suppr ne doit pas l'envoyer à la corbeille à l'insu de l'utilisateur
  // (relecture F5). Même confirmation que le bouton Supprimer (brouillons).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (target.closest("input, textarea, [contenteditable], [role='dialog']")) return;
      if (view !== "mail" || settingsOpen || dialog !== null) return;
      if (e.key === "Delete" && selectedIds.length > 0) deleteIds(selectedIds);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const startDrag = (id: number) => {
    // On glisse toute la sélection si l'élément en fait partie, sinon lui seul.
    if (selectedIds.includes(id)) setDraggingIds(selectedIds);
    else {
      setSelectedIds([id]);
      setDraggingIds([id]);
    }
  };

  /** Bascule vue unifiée / comptes séparés, en gardant un dossier équivalent sélectionné. */
  const changeUnifiedView = (unified: boolean) => {
    setUnifiedView(unified);
    try {
      localStorage.setItem("vortex.unifiedView", unified ? "1" : "0");
    } catch {
      // stockage indisponible : le choix vaut pour cette session
    }
    if (!unified && isUnified(folderId)) {
      // Dossier unifié → le même dossier du premier compte.
      setFolderId(members(folderId, folders)[0]?.id ?? folders[0]?.id ?? null);
    } else if (unified && accounts.length > 1) {
      // Dossier spécial d'un compte → son dossier unifié.
      const f = folderId ? folderById.get(folderId) : undefined;
      if (f && f.role !== "custom" && !f.parentId) setFolderId(unifiedId(f.role));
    }
  };

  // ---------------------------------------------------------- dossiers (clic droit)

  const applyListing = (listing: FolderListing) => {
    setFolders(listing.folders);
    applyErrors(listing.errors, new Set(listing.folders.map((f) => f.accountId)).size);
    pickFolder(listing.folders);
  };

  const createFolder = (accountId: string, parent: Folder | null) => {
    const where = parent?.name ?? accounts.find((a) => a.id === accountId)?.name ?? accountId;
    setDialog(
      <PromptDialog
        title={parent ? t("Nouveau sous-dossier") : t("Nouveau dossier")}
        label={t("Nom du dossier, dans « {where} »", { where })}
        confirm={t("Créer")}
        onSubmit={async (name) => {
          applyListing(await api.createFolder(accountId, parent?.id ?? null, name));
          notify(t("Dossier créé"), name);
        }}
        onClose={() => setDialog(null)}
      />,
    );
  };

  const renameFolder = (f: Folder) =>
    setDialog(
      <PromptDialog
        title={t("Renommer le dossier")}
        label={t("Nouveau nom")}
        confirm={t("Renommer")}
        initial={f.name}
        onSubmit={async (name) => {
          if (name === f.name) return;
          const wasShown = folderRef.current === f.id;
          const listing = await api.renameFolder(f.id, name);
          applyListing(listing);
          if (wasShown) {
            const renamed = listing.folders.find((x) => x.accountId === f.accountId && x.parentId === f.parentId && x.name === name);
            if (renamed) setFolderId(renamed.id);
          }
          notify(t("Dossier renommé"), t("« {from} » → « {to} »", { from: f.name, to: name }));
        }}
        onClose={() => setDialog(null)}
      />,
    );

  const moveFolder = (f: Folder) => {
    const own = folders.filter((x) => x.accountId === f.accountId);
    // Ni lui-même, ni un de ses sous-dossiers, ni son parent actuel.
    const isUnder = (x: Folder): boolean => {
      let parent = x.parentId;
      while (parent) {
        if (parent === f.id) return true;
        parent = own.find((y) => y.id === parent)?.parentId ?? null;
      }
      return false;
    };
    const targets = own.filter((x) => x.id !== f.id && !isUnder(x) && x.id !== f.parentId);
    const choices = [
      ...(f.parentId ? [{ value: "", label: t("Au premier niveau du compte") }] : []),
      ...targets.map((x) => ({ value: x.id, label: `${"\u2003".repeat(x.depth)}${x.name}` })),
    ];
    if (choices.length === 0) {
      notify(t("Aucune destination possible"), f.name, "info");
      return;
    }
    setDialog(
      <ChoiceDialog
        title={t("Déplacer « {name} »", { name: f.name })}
        label={t("Dans le dossier")}
        confirm={t("Déplacer")}
        choices={choices}
        onSubmit={async (target) => {
          const listing = await api.moveFolder(f.id, target || null);
          applyListing(listing);
          if (folderRef.current === f.id) {
            const moved = listing.folders.find(
              (x) => x.accountId === f.accountId && x.name === f.name && x.parentId === (target || null),
            );
            if (moved) setFolderId(moved.id);
          }
          notify(
            t("Dossier déplacé"),
            target
              ? t("« {from} » → « {to} »", { from: f.name, to: folders.find((x) => x.id === target)?.name ?? "" })
              : t("« {from} » → « premier niveau »", { from: f.name }),
          );
        }}
        onClose={() => setDialog(null)}
      />,
    );
  };

  const deleteFolder = (f: Folder) =>
    setDialog(
      <ConfirmDialog
        title={t("Supprimer le dossier")}
        message={t("Le dossier « {name} » et tous ses messages ({total}) seront supprimés définitivement du serveur.", {
          name: f.name,
          total: f.total,
        })}
        confirm={t("Supprimer")}
        onConfirm={async () => {
          applyListing(await api.deleteFolder(f.id));
          notify(t("Dossier supprimé"), f.name);
        }}
        onClose={() => setDialog(null)}
      />,
    );

  const emptyFolders = (list: Folder[], label: string) => {
    const total = list.reduce((n, f) => n + f.total, 0);
    setDialog(
      <ConfirmDialog
        title={t("Vider « {label} »", { label })}
        message={
          list.length > 1
            ? total > 1
              ? t("{n} éléments seront supprimés définitivement dans {accounts} comptes. Cette action est irréversible.", { n: total, accounts: list.length })
              : t("{n} élément sera supprimé définitivement dans {accounts} comptes. Cette action est irréversible.", { n: total, accounts: list.length })
            : total > 1
              ? t("{n} éléments seront supprimés définitivement. Cette action est irréversible.", { n: total })
              : t("{n} élément sera supprimé définitivement. Cette action est irréversible.", { n: total })
        }
        confirm={t("Vider")}
        onConfirm={async () => {
          let removed = 0;
          for (const f of list) removed += await api.emptyFolder(f.id);
          notify(
            t("« {label} » vidé", { label }),
            removed > 1 ? t("{n} éléments supprimés", { n: removed }) : t("{n} élément supprimé", { n: removed }),
          );
          await reloadFolders();
          if (folderRef.current) reloadMessages(folderRef.current);
        }}
        onClose={() => setDialog(null)}
      />,
    );
  };

  // ---------------------------------------------------------- import et export (lot E8a)

  /** Ce message, ou toute la sélection s'il en fait partie, en `.eml` dans Téléchargements. */
  const exportMessages = async (m: MessageSummary) => {
    const ids = selectedIds.includes(m.id) ? selectedIds : [m.id];
    try {
      const paths = await api.exportEml(ids);
      notify(
        paths.length > 1 ? t("{n} messages enregistrés dans Téléchargements", { n: paths.length }) : t("Message enregistré dans Téléchargements"),
        paths.length === 1 ? paths[0] : undefined,
      );
    } catch (e) {
      notify(t("Enregistrement impossible"), String(e), "error");
    }
  };

  const exportFolder = async (f: Folder) => {
    notify(t("Export de « {folder} » en cours…", { folder: f.name }), undefined, "info");
    try {
      const { path, count } = await api.exportMbox(f.id);
      notify(count > 1 ? t("{n} messages exportés", { n: count }) : t("{n} message exporté", { n: count }), path);
    } catch (e) {
      notify(t("Export impossible"), String(e), "error");
    }
  };

  /** Fichier lu en base64, sans l'en-tête `data:…;base64,`. */
  const readBase64 = (file: File) =>
    new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ""));
      reader.onerror = () => reject(reader.error);
      reader.readAsDataURL(file);
    });

  const importMessages = (f: Folder) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.accept = ".eml,.mbox,.mbx,message/rfc822,application/mbox";
    input.onchange = async () => {
      const files = Array.from(input.files ?? []);
      if (files.length === 0) return;
      notify(t("Import dans « {folder} » en cours…", { folder: f.name }), undefined, "info");
      let imported = 0;
      let failed = 0;
      let error = "";
      for (const file of files) {
        try {
          const report = await api.importMessages(f.id, await readBase64(file));
          imported += report.imported;
          failed += report.failed;
          if (report.error) error = report.error;
        } catch (e) {
          failed += 1;
          error = `${file.name} : ${String(e)}`;
        }
      }
      if (failed > 0) {
        notify(t("Import incomplet : {ok} importés, {failed} refusés", { ok: imported, failed }), error, "error");
      } else {
        notify(imported > 1 ? t("{n} messages importés", { n: imported }) : t("{n} message importé", { n: imported }), f.name);
      }
      await reloadFolders();
      if (folderRef.current) reloadMessages(folderRef.current);
    };
    input.click();
  };

  // ---------------------------------------------------------- images distantes

  const showImages = async (m: Message) => {
    try {
      const updated = await api.showImages(m.id);
      setCurrent((cur) => (cur && cur.id === updated.id ? updated : cur));
    } catch (e) {
      notify(t("Images non affichées"), String(e), "error");
    }
  };

  /** Signature S/MIME examinée de nouveau (lot D1). */
  const checkSignature = async (m: Message, quiet: boolean) => {
    try {
      const updated = await api.checkSignature(m.id);
      setCurrent((cur) => (cur && cur.id === updated.id ? updated : cur));
    } catch (e) {
      if (!quiet) notify(t("Signature non vérifiée"), String(e), "error");
    }
  };

  // ---------------------------------------------------------- pièces jointes

  const openAttachment = async (m: Message, a: AttachmentInfo) => {
    try {
      await api.openAttachment(m.id, a.index);
    } catch (e) {
      notify(t("Ouverture impossible"), String(e), "error");
    }
  };

  const saveAttachments = async (m: Message, list: AttachmentInfo[]) => {
    const saved: string[] = [];
    for (const a of list) {
      try {
        saved.push(await api.saveAttachment(m.id, a.index));
      } catch (e) {
        notify(t("Échec : {name}", { name: a.name }), String(e), "error");
      }
    }
    if (saved.length === 1) notify(t("Pièce jointe enregistrée"), saved[0]);
    else if (saved.length > 1) {
      // Dossier du premier fichier, séparateur « / » ou « \ » (relecture complète n° 49).
      const dir = saved[0].slice(0, Math.max(saved[0].lastIndexOf("/"), saved[0].lastIndexOf("\\")));
      notify(t("{n} pièces jointes enregistrées", { n: saved.length }), dir);
    }
  };

  // ---------------------------------------------------------- rédaction

  const startCompose = () => {
    if (accounts.length === 0) {
      notify(t("Aucun compte"), t("Ajoutez d'abord un compte dans les paramètres."), "info");
      return;
    }
    openComposeWindow(newDraft(currentAccount?.id ?? accounts[0].id));
  };

  const startComposeRef = useRef(startCompose);
  startComposeRef.current = startCompose;
  useEffect(() => {
    if (!runningInTauri) return;
    const unlisten = listen("tray-compose", () => startComposeRef.current());
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Alertes des serveurs IMAP (`[ALERT]` : quota, maintenance) : montrées
  // telles quelles, une fois par compte (relecture complète n° 22).
  const alertRef = useRef(notify);
  alertRef.current = notify;
  useEffect(() => {
    if (!runningInTauri) return;
    const unlisten = listen<{ account: string; text: string }>("server-alert", (e) =>
      alertRef.current(t("Message du serveur ({account})", { account: e.payload.account }), e.payload.text, "info"),
    );
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const startReply = (mode: "reply" | "replyAll" | "forward") => {
    if (current) openComposeWindow(replyDraft(mode, current, accounts));
  };

  /** Message envoyé depuis une fenêtre de rédaction. */
  const afterSend = (report: SendReport, accountId: string, answeredId: number | null) => {
    const n = report.recipients;
    notify(
      t("Message envoyé"),
      report.warning ??
        (report.savedIn
          ? n > 1
            ? t("{n} destinataires — copie dans « {folder} »", { n, folder: report.savedIn })
            : t("{n} destinataire — copie dans « {folder} »", { n, folder: report.savedIn })
          : n > 1
            ? t("{n} destinataires", { n })
            : t("{n} destinataire", { n })),
      report.warning ? "info" : "success",
    );
    if (answeredId !== null) {
      const mark = <T extends MessageSummary>(m: T): T =>
        m.id === answeredId ? { ...m, flags: { ...m.flags, answered: true } } : m;
      patchList((list) => list.map(mark));
      setCurrent((m) => (m ? mark(m) : m));
    }
    // Le dossier « Éléments envoyés » affiché se met à jour tout de suite.
    const sent = folders.find((f) => f.accountId === accountId && f.role === "sent");
    reloadFolders();
    const shown = folderRef.current;
    if (shown && (shown === sent?.id || shown === unifiedId("sent"))) reloadMessages(shown);
  };

  /** Ce que signalent les fenêtres secondaires (envoi, déplacement, indicateurs). */
  const windowEventRef = useRef<(e: WindowEvent) => void>(() => {});
  windowEventRef.current = (e: WindowEvent) => {
    if (e.type === "sent") {
      afterSend(e.report, e.accountId, e.answeredId);
      return;
    }
    // Envoi en arrière-plan (lot E1) : la rédaction est déjà fermée.
    if (e.type === "sending") {
      notify(t("Envoi en cours…"), undefined, "info");
      return;
    }
    if (e.type === "sendFailed") {
      notify(t("Message non envoyé"), t("{error} — le message est rouvert pour un nouvel essai.", { error: e.error }), "error");
      // La copie de secours de l'ancienne fenêtre n'est retirée qu'une fois la
      // nouvelle ouverte : celle-ci écrit aussitôt la sienne.
      openComposeWindow(failedInit(e.accountId, e.draft, e.attachments, e.answeredId, e.draftId, e.error))
        .then(() => api.clearDraftBackup(e.window))
        .catch(() => {
          // fenêtre non ouverte : la copie de secours reste proposée au prochain démarrage
        });
      return;
    }
    if (e.type === "draft") {
      // Brouillon enregistré depuis une fenêtre de rédaction : « Brouillons » affiché à jour.
      const shown = folderRef.current;
      const role = shown && (isUnified(shown) ? unifiedFolder(shown, foldersRef.current)?.role : foldersRef.current.find((f) => f.id === shown)?.role);
      if (shown && role === "drafts") reloadMessages(shown);
      // Compteurs de « Brouillons » (par compte et unifié) à jour tout de suite.
      reloadFolders();
      return;
    }
    if (e.type === "moved") {
      setSelectedIds((ids) => ids.filter((x) => !e.ids.includes(x)));
      setCurrent((m) => (m && e.ids.includes(m.id) ? null : m));
      setSearchResults((list) => (list ? list.filter((m) => !e.ids.includes(m.id)) : list));
    } else if (current && e.ids.includes(current.id)) {
      api.getMessage(current.id).then((m) => setCurrent((cur) => (cur && cur.id === m.id ? m : cur)), () => {});
    }
    refreshPending();
    reloadFolders();
    if (folderRef.current) reloadMessages(folderRef.current);
  };
  useEffect(() => onWindowEvent((e) => windowEventRef.current(e)), []);

  // Nouveaux messages signalés en temps réel par un serveur (IMAP IDLE).
  const mailChangedRef = useRef<(e: MailChanged) => void>(() => {});
  mailChangedRef.current = (e: MailChanged) => {
    reloadFolders();
    const shown = folderRef.current;
    if (shown && (shown === e.folderId || shown === unifiedId("inbox"))) reloadMessages(shown);
    void runRulesRef.current().then(() => runPrefetch(false));
  };
  useEffect(() => {
    if (!runningInTauri) return;
    const unlisten = listen<MailChanged>("mail-changed", (ev) => mailChangedRef.current(ev.payload));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Brouillons jamais arrivés sur le serveur (hors connexion à la fermeture) : proposés à la reprise.
  useEffect(() => {
    const timer = window.setTimeout(async () => {
      for (const [key, b] of await backups()) {
        const title = b.draft.subject || t("(sans objet)");
        dispatchToast(
          <Toast>
            <ToastTitle
              action={
                <Link
                  onClick={() => {
                    // L'ancienne copie n'est retirée qu'une fois la fenêtre
                    // ouverte : celle-ci écrit aussitôt la sienne puis enregistre
                    // sur le serveur (relecture complète n° 33).
                    void openComposeWindow({
                      key: Date.now(),
                      mode: "draft",
                      accountId: b.accountId,
                      to: b.draft.to.join("; "),
                      cc: b.draft.cc.join("; "),
                      bcc: b.draft.bcc.join("; "),
                      subject: b.draft.subject,
                      html: b.draft.html,
                      inReplyTo: b.draft.inReplyTo,
                      references: b.draft.references,
                      answeredId: null,
                      attachments: [],
                      // Numéro de la session précédente : il désigne peut-être un
                      // autre message maintenant (numéros attribués à chaque
                      // lancement). Un nouveau brouillon est enregistré ; l'ancien,
                      // s'il avait atteint le serveur, reste dans « Brouillons ».
                      draftId: null,
                      plainText: b.draft.plainText,
                      recovered: true,
                    })
                      .then(() => clearBackup(key))
                      .catch(() => {
                        // fenêtre non ouverte : la copie reste proposée au prochain démarrage
                      });
                  }}
                >
                  {t("Rouvrir")}
                </Link>
              }
            >
              {t("Brouillon non enregistré retrouvé")}
            </ToastTitle>
            <ToastBody>{t("« {title} » — sans ses pièces jointes.", { title })}</ToastBody>
          </Toast>,
          { intent: "info", timeout: -1 },
        );
      }
    }, 1500);
    return () => window.clearTimeout(timer);
  }, [dispatchToast]);

  const openMessage = selectedIds.length === 1 ? current : null;

  return (
    <div className="shell">
      <ReconnectBar
        notify={notify}
        onReconnected={() => {
          reloadFolders();
          if (folderRef.current) reloadMessages(folderRef.current);
          window.dispatchEvent(new Event("vortex:pim-sync"));
        }}
      />
      <NavRail
        accounts={accounts}
        activeAccountId={isUnified(folderId) ? null : (currentFolder?.accountId ?? null)}
        unifiedActive={isUnified(folderId)}
        dark={theme.dark}
        onUnified={showUnified}
        onAccount={showAccount}
        onToggleTheme={() => theme.setMode(theme.dark ? "light" : "dark")}
        onSettings={() => setSettingsOpen(true)}
        view={view}
        onView={openView}
      />
      {view === "calendar" && (
        <main className="main">
          <Suspense fallback={<Loading />}>
            <CalendarView accounts={accounts} notify={notify} version={pimVersion} syncing={pimSyncing} onSync={() => syncPim(true)} />
          </Suspense>
        </main>
      )}
      {view === "contacts" && (
        <main className="main">
          <Suspense fallback={<Loading />}>
            <ContactsView
              accounts={accounts}
              notify={notify}
              version={pimVersion}
              onWrite={(email, name) => {
                if (accounts.length === 0) return;
                openComposeWindow({ ...newDraft(accounts[0].id), to: name && name !== email ? `${name} <${email}>` : email });
              }}
            />
          </Suspense>
        </main>
      )}
      <main className="main" hidden={view !== "mail"}>
        <div
          className={`panes layout-${layout}`}
          style={
            layout === "right"
              ? { gridTemplateColumns: `${widths.folder}px 5px minmax(260px, ${widths.list}px) 5px minmax(320px, 1fr)` }
              : layout === "bottom"
                ? {
                    gridTemplateColumns: `${widths.folder}px 5px minmax(320px, 1fr)`,
                    gridTemplateRows: `minmax(120px, ${listHeight}px) 5px minmax(160px, 1fr)`,
                  }
                : { gridTemplateColumns: `${widths.folder}px 5px minmax(320px, 1fr)` }
          }
        >
          <FolderPane
            accounts={focusAccount ? accounts.filter((a) => a.id === focusAccount) : accounts}
            folders={focusAccount ? folders.filter((f) => f.accountId === focusAccount) : folders}
            selectedFolderId={folderId}
            draggingIds={draggingIds}
            accountErrors={accountErrors}
            loading={foldersLoading}
            unifiedView={unifiedView}
            onUnifiedViewChange={(unified) => {
              setFocusAccount(null);
              changeUnifiedView(unified);
            }}
            onRetry={reloadFolders}
            onAddAccount={() => setSettingsOpen(true)}
            onCompose={startCompose}
            onSelect={(id) => {
              setSearch("");
              setFolderId(id);
            }}
            onDropMessages={(target, ids) => {
              setDraggingIds([]);
              moveTo(target, ids);
            }}
            onCreateFolder={createFolder}
            onDeleteFolder={deleteFolder}
            onRenameFolder={renameFolder}
            onMoveFolder={moveFolder}
            onEmptyFolders={emptyFolders}
            onExportFolder={exportFolder}
            onImportMessages={importMessages}
            offline={offline}
          />
          <div
            className="splitter"
            role="separator"
            aria-orientation="vertical"
            title={t("Glisser pour régler la largeur (double-clic : par défaut)")}
            onMouseDown={startResize("folder")}
            onDoubleClick={() => setWidths((w) => ({ ...w, folder: 250 }))}
          />
          <MessageList
            folder={currentFolder}
            account={currentAccount}
            accounts={accounts}
            messages={visible}
            selectedIds={selectedIds}
            loading={syncingList}
            search={search}
            onSearch={(value) => {
              setSearch(value);
              window.clearTimeout(searchTimer.current);
              if (value.trim() === "") {
                if (searchResults !== null || searching) closeSearch();
                return;
              }
              // Recherche complète (contenu compris, tous les dossiers) dès que la saisie se pose.
              if (value.trim().length >= 3) {
                searchTimer.current = window.setTimeout(() => runSearch(value), 700);
              }
            }}
            sort={sort}
            onSort={setSort}
            filter={filter}
            onFilter={setFilter}
            onSelectAll={selectAll}
            onSelect={select}
            onOpen={(m) => {
              const isDraft = folderById.get(m.folderId)?.role === "drafts";
              if (isDraft) openDraftWindow(m.id, m.subject);
              else openMessageWindow(m.id, m.subject);
            }}
            onSearchSubmit={runSearch}
            searchMode={searchResults !== null || searching ? { query: searchQuery, searching } : null}
            onCloseSearch={closeSearch}
            folderLabel={(id) => {
              const f = folderById.get(id);
              const a = accounts.find((x) => x.id === f?.accountId);
              return f ? (accounts.length > 1 ? `${f.name} · ${a?.name ?? ""}` : f.name) : "";
            }}
            onDragStart={startDrag}
            onDragEnd={() => setDraggingIds([])}
            onQuickSeen={quickSeen}
            onQuickFlag={quickFlag}
            onQuickDelete={quickDelete}
            onMenuDelete={menuDelete}
            onMenuRestore={menuRestore}
            restorable={(m) => inTrash(m.id)}
            onCreateRule={createRule}
            onExport={exportMessages}
          />
          {layout !== "hidden" && (
          <div
            className={layout === "bottom" ? "splitter splitter-rows" : "splitter"}
            role="separator"
            aria-orientation={layout === "bottom" ? "horizontal" : "vertical"}
            title={
              layout === "bottom"
                ? t("Glisser pour régler la hauteur (double-clic : par défaut)")
                : t("Glisser pour régler la largeur (double-clic : par défaut)")
            }
            onMouseDown={startResize(layout === "bottom" ? "height" : "list")}
            onDoubleClick={() =>
              layout === "bottom" ? setListHeight(320) : setWidths((w) => ({ ...w, list: 400 }))
            }
          />
          )}
          {layout !== "hidden" && (
          <ReadingPane
              message={current}
              loading={loadingMessage && !current}
              selectionCount={selectedIds.length}
              dark={theme.dark}
              toolbar={
                <ActionBar
                  accounts={accounts}
                  folders={folders}
                  canReply={openMessage !== null}
                  selectionCount={selectedIds.length}
                  allSeen={allSeen}
                  allFlagged={allFlagged}
                  onReply={startReply}
                  onArchive={() => moveToRole("archive", t("Archive"))}
                  onDelete={() => deleteIds(selectedIds)}
                  onRestore={selectedIds.length > 0 && selectedIds.every(inTrash) ? () => restoreIds(selectedIds) : undefined}
                  onMove={(id) => moveTo(id, selectedIds)}
                  onToggleSeen={toggleSeen}
                  onToggleFlag={toggleFlag}
                  onPopOut={
                    openMessage
                      ? () =>
                          folderById.get(openMessage.folderId)?.role === "drafts"
                            ? openDraftWindow(openMessage.id, openMessage.subject)
                            : openMessageWindow(openMessage.id, openMessage.subject)
                      : undefined
                  }
                />
              }
              onOpenAttachment={openAttachment}
              onSaveAttachments={saveAttachments}
              onShowImages={showImages}
              onCheckSignature={checkSignature}
              onAddInvitation={async (m, a) => {
                try {
                  const { done, title } = await applyInvitation(m, a);
                  notify(done, title);
                  setPimVersion((v) => v + 1);
                } catch (e) {
                  notify(t("Agenda non modifié"), String(e), "error");
                }
              }}
              onAnswerInvitation={async (m, a, answer, send, asAccount) => {
                try {
                  const { done, title } = await answerInvitation(m, a, answer, send, asAccount);
                  notify(done, title);
                  setPimVersion((v) => v + 1);
                } catch (e) {
                  notify(t("Réponse non envoyée"), String(e), "error");
                }
              }}
            />
          )}
        </div>
      </main>
      <StatusBar
        offline={offline}
        syncing={foldersLoading || syncingList}
        offlinePending={offlinePending}
        pendingActions={pendingActions}
        accountCount={accounts.length}
      />
      {(settingsOpen || settingsLoaded) && (
        <Suspense fallback={null}>
          <SettingsDialog
            themeMode={theme.mode}
            onThemeMode={theme.setMode}
            open={settingsOpen}
            ruleDraft={ruleDraft}
            onClose={() => {
              setSettingsOpen(false);
              setRuleDraft(null);
            }}
            accounts={accounts}
            onChanged={reloadAll}
            notify={notify}
          />
        </Suspense>
      )}
      {dialog}
      <Toaster toasterId={toasterId} position="bottom-end" />
    </div>
  );
}
