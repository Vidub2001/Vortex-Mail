// « Déplacer » du clic droit (lot Q) : sous-menu ouvert à droite au survol
// de l'entrée. Une zone de recherche ; sans recherche, les derniers dossiers
// où l'on a déplacé des messages ; avec une recherche, les dossiers de tous
// les comptes dont le nom correspond. En bas, « Créer un dossier… ». Flèches
// haut et bas pour choisir, Entrée pour déplacer, ou un clic sur le dossier.
import { Input } from "@fluentui/react-components";
import { Dismiss16Regular, FolderAdd16Regular, Search16Regular } from "@fluentui/react-icons";
import { Fragment, useEffect, useMemo, useState, type KeyboardEvent, type ReactNode } from "react";
import { t } from "../i18n";
import { moveTargets, recentMoveIds, recentTargets } from "../move-targets";
import type { Account, Folder } from "../types";
import { FolderIcon } from "./FolderIcon";

interface Props {
  accounts: Account[];
  folders: Folder[];
  /** Dossiers à ne pas proposer : celui d'où viennent les messages. */
  exclude: ReadonlySet<string>;
  onMove: (folderId: string) => void;
  /** « Créer un dossier… », avec le texte cherché comme nom proposé. */
  onCreate: (name: string) => void;
}

export function MovePicker(p: Props) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const searching = query.trim() !== "";
  const recents = useMemo(() => recentTargets(p.accounts, p.folders, recentMoveIds(), p.exclude), [p.accounts, p.folders, p.exclude]);
  const groups = useMemo(() => (searching ? moveTargets(p.accounts, p.folders, query, p.exclude) : []), [p.accounts, p.folders, query, p.exclude, searching]);
  const flat = useMemo(
    () => (searching ? groups.flatMap((g) => g.targets.map((x) => x.folder.id)) : recents.map((x) => x.folder.id)),
    [searching, groups, recents],
  );

  // Dossier choisi au clavier toujours visible dans la liste.
  useEffect(() => {
    document.getElementById(`move-target-${active}`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(i + 1, flat.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter" && flat[active]) {
      e.preventDefault();
      p.onMove(flat[active]);
    }
  };

  const option = (i: number, folder: Folder, label: ReactNode, indent: number) => (
    <div
      key={folder.id}
      id={`move-target-${i}`}
      role="option"
      tabIndex={-1}
      aria-selected={i === active}
      className={i === active ? "move-target active" : "move-target"}
      style={{ paddingLeft: 10 + indent }}
      onMouseEnter={() => setActive(i)}
      onClick={() => p.onMove(folder.id)}
      onKeyDown={(e) => e.key === "Enter" && p.onMove(folder.id)}
    >
      <FolderIcon role={folder.role} />
      <span className="move-name">{folder.name}</span>
      {label}
    </div>
  );

  let index = -1;
  return (
    <div className="move-picker">
      <Input
        autoFocus
        className="move-search"
        contentBefore={<Search16Regular />}
        contentAfter={
          query ? (
            <button
              type="button"
              className="move-clear"
              title={t("Effacer la recherche")}
              aria-label={t("Effacer la recherche")}
              onClick={() => {
                setQuery("");
                setActive(0);
              }}
            >
              <Dismiss16Regular />
            </button>
          ) : undefined
        }
        placeholder={t("Rechercher un dossier")}
        aria-label={t("Rechercher un dossier")}
        aria-controls="move-targets"
        aria-activedescendant={flat.length > 0 ? `move-target-${active}` : undefined}
        value={query}
        onChange={(_, d) => {
          setQuery(d.value);
          setActive(0);
        }}
        onKeyDown={onKey}
      />
      <div id="move-targets" className="move-targets" role="listbox" aria-label={t("Dossiers")}>
        {!searching && (
          <>
            <div className="move-account" role="presentation">
              {t("Récents")}
            </div>
            {recents.length === 0 && <p className="move-empty">{t("Tapez le nom d'un dossier pour le trouver.")}</p>}
            {recents.map(({ folder, account, parents }) => {
              index += 1;
              const where = [parents, p.accounts.length > 1 ? account.name : ""].filter(Boolean).join(" · ");
              return option(index, folder, where ? <span className="move-parents">{where}</span> : null, 0);
            })}
          </>
        )}
        {searching && groups.length === 0 && <p className="move-empty">{t("Aucun dossier ne correspond")}</p>}
        {groups.map((g) => (
          <Fragment key={g.account.id}>
            <div className="move-account" role="presentation">
              <span className="account-dot" style={{ background: g.account.color }} />
              {g.account.name}
            </div>
            {g.targets.map(({ folder, parents }) => {
              index += 1;
              return option(index, folder, parents ? <span className="move-parents">{parents}</span> : null, 0);
            })}
          </Fragment>
        ))}
      </div>
      <div className="move-actions">
        <button type="button" className="context-item" onClick={() => p.onCreate(query.trim())}>
          <span className="context-icon">
            <FolderAdd16Regular />
          </span>
          {t("Créer un dossier…")}
        </button>
      </div>
    </div>
  );
}
