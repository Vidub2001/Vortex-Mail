import {
  Avatar,
  Checkbox,
  Menu,
  MenuItemRadio,
  MenuList,
  MenuPopover,
  MenuTrigger,
  SearchBox,
  Spinner,
  Text,
  ToolbarButton,
} from "@fluentui/react-components";
import {
  ArrowSortDownLines16Regular,
  Attach16Regular,
  Delete16Regular,
  Dismiss16Regular,
  Filter16Regular,
  Flag16Filled,
  Flag16Regular,
  MailRead16Regular,
  MailUnread16Regular,
  Open16Regular,
  Save16Regular,
} from "@fluentui/react-icons";
import { memo, useEffect, useMemo, useRef, useState } from "react";
import { groupByDate, listDate } from "../format";
import { useGeneral } from "../general";
import { ContextMenu, type ContextMenuItem } from "./ContextMenu";
import type { Account, Folder, MessageSummary } from "../types";
import { isUnified } from "../unified";
import { t } from "../i18n";

export type SortMode = "date-desc" | "date-asc" | "from" | "subject";
export type FilterMode = "all" | "unread" | "flagged" | "attachments";

const SORTS: { value: SortMode; label: string }[] = [
  { value: "date-desc", label: t("Date (récents d'abord)") },
  { value: "date-asc", label: t("Date (anciens d'abord)") },
  { value: "from", label: t("Expéditeur") },
  { value: "subject", label: t("Objet") },
];

const FILTERS: { value: FilterMode; label: string }[] = [
  { value: "all", label: t("Tous") },
  { value: "unread", label: t("Non lus") },
  { value: "flagged", label: t("Avec indicateur") },
  { value: "attachments", label: t("Avec pièces jointes") },
];

interface Props {
  folder: Folder | undefined;
  account: Account | undefined;
  /** Tous les comptes : couleur de chaque message dans un dossier unifié. */
  accounts: Account[];
  messages: MessageSummary[];
  selectedIds: number[];
  loading: boolean;
  search: string;
  onSearch: (value: string) => void;
  sort: SortMode;
  onSort: (sort: SortMode) => void;
  filter: FilterMode;
  onFilter: (filter: FilterMode) => void;
  onSelectAll: (all: boolean) => void;
  onSelect: (id: number, mode: "single" | "toggle" | "range") => void;
  /** Double-clic : ouvre le message dans sa propre fenêtre. */
  onOpen: (m: MessageSummary) => void;
  onDragStart: (id: number) => void;
  onDragEnd: () => void;
  /** Actions au survol d'un message. */
  onQuickSeen: (m: MessageSummary) => void;
  onQuickFlag: (m: MessageSummary) => void;
  onQuickDelete: (m: MessageSummary) => void;
  /** Clic droit › Créer une règle (expéditeur ou objet du message). */
  onCreateRule: (m: MessageSummary, from: "sender" | "subject") => void;
  /** Enregistrer en `.eml` (ce message, ou la sélection qui le contient : lot E8a). */
  onExport: (m: MessageSummary) => void;
  /** Entrée dans la recherche : recherche dans tous les dossiers de tous les comptes. */
  onSearchSubmit: (query: string) => void;
  /** Résultats de recherche affichés à la place du dossier. */
  searchMode: { query: string; searching: boolean } | null;
  onCloseSearch: () => void;
  /** Nom du dossier d'un message (affiché dans les résultats de recherche). */
  folderLabel: (folderId: string) => string;
}

/**
 * Remplace l'image de glisser par défaut (tout le bloc du message, opaque)
 * par une petite étiquette semi-transparente placée en bas à droite du curseur,
 * comme les clients mail courants : le dossier visé reste visible sous la souris.
 */
function setDragGhost(dt: DataTransfer, label: string) {
  const ghost = document.createElement("div");
  ghost.className = "drag-ghost";
  ghost.textContent = `✉  ${label}`;
  document.body.appendChild(ghost);
  dt.setDragImage(ghost, -14, -10);
  setTimeout(() => ghost.remove(), 0);
}

const PAGE = 150;

/** Identifiant de la ligne d'un message (`aria-activedescendant`). */
const rowId = (id: number) => `msg-row-${id}`;

interface RowActions {
  select: (id: number, mode: "single" | "toggle" | "range") => void;
  open: (m: MessageSummary) => void;
  dragStart: (id: number) => void;
  dragEnd: () => void;
  quickSeen: (m: MessageSummary) => void;
  quickFlag: (m: MessageSummary) => void;
  quickDelete: (m: MessageSummary) => void;
  menu: (m: MessageSummary, x: number, y: number) => void;
}

/** Une ligne de la liste ; mémorisée pour ne pas tout redessiner à chaque clic. */
const Row = memo(function Row({
  m,
  selected,
  active,
  selectionSize,
  ownerColor,
  ownerName,
  folderText,
  actions,
}: {
  /** Formats de date/heure : change quand le réglage change, pour redessiner. */
  stamp: string;
  m: MessageSummary;
  selected: boolean;
  /** Ligne active au clavier (`aria-activedescendant` de la liste). */
  active: boolean;
  selectionSize: number;
  ownerColor?: string;
  ownerName?: string;
  folderText?: string;
  actions: RowActions;
}) {
  const classes = ["msg", !m.flags.seen && "unread", selected && "selected", active && "active"].filter(Boolean).join(" ");
  return (
    <div
      id={rowId(m.id)}
      role="option"
      aria-selected={selected}
      className={classes}
      draggable
      onDragStart={(e) => {
        // WebKitGTK exige des données pour démarrer le glisser.
        e.dataTransfer.setData("text/plain", m.subject);
        e.dataTransfer.effectAllowed = "move";
        const count = selected ? selectionSize : 1;
        setDragGhost(e.dataTransfer, count > 1 ? t("{n} éléments", { n: count }) : m.subject);
        actions.dragStart(m.id);
      }}
      onDragEnd={actions.dragEnd}
      onClick={(e) => actions.select(m.id, e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : "single")}
      onDoubleClick={() => actions.open(m)}
      onContextMenu={(e) => {
        e.preventDefault();
        actions.menu(m, e.clientX, e.clientY);
      }}
    >
      <Avatar className="msg-avatar" name={m.fromName || m.fromEmail} size={36} color="colorful" />
      <div className="msg-body">
        <div className="msg-line">
          {ownerColor !== undefined && (
            <span className="account-dot msg-account-dot" style={{ background: ownerColor }} title={ownerName} />
          )}
          <span className="msg-from">{m.fromName || m.fromEmail}</span>
          <span className="msg-meta">
            {m.hasAttachments && <Attach16Regular />}
            {m.flags.flagged && <Flag16Filled className="flag" />}
            <span className="msg-date">{listDate(m.date)}</span>
          </span>
          {/* Actions au survol, à la place de la date. */}
          <span className="msg-actions" onClick={(e) => e.stopPropagation()}>
            <button
              type="button"
              title={m.flags.seen ? t("Marquer comme non lu") : t("Marquer comme lu")}
              onClick={() => actions.quickSeen(m)}
            >
              {m.flags.seen ? <MailUnread16Regular /> : <MailRead16Regular />}
            </button>
            <button
              type="button"
              className={m.flags.flagged ? "on" : undefined}
              title={m.flags.flagged ? t("Retirer l'indicateur") : t("Indicateur")}
              onClick={() => actions.quickFlag(m)}
            >
              {m.flags.flagged ? <Flag16Filled /> : <Flag16Regular />}
            </button>
            <button type="button" className="danger" title={t("Supprimer")} onClick={() => actions.quickDelete(m)}>
              <Delete16Regular />
            </button>
          </span>
        </div>
        <div className="msg-subject">
          {folderText !== undefined && <span className="msg-folder">{folderText}</span>}
          {m.subject || t("(sans objet)")}
        </div>
        <div className="msg-preview">{m.preview}</div>
      </div>
    </div>
  );
});

export function MessageList(p: Props) {
  const general = useGeneral();
  const stamp = `${general.timeFormat}|${general.dateFormat}`;
  const grouped = p.sort === "date-desc";
  // Affichage progressif : les premières lignes tout de suite, la suite en défilant.
  const [limit, setLimit] = useState(PAGE);
  const listKey = `${p.folder?.id ?? ""}|${p.searchMode?.query ?? ""}`;
  useEffect(() => setLimit(PAGE), [listKey]);
  const shown = useMemo(() => p.messages.slice(0, limit), [p.messages, limit]);
  const groups = useMemo(
    () => (grouped ? groupByDate(shown) : [{ label: "", items: shown }]),
    [grouped, shown],
  );
  const selectedSet = useMemo(() => new Set(p.selectedIds), [p.selectedIds]);
  /**
   * Ligne active au clavier (relecture complète n° 35) : la liste reçoit le
   * focus (Tab), les flèches déplacent la sélection (Maj : l'étendent),
   * Entrée ouvre le message dans une fenêtre, la touche Menu ou Maj+F10
   * ouvre son menu. Par défaut, la dernière ligne sélectionnée.
   */
  const [activeId, setActiveId] = useState<number | null>(null);
  const active = activeId !== null && p.messages.some((m) => m.id === activeId) ? activeId : (p.selectedIds[p.selectedIds.length - 1] ?? null);
  const order = useMemo(() => groups.flatMap((g) => g.items), [groups]);
  // Gestionnaires stables : une ligne ne se redessine que si son message ou sa sélection change.
  const live = useRef(p);
  live.current = p;
  const actions = useMemo<RowActions>(
    () => ({
      select: (id, mode) => {
        setActiveId(id);
        live.current.onSelect(id, mode);
      },
      open: (m) => live.current.onOpen(m),
      dragStart: (id) => live.current.onDragStart(id),
      dragEnd: () => live.current.onDragEnd(),
      quickSeen: (m) => live.current.onQuickSeen(m),
      quickFlag: (m) => live.current.onQuickFlag(m),
      quickDelete: (m) => live.current.onQuickDelete(m),
      menu: (m, x, y) => setMenu({ m, x, y }),
    }),
    [],
  );
  const [menu, setMenu] = useState<{ m: MessageSummary; x: number; y: number } | null>(null);
  const menuItems = (m: MessageSummary): ContextMenuItem[] => {
    const sender = m.fromName || m.fromEmail;
    const subject = m.subject.length > 40 ? `${m.subject.slice(0, 40)}…` : m.subject;
    return [
      { label: t("Ouvrir dans une fenêtre"), icon: <Open16Regular />, onClick: () => p.onOpen(m) },
      {
        label: m.flags.seen ? t("Marquer comme non lu") : t("Marquer comme lu"),
        icon: m.flags.seen ? <MailUnread16Regular /> : <MailRead16Regular />,
        onClick: () => p.onQuickSeen(m),
      },
      {
        label: m.flags.flagged ? t("Retirer l'indicateur") : t("Ajouter un indicateur"),
        icon: m.flags.flagged ? <Flag16Filled /> : <Flag16Regular />,
        onClick: () => p.onQuickFlag(m),
      },
      {
        label: t("Créer une règle : messages de {sender}…", { sender }),
        icon: <Filter16Regular />,
        disabled: !m.fromEmail,
        onClick: () => p.onCreateRule(m, "sender"),
      },
      {
        label: t("Créer une règle : objet « {subject} »…", { subject }),
        icon: <Filter16Regular />,
        disabled: !m.subject.trim(),
        onClick: () => p.onCreateRule(m, "subject"),
      },
      { label: t("Enregistrer (.eml)"), icon: <Save16Regular />, onClick: () => p.onExport(m) },
      { label: t("Supprimer"), icon: <Delete16Regular />, danger: true, onClick: () => p.onQuickDelete(m) },
    ];
  };
  const searching = p.searchMode !== null;
  const unified = isUnified(p.folder?.id) || searching;
  const accountById = new Map(p.accounts.map((a) => [a.id, a]));
  const allSelected = p.messages.length > 0 && p.selectedIds.length >= p.messages.length && p.messages.every((m) => selectedSet.has(m.id));
  const someSelected = p.selectedIds.length > 0 && !allSelected;
  const filtering = p.filter !== "all" || p.search.trim() !== "";

  return (
    <section className="pane message-list" aria-label={t("Messages")}>
      <div className="list-tools">
        <Checkbox
          checked={allSelected ? true : someSelected ? "mixed" : false}
          onChange={(_, d) => p.onSelectAll(d.checked === true)}
          aria-label={t("Tout sélectionner")}
          title={t("Tout sélectionner")}
          disabled={p.messages.length === 0}
        />
        <SearchBox
          className="list-search"
          size="small"
          placeholder={t("Rechercher partout")}
          title={t("Cherche dans tous les dossiers de tous les comptes, contenu des messages compris. Critères : de:nom, à:nom, objet:mot, avant:16/11/2026, après:01/11/2026, est:nonlu, est:lu, est:suivi, avec:pj, \"phrase exacte\", -mot pour exclure.")}
          value={p.search}
          onChange={(_, d) => p.onSearch(d.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") p.onSearchSubmit(p.search);
            if (e.key === "Escape" && searching) p.onCloseSearch();
          }}
        />
        <Menu
          checkedValues={{ sort: [p.sort] }}
          onCheckedValueChange={(_, d) => p.onSort(d.checkedItems[0] as SortMode)}
        >
          <MenuTrigger disableButtonEnhancement>
            <ToolbarButton className={p.sort !== "date-desc" ? "list-tool on" : "list-tool"} icon={<ArrowSortDownLines16Regular />}>
              {t("Trier")}
            </ToolbarButton>
          </MenuTrigger>
          <MenuPopover>
            <MenuList>
              {SORTS.map((s) => (
                <MenuItemRadio key={s.value} name="sort" value={s.value}>
                  {s.label}
                </MenuItemRadio>
              ))}
            </MenuList>
          </MenuPopover>
        </Menu>
        <Menu
          checkedValues={{ filter: [p.filter] }}
          onCheckedValueChange={(_, d) => p.onFilter(d.checkedItems[0] as FilterMode)}
        >
          <MenuTrigger disableButtonEnhancement>
            <ToolbarButton className={p.filter !== "all" ? "list-tool on" : "list-tool"} icon={<Filter16Regular />}>
              {t("Filtrer")}
            </ToolbarButton>
          </MenuTrigger>
          <MenuPopover>
            <MenuList>
              {FILTERS.map((f) => (
                <MenuItemRadio key={f.value} name="filter" value={f.value}>
                  {f.label}
                </MenuItemRadio>
              ))}
            </MenuList>
          </MenuPopover>
        </Menu>
      </div>

      {p.searchMode ? (
        <div className="list-header search-header">
          <Text weight="semibold" size={400} truncate wrap={false}>
            {t("Résultats pour « {query} »", { query: p.searchMode.query })}
          </Text>
          <span className="list-account">
            {p.searchMode.searching
              ? t("Recherche sur les serveurs…")
              : p.messages.length > 1
                ? t("{n} trouvés", { n: p.messages.length })
                : t("{n} trouvé", { n: p.messages.length })}
          </span>
          {p.searchMode.searching && <Spinner className="list-sync" size="extra-tiny" aria-label={t("Recherche")} />}
          <button type="button" className="search-close" onClick={p.onCloseSearch} title={t("Revenir au dossier (Échap)")}>
            <Dismiss16Regular />
          </button>
        </div>
      ) : (
      <div className="list-header">
        <Text weight="semibold" size={400} truncate wrap={false}>
          {p.folder?.name ?? ""}
        </Text>
        {p.account && (
          <span className="list-account">
            <span className="account-dot" style={{ background: p.account.color }} />
            {p.account.name}
          </span>
        )}
        {unified && (
          <span className="list-account">
            <span className="unified-dots">
              {p.accounts.slice(0, 3).map((a) => (
                <span key={a.id} className="account-dot" style={{ background: a.color }} />
              ))}
            </span>
            {t("Tous les comptes")}
          </span>
        )}
      </div>
      )}

      <div
        className="list-scroll"
        role="listbox"
        aria-multiselectable="true"
        aria-label={t("Messages")}
        tabIndex={0}
        aria-activedescendant={active !== null ? rowId(active) : undefined}
        onKeyDown={(e) => {
          const index = active === null ? -1 : order.findIndex((m) => m.id === active);
          const go = (i: number) => {
            const m = order[Math.max(0, Math.min(order.length - 1, i))];
            if (!m) return;
            e.preventDefault();
            actions.select(m.id, e.shiftKey ? "range" : "single");
            document.getElementById(rowId(m.id))?.scrollIntoView({ block: "nearest" });
            // Fin des lignes affichées : la suite est ajoutée.
            if (i >= order.length - 5 && limit < p.messages.length) setLimit((l) => l + PAGE);
          };
          const current = index >= 0 ? order[index] : undefined;
          switch (e.key) {
            case "ArrowDown":
              go(index + 1);
              break;
            case "ArrowUp":
              go(index < 0 ? 0 : index - 1);
              break;
            case "Home":
              go(0);
              break;
            case "End":
              go(order.length - 1);
              break;
            case "PageDown":
              go(index + 10);
              break;
            case "PageUp":
              go(index - 10);
              break;
            case " ":
              if (current) {
                e.preventDefault();
                actions.select(current.id, e.ctrlKey || e.metaKey ? "toggle" : "single");
              }
              break;
            case "Enter":
              if (current) {
                e.preventDefault();
                p.onOpen(current);
              }
              break;
            case "ContextMenu":
            case "F10":
              if (current && (e.key === "ContextMenu" || e.shiftKey)) {
                e.preventDefault();
                const r = document.getElementById(rowId(current.id))?.getBoundingClientRect();
                setMenu({ m: current, x: (r?.left ?? 0) + 40, y: r?.bottom ?? 0 });
              }
              break;
            case "a":
              if (e.ctrlKey || e.metaKey) {
                e.preventDefault();
                p.onSelectAll(true);
              }
              break;
          }
        }}
        onScroll={(e) => {
          const el = e.currentTarget;
          if (limit < p.messages.length && el.scrollTop + el.clientHeight > el.scrollHeight - 1500) {
            setLimit((l) => l + PAGE);
          }
        }}
      >
        {!searching && p.loading && p.messages.length === 0 && (
          <Spinner className="list-loading" size="small" label={t("Synchronisation…")} />
        )}
        {searching && p.searchMode?.searching && p.messages.length === 0 && (
          <Spinner className="list-loading" size="small" label={t("Recherche dans tous les dossiers…")} />
        )}
        {(searching ? !p.searchMode?.searching : !p.loading) && p.messages.length === 0 && (
          <div className="list-empty">
            {searching || filtering ? t("Aucun résultat.") : t("Aucun élément dans ce dossier.")}
          </div>
        )}

        {groups.map((g) => (
          <div key={g.label || "all"} role="group" aria-label={g.label || undefined}>
            {g.label && <div className="group-label">{g.label}</div>}
            {g.items.map((m) => (
              <Row
                key={m.id}
                m={m}
                selected={selectedSet.has(m.id)}
                active={m.id === active}
                selectionSize={p.selectedIds.length}
                ownerColor={unified ? accountById.get(m.accountId)?.color : undefined}
                ownerName={unified ? accountById.get(m.accountId)?.name : undefined}
                folderText={searching ? p.folderLabel(m.folderId) : undefined}
                actions={actions}
                stamp={stamp}
              />
            ))}
          </div>
        ))}
      </div>
      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.m)} onClose={() => setMenu(null)} />}
    </section>
  );
}
