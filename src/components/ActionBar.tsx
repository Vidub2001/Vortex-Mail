import {
  Menu,
  MenuDivider,
  MenuGroup,
  MenuGroupHeader,
  MenuItem,
  MenuList,
  MenuPopover,
  MenuTrigger,
  Overflow,
  OverflowItem,
  Toolbar,
  ToolbarButton,
  useIsOverflowItemVisible,
  useOverflowMenu,
} from "@fluentui/react-components";
import {
  Archive20Regular,
  ArrowForward20Regular,
  ArrowReply20Regular,
  ArrowReplyAll20Regular,
  Delete20Regular,
  Flag20Filled,
  Flag20Regular,
  FolderArrowRight20Regular,
  MailRead20Regular,
  MailUnread20Regular,
  MoreHorizontal20Regular,
  WindowNew20Regular,
} from "@fluentui/react-icons";
import { Fragment } from "react";
import type { Account, Folder } from "../types";
import { FolderIcon } from "./FolderIcon";
import { t } from "../i18n";

interface Props {
  accounts: Account[];
  folders: Folder[];
  /** Un seul message ouvert : réponses et transfert possibles. */
  canReply: boolean;
  selectionCount: number;
  allSeen: boolean;
  allFlagged: boolean;
  onReply: (mode: "reply" | "replyAll" | "forward") => void;
  onArchive: () => void;
  onDelete: () => void;
  onMove: (folderId: string) => void;
  onToggleSeen: () => void;
  onToggleFlag: () => void;
  /** « Ouvrir dans une fenêtre » (absent dans la fenêtre du message elle-même). */
  onPopOut?: () => void;
  /** Icônes seules (libellés en info-bulle), pour l'en-tête du message. */
  compact?: boolean;
}

interface Item {
  id: string;
  label: string;
  icon: JSX.Element;
  disabled: boolean;
  onClick?: () => void;
  /** Début d'un groupe : petit espace avant. */
  group?: boolean;
  move?: boolean;
}

/** Dossiers de tous les comptes, pour « Déplacer ». */
function MoveList({ accounts, folders, onMove }: Pick<Props, "accounts" | "folders" | "onMove">) {
  return (
    <MenuList className="move-menu">
      {accounts.map((account, i) => (
        <Fragment key={account.id}>
          {i > 0 && <MenuDivider />}
          <MenuGroup>
            <MenuGroupHeader>
              <span className="account-dot" style={{ background: account.color }} />
              {account.name}
            </MenuGroupHeader>
            {folders
              .filter((f) => f.accountId === account.id)
              .map((f) => (
                <MenuItem key={f.id} icon={<FolderIcon role={f.role} />} onClick={() => onMove(f.id)}>
                  <span style={{ paddingLeft: f.depth * 14 }}>{f.name}</span>
                </MenuItem>
              ))}
          </MenuGroup>
        </Fragment>
      ))}
    </MenuList>
  );
}

/** Entrée du menu « Plus » : n'apparaît que si son bouton ne tient plus dans la barre. */
function OverflowEntry({ item, p }: { item: Item; p: Props }) {
  const visible = useIsOverflowItemVisible(item.id);
  if (visible) return null;
  if (item.move) {
    return (
      <Menu>
        <MenuTrigger disableButtonEnhancement>
          <MenuItem icon={item.icon} disabled={item.disabled}>
            {item.label}
          </MenuItem>
        </MenuTrigger>
        <MenuPopover>
          <MoveList accounts={p.accounts} folders={p.folders} onMove={p.onMove} />
        </MenuPopover>
      </Menu>
    );
  }
  return (
    <MenuItem icon={item.icon} disabled={item.disabled} onClick={item.onClick}>
      {item.label}
    </MenuItem>
  );
}

/** « Plus » : seulement quand la fenêtre est trop étroite pour tout afficher. */
function OverflowMenu({ items, p }: { items: Item[]; p: Props }) {
  const { ref, isOverflowing } = useOverflowMenu<HTMLButtonElement>();
  if (!isOverflowing) return null;
  return (
    <Menu positioning="below-end">
      <MenuTrigger disableButtonEnhancement>
        <ToolbarButton ref={ref} className="action-btn" icon={<MoreHorizontal20Regular />} title={t("Plus d'actions")}>
          {p.compact ? null : t("Plus")}
        </ToolbarButton>
      </MenuTrigger>
      <MenuPopover>
        <MenuList>
          {items.map((item) => (
            <OverflowEntry key={item.id} item={item} p={p} />
          ))}
        </MenuList>
      </MenuPopover>
    </Menu>
  );
}

/** Barre d'actions en tête du volet de lecture ; s'applique à la sélection. */
export function ActionBar(p: Props) {
  const none = p.selectionCount === 0;
  const items: Item[] = [
    { id: "reply", label: t("Répondre"), icon: <ArrowReply20Regular />, disabled: !p.canReply, onClick: () => p.onReply("reply") },
    { id: "replyAll", label: t("Rép. à tous"), icon: <ArrowReplyAll20Regular />, disabled: !p.canReply, onClick: () => p.onReply("replyAll") },
    { id: "forward", label: t("Transférer"), icon: <ArrowForward20Regular />, disabled: !p.canReply, onClick: () => p.onReply("forward") },
    { id: "archive", label: t("Archiver"), icon: <Archive20Regular />, disabled: none, onClick: p.onArchive, group: true },
    { id: "delete", label: t("Supprimer"), icon: <Delete20Regular />, disabled: none, onClick: p.onDelete },
    { id: "move", label: t("Déplacer"), icon: <FolderArrowRight20Regular />, disabled: none, move: true },
    {
      id: "seen",
      label: p.allSeen ? t("Non lu") : t("Lu"),
      icon: p.allSeen ? <MailUnread20Regular /> : <MailRead20Regular />,
      disabled: none,
      onClick: p.onToggleSeen,
      group: true,
    },
    {
      id: "flag",
      label: p.allFlagged ? t("Sans indicateur") : t("Indicateur"),
      icon: p.allFlagged ? <Flag20Filled /> : <Flag20Regular />,
      disabled: none,
      onClick: p.onToggleFlag,
    },
  ];

  return (
    // Place réservée hors débordement : « Plus » et, à droite, « Fenêtre ».
    <Overflow padding={p.compact ? (p.onPopOut ? 90 : 44) : p.onPopOut ? 130 : 64}>
      <Toolbar className={p.compact ? "actionbar compact" : "actionbar"} aria-label={t("Actions sur la sélection")}>
        {items.map((item) => (
          <OverflowItem key={item.id} id={item.id}>
            {item.move ? (
              <div className={item.group ? "action-wrap group" : "action-wrap"}>
                <Menu positioning="below-start">
                  <MenuTrigger disableButtonEnhancement>
                    <ToolbarButton className="action-btn" icon={item.icon} disabled={item.disabled} title={t("Déplacer vers")}>
                      {p.compact ? null : item.label}
                    </ToolbarButton>
                  </MenuTrigger>
                  <MenuPopover>
                    <MoveList accounts={p.accounts} folders={p.folders} onMove={p.onMove} />
                  </MenuPopover>
                </Menu>
              </div>
            ) : (
              <ToolbarButton
                className={item.group ? "action-btn group" : "action-btn"}
                icon={item.icon}
                disabled={item.disabled}
                onClick={item.onClick}
                title={item.label}
              >
                {p.compact ? null : item.label}
              </ToolbarButton>
            )}
          </OverflowItem>
        ))}
        <OverflowMenu items={items} p={p} />
        {p.onPopOut && (
          <ToolbarButton
            className="action-btn popout"
            icon={<WindowNew20Regular />}
            disabled={!p.canReply}
            onClick={p.onPopOut}
            title={t("Ouvrir dans une nouvelle fenêtre")}
          >
            {p.compact ? null : t("Fenêtre")}
          </ToolbarButton>
        )}
      </Toolbar>
    </Overflow>
  );
}
