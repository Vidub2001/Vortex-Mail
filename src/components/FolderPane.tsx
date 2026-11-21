import {
  Button,
  Menu,
  MenuItemRadio,
  MenuList,
  MenuPopover,
  MenuTrigger,
} from "@fluentui/react-components";
import {
  ArrowExport20Regular,
  ArrowImport20Regular,
  ArrowSync16Regular,
  ChevronDown16Regular,
  ChevronRight16Regular,
  Compose20Regular,
  Delete20Regular,
  DeleteDismiss20Regular,
  FolderAdd20Regular,
  FolderArrowRight20Regular,
  MoreHorizontal20Regular,
  Rename20Regular,
  Warning16Regular,
} from "@fluentui/react-icons";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import type { Account, Folder } from "../types";
import { UNIFIED_ROLES, members, unifiedFolder, unifiedId } from "../unified";
import { ContextMenu, type ContextMenuItem } from "./ContextMenu";
import { FolderIcon } from "./FolderIcon";
import { VortexSpinner } from "./VortexSpinner";
import { folderTree } from "../folder-tree";
import { t } from "../i18n";

interface Props {
  accounts: Account[];
  folders: Folder[];
  selectedFolderId: string | null;
  /** Messages en cours de glisser-déposer (vide si aucun). */
  draggingIds: number[];
  /** Erreur de connexion par compte. */
  accountErrors: Record<string, string>;
  loading: boolean;
  /** Vue unifiée (dossiers spéciaux regroupés) ou comptes séparés. */
  unifiedView: boolean;
  onUnifiedViewChange: (unified: boolean) => void;
  onRetry: () => void;
  onAddAccount: () => void;
  onSelect: (folderId: string) => void;
  onCompose: () => void;
  /** Clic droit : nouveau dossier (à la racine du compte si `parent` est nul). */
  onCreateFolder: (accountId: string, parent: Folder | null) => void;
  onDeleteFolder: (folder: Folder) => void;
  onRenameFolder: (folder: Folder) => void;
  onMoveFolder: (folder: Folder) => void;
  /** Vider définitivement (corbeille, indésirables), éventuellement plusieurs comptes. */
  onEmptyFolders: (folders: Folder[], label: string) => void;
  /** Export du dossier en `.mbox`, import de `.eml` ou `.mbox` dans le dossier (lot E8a). */
  onExportFolder: (folder: Folder) => void;
  onImportMessages: (folder: Folder) => void;
  /** Pas de réseau : erreurs de connexion masquées, lecture depuis le cache. */
  offline: boolean;
  onDropMessages: (folderId: string, ids: number[]) => void;
}

const STORAGE_KEY = "vortex.folderPane";

/** Sections et dossiers repliés, retrouvés au prochain lancement. */
function loadCollapsed(): Set<string> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return new Set(raw ? (JSON.parse(raw) as string[]) : []);
  } catch {
    return new Set();
  }
}

/** Compteur affiché : total des brouillons, non-lus ailleurs, rien pour les envoyés. */
function count(f: Folder): number | null {
  // Brouillons et éléments supprimés : nombre total de messages.
  if (f.role === "drafts" || f.role === "trash") return f.total > 0 ? f.total : null;
  if (f.role === "sent") return null;
  return f.unread > 0 ? f.unread : null;
}

/** Arborescence de tous les comptes, précédée des dossiers unifiés. */
export function FolderPane(p: Props) {
  const [collapsed, setCollapsed] = useState<Set<string>>(loadCollapsed);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);

  useEffect(() => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify([...collapsed]));
    } catch {
      // stockage indisponible : l'état repart par défaut au prochain lancement
    }
  }, [collapsed]);

  const toggle = (id: string) =>
    setCollapsed((set) => {
      const next = new Set(set);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  // Les dossiers unifiés sont repliés tant qu'on ne les a pas ouverts.
  const isOpen = (key: string, defaultOpen = true) => (collapsed.has(key) ? !defaultOpen : defaultOpen);

  /** Vue unifiée active (plusieurs comptes) : les dossiers non abonnés y sont masqués. */
  const unifiedActive = p.unifiedView && p.accounts.length > 1;

  /** Abonné, ou parent d'un dossier abonné (comme Thunderbird). */
  const tree = useMemo(() => folderTree(p.folders), [p.folders]);
  const folderById = tree.byId;

  const shown = (f: Folder): boolean => !unifiedActive || f.subscribed || tree.holdsSubscribed.has(f.id);

  const hasChildren = (f: Folder) => (tree.children.get(f.id) ?? []).some(shown);

  // Un dossier est visible si aucun de ses ancêtres n'est replié.
  const isVisible = (f: Folder): boolean => {
    let parent = f.parentId;
    while (parent) {
      if (collapsed.has(parent)) return false;
      parent = folderById.get(parent)?.parentId ?? null;
    }
    return true;
  };

  const accountById = new Map(p.accounts.map((a) => [a.id, a]));

  /** Clic droit sur un dossier réel. */
  const folderMenu = (f: Folder): ContextMenuItem[] => {
    const items: ContextMenuItem[] = [
      { label: t("Nouveau sous-dossier"), icon: <FolderAdd20Regular />, onClick: () => p.onCreateFolder(f.accountId, f) },
    ];
    if (f.role === "trash" || f.role === "junk") {
      items.push({
        label: t("Vider le dossier"),
        icon: <DeleteDismiss20Regular />,
        danger: true,
        disabled: f.total === 0,
        onClick: () => p.onEmptyFolders([f], f.name),
      });
    }
    items.push(
      { label: t("Importer des messages (.eml, .mbox)…"), icon: <ArrowImport20Regular />, onClick: () => p.onImportMessages(f) },
      { label: t("Exporter le dossier (.mbox)"), icon: <ArrowExport20Regular />, disabled: f.total === 0, onClick: () => p.onExportFolder(f) },
    );
    if (f.role === "custom") {
      items.push(
        { label: t("Renommer…"), icon: <Rename20Regular />, onClick: () => p.onRenameFolder(f) },
        { label: t("Déplacer vers…"), icon: <FolderArrowRight20Regular />, onClick: () => p.onMoveFolder(f) },
      );
      items.push({
        label: t("Supprimer le dossier"),
        icon: <Delete20Regular />,
        danger: true,
        onClick: () => p.onDeleteFolder(f),
      });
    }
    return items;
  };

  /** « Vider » seul (corbeille, indésirables), pour la partie unifiée. */
  const emptyMenu = (f: Folder): ContextMenuItem[] => folderMenu(f).filter((i) => i.label === t("Vider le dossier"));

  /** Clic droit sur un dossier unifié : vider tous les comptes d'un coup. */
  const unifiedMenu = (uf: Folder): ContextMenuItem[] =>
    uf.role === "trash" || uf.role === "junk"
      ? [
          {
            label: t("Vider (tous les comptes)"),
            icon: <DeleteDismiss20Regular />,
            danger: true,
            disabled: uf.total === 0,
            onClick: () => p.onEmptyFolders(members(uf.id, p.folders), uf.name),
          },
        ]
      : [];

  const openMenu = (e: React.MouseEvent, items: ContextMenuItem[]) => {
    e.preventDefault();
    e.stopPropagation();
    if (items.length > 0) setMenu({ x: e.clientX, y: e.clientY, items });
  };

  /** `f` est-il dans la sous-arborescence de `root` (sans être `root`) ? */
  const isUnder = (f: Folder, root: Folder): boolean => {
    let parent = f.parentId;
    while (parent) {
      if (parent === root.id) return true;
      parent = folderById.get(parent)?.parentId ?? null;
    }
    return false;
  };

  /** Dossier spécial (réception, envoyés…) ou rangé dans un dossier spécial. */
  const inSpecial = (f: Folder): boolean => {
    let cur: Folder | undefined = f;
    while (cur) {
      if (cur.role !== "custom") return true;
      cur = cur.parentId ? folderById.get(cur.parentId) : undefined;
    }
    return false;
  };

  /** Ligne de dossier ; `dropId` : dossier réel qui reçoit les messages déposés. */
  const row = (opts: {
    key: string;
    folder: Folder;
    label: ReactNode;
    indent: number;
    dropId: string | null;
    expandable?: boolean;
    open?: boolean;
    onToggle?: () => void;
    icon: ReactNode;
    title?: string;
    menu?: ContextMenuItem[];
  }) => {
    const hint = opts.folder.subscribed ? opts.title : t("{name} — non abonné (masqué en vue unifiée)", { name: opts.folder.name });
    const { folder: f } = opts;
    const selected = f.id === p.selectedFolderId;
    const canDrop = p.draggingIds.length > 0 && opts.dropId !== null;
    const n = count(f);
    const classes = [
      "folder-row",
      selected && "selected",
      f.unread > 0 && f.role !== "sent" && "has-unread",
      !f.subscribed && "unsubscribed",
      dropTarget === opts.key && "drop-target",
    ]
      .filter(Boolean)
      .join(" ");
    return (
      <div
        key={opts.key}
        role="treeitem"
        aria-selected={selected}
        aria-expanded={opts.expandable ? opts.open : undefined}
        // Un seul arrêt de tabulation pour l'arborescence (le dossier
        // ouvert) ; les flèches passent d'un dossier à l'autre (relecture
        // complète n° 35, modèle « Tree View » des pratiques WAI-ARIA).
        tabIndex={selected || !p.selectedFolderId ? 0 : -1}
        className={classes}
        style={{ paddingLeft: 12 + opts.indent * 16 }}
        title={hint}
        onContextMenu={(e) => openMenu(e, opts.menu ?? [])}
        onClick={() => p.onSelect(f.id)}
        onKeyDown={(e) => {
          const rows = Array.from(e.currentTarget.closest('[role="tree"]')?.querySelectorAll<HTMLElement>('[role="treeitem"]') ?? []);
          const here = rows.indexOf(e.currentTarget);
          const go = (target: HTMLElement | undefined) => {
            if (!target) return;
            e.preventDefault();
            target.focus();
          };
          switch (e.key) {
            case "Enter":
            case " ":
              e.preventDefault();
              p.onSelect(f.id);
              break;
            case "ArrowDown":
              go(rows[here + 1]);
              break;
            case "ArrowUp":
              go(rows[here - 1]);
              break;
            case "Home":
              go(rows[0]);
              break;
            case "End":
              go(rows[rows.length - 1]);
              break;
            case "ArrowRight":
              if (opts.expandable && !opts.open) {
                e.preventDefault();
                opts.onToggle?.();
              }
              break;
            case "ArrowLeft":
              if (opts.expandable && opts.open) {
                e.preventDefault();
                opts.onToggle?.();
              }
              break;
            case "ContextMenu":
            case "F10":
              if ((e.key === "ContextMenu" || e.shiftKey) && opts.menu && opts.menu.length > 0) {
                e.preventDefault();
                const r = e.currentTarget.getBoundingClientRect();
                setMenu({ x: r.left + 24, y: r.bottom, items: opts.menu });
              }
              break;
          }
        }}
        onDragOver={(e) => {
          if (!canDrop) return;
          e.preventDefault(); // autorise le dépôt
          e.dataTransfer.dropEffect = "move";
          if (dropTarget !== opts.key) setDropTarget(opts.key);
        }}
        onDragLeave={() => dropTarget === opts.key && setDropTarget(null)}
        onDrop={(e) => {
          e.preventDefault();
          setDropTarget(null);
          if (canDrop && opts.dropId) p.onDropMessages(opts.dropId, p.draggingIds);
        }}
      >
        <span
          className="folder-chevron"
          aria-hidden
          onClick={(e) => {
            if (!opts.expandable) return;
            e.stopPropagation();
            opts.onToggle?.();
          }}
        >
          {opts.expandable && (opts.open ? <ChevronDown16Regular /> : <ChevronRight16Regular />)}
        </span>
        {opts.icon}
        <span className="folder-name">{opts.label}</span>
        {n !== null && <span className="folder-count">{n}</span>}
      </div>
    );
  };

  // Dossiers unifiés : utiles dès qu'il y a plusieurs comptes.
  const unified =
    p.unifiedView && p.accounts.length > 1
      ? UNIFIED_ROLES.map(({ role }) => unifiedFolder(unifiedId(role), p.folders)).filter(
          (f): f is Folder => f !== undefined,
        )
      : [];
  const unifiedOpen = isOpen("section:unified");

  return (
    <nav className="folder-pane" aria-label={t("Dossiers")}>
      <div className="folder-pane-header">
        <Button appearance="primary" className="compose-btn" icon={<Compose20Regular />} onClick={p.onCompose}>
          {t("Nouveau courrier")}
        </Button>
        <span className="folder-pane-actions">
          {p.accounts.length > 1 && (
            <Menu
              positioning="below-end"
              checkedValues={{ view: [p.unifiedView ? "unified" : "accounts"] }}
              onCheckedValueChange={(_, d) => p.onUnifiedViewChange(d.checkedItems[0] === "unified")}
            >
              <MenuTrigger disableButtonEnhancement>
                <Button
                  appearance="subtle"
                  icon={<MoreHorizontal20Regular />}
                  title={t("Affichage des dossiers")}
                  aria-label={t("Affichage des dossiers")}
                />
              </MenuTrigger>
              <MenuPopover>
                <MenuList>
                  <MenuItemRadio name="view" value="unified">
                    {t("Dossiers unifiés")}
                  </MenuItemRadio>
                  <MenuItemRadio name="view" value="accounts">
                    {t("Comptes séparés")}
                  </MenuItemRadio>
                </MenuList>
              </MenuPopover>
            </Menu>
          )}
          {/* Pendant une relecture : le tourbillon, et un clic en programme une autre juste après (lot P). */}
          <Button
            appearance="subtle"
            icon={p.loading ? <VortexSpinner /> : <ArrowSync16Regular />}
            title={p.loading ? t("Actualisation des dossiers en cours") : t("Actualiser les dossiers")}
            aria-label={p.loading ? t("Actualisation des dossiers en cours") : t("Actualiser les dossiers")}
            aria-busy={p.loading}
            onClick={p.onRetry}
          />
        </span>
      </div>

      <div className="folder-scroll" role="tree">
        {!p.loading && p.accounts.length === 0 && (
          <div className="no-account">
            <p>{t("Aucun compte configuré.")}</p>
            <Button appearance="primary" onClick={p.onAddAccount}>
              {t("Ajouter un compte")}
            </Button>
          </div>
        )}

        {unified.length > 0 && (
          <section className="account-section unified-section">
            <button
              className="account-header"
              onClick={() => toggle("section:unified")}
              aria-expanded={unifiedOpen}
              title={t("Les dossiers de même type de tous les comptes, réunis")}
            >
              {unifiedOpen ? <ChevronDown16Regular /> : <ChevronRight16Regular />}
              <span className="unified-dots" aria-hidden>
                {p.accounts.slice(0, 3).map((a) => (
                  <span key={a.id} className="account-dot" style={{ background: a.color }} />
                ))}
              </span>
              <span className="account-name">{t("Dossiers unifiés")}</span>
            </button>

            {unifiedOpen &&
              unified.map((uf) => {
                const key = `unified:${uf.role}`;
                const open = isOpen(`open:${key}`, false);
                const parts = members(uf.id, p.folders);
                return [
                  row({
                    key,
                    folder: uf,
                    label: uf.name,
                    indent: 0,
                    dropId: null, // un dossier unifié n'a pas de compte : on dépose sur un de ses membres
                    expandable: true,
                    open,
                    onToggle: () => toggle(`open:${key}`),
                    icon: <FolderIcon role={uf.role} />,
                    menu: unifiedMenu(uf),
                  }),
                  ...(open
                    ? parts.flatMap((f) => {
                        const a = accountById.get(f.accountId);
                        const memberOpen = !collapsed.has(f.id);
                        // Sous-dossiers du dossier de ce compte (ex. dossiers rangés dans la réception).
                        const children = memberOpen
                          ? p.folders.filter((x) => isUnder(x, f) && isVisible(x) && shown(x))
                          : [];
                        return [
                          row({
                            key: `${key}>${f.id}`,
                            folder: f,
                            label: a?.name ?? f.accountId,
                            indent: 1,
                            dropId: f.id,
                            expandable: hasChildren(f),
                            open: memberOpen,
                            onToggle: () => toggle(f.id),
                            icon: <span className="account-dot unified-member-dot" style={{ background: a?.color }} />,
                            title: `${f.name} — ${a?.email ?? f.accountId}`,
                            // Vue unifiée : seulement « Vider » ; créer et supprimer se font sous le compte.
                            menu: emptyMenu(f),
                          }),
                          ...children.map((x) =>
                            row({
                              key: `${key}>${x.id}`,
                              folder: x,
                              label: x.name,
                              indent: 1 + x.depth - f.depth,
                              dropId: x.id,
                              expandable: hasChildren(x),
                              open: !collapsed.has(x.id),
                              onToggle: () => toggle(x.id),
                              icon: <FolderIcon role={x.role} />,
                            }),
                          ),
                        ];
                      })
                    : []),
                ];
              })}
          </section>
        )}

        {p.accounts.map((account) => {
          const open = isOpen(account.id);
          // Avec les dossiers unifiés, le compte ne liste que ses dossiers personnels,
          // comme Thunderbird : les dossiers spéciaux sont déjà regroupés plus haut.
          const folders = p.folders.filter(
            (f) => f.accountId === account.id && !(unified.length > 0 && inSpecial(f)) && shown(f),
          );
          return (
            <section key={account.id} className="account-section">
              <button
                className="account-header"
                onClick={() => toggle(account.id)}
                onContextMenu={(e) =>
                  openMenu(e, [
                    {
                      label: t("Nouveau dossier"),
                      icon: <FolderAdd20Regular />,
                      onClick: () => p.onCreateFolder(account.id, null),
                    },
                  ])
                }
                aria-expanded={open}
                title={account.email}
              >
                {open ? <ChevronDown16Regular /> : <ChevronRight16Regular />}
                <span className="account-dot" style={{ background: account.color }} />
                <span className="account-name">{account.name}</span>
              </button>

              {open && !p.offline && p.accountErrors[account.id] && (
                <div className="account-error" title={p.accountErrors[account.id]}>
                  <Warning16Regular />
                  <span>{p.accountErrors[account.id]}</span>
                  <Button size="small" appearance="transparent" onClick={p.onRetry}>
                    {t("Réessayer")}
                  </Button>
                </div>
              )}

              {open && unified.length > 0 && folders.length === 0 && (p.offline || !p.accountErrors[account.id]) && (
                <div className="account-empty">{t("Aucun dossier personnel")}</div>
              )}

              {open &&
                folders.filter(isVisible).map((f) =>
                  row({
                    key: f.id,
                    folder: f,
                    label: f.name,
                    indent: f.depth,
                    dropId: f.id,
                    expandable: hasChildren(f),
                    open: !collapsed.has(f.id),
                    onToggle: () => toggle(f.id),
                    icon: <FolderIcon role={f.role} />,
                    menu: folderMenu(f),
                  }),
                )}
            </section>
          );
        })}
      </div>
      {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}
    </nav>
  );
}
