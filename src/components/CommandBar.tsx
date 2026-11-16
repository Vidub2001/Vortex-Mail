import {
  Menu,
  MenuDivider,
  MenuGroup,
  MenuGroupHeader,
  MenuItem,
  MenuList,
  MenuPopover,
  MenuTrigger,
  Toolbar,
  ToolbarButton,
  ToolbarDivider,
} from "@fluentui/react-components";
import {
  Archive20Regular,
  Compose20Regular,
  Delete20Regular,
  Flag20Regular,
  FolderArrowRight20Regular,
  MailRead20Regular,
  MailUnread20Regular,
} from "@fluentui/react-icons";
import { Fragment } from "react";
import type { Account, Folder } from "../types";
import { FolderIcon } from "./FolderIcon";
import { t } from "../i18n";

interface Props {
  accounts: Account[];
  folders: Folder[];
  selectionCount: number;
  allSeen: boolean;
  allFlagged: boolean;
  onCompose: () => void;
  onDelete: () => void;
  onArchive: () => void;
  onMove: (folderId: string) => void;
  onToggleSeen: () => void;
  onToggleFlag: () => void;
}

/** Ruban d'actions simplifié, au-dessus de la liste et du volet de lecture. */
export function CommandBar(p: Props) {
  const none = p.selectionCount === 0;
  return (
    <Toolbar className="commandbar" aria-label={t("Actions")}>
      <ToolbarButton appearance="primary" icon={<Compose20Regular />} onClick={p.onCompose}>
        {t("Nouveau courrier")}
      </ToolbarButton>
      <ToolbarDivider />
      <ToolbarButton icon={<Delete20Regular />} disabled={none} onClick={p.onDelete}>
        {t("Supprimer")}
      </ToolbarButton>
      <ToolbarButton icon={<Archive20Regular />} disabled={none} onClick={p.onArchive}>
        {t("Archiver")}
      </ToolbarButton>

      <Menu positioning="below-start">
        <MenuTrigger disableButtonEnhancement>
          <ToolbarButton icon={<FolderArrowRight20Regular />} disabled={none}>
            {t("Déplacer vers")}
          </ToolbarButton>
        </MenuTrigger>
        <MenuPopover>
          <MenuList className="move-menu">
            {p.accounts.map((account, i) => (
              <Fragment key={account.id}>
                {i > 0 && <MenuDivider />}
                <MenuGroup>
                  <MenuGroupHeader>
                    <span className="account-dot" style={{ background: account.color }} />
                    {account.name}
                  </MenuGroupHeader>
                  {p.folders
                    .filter((f) => f.accountId === account.id)
                    .map((f) => (
                      <MenuItem key={f.id} icon={<FolderIcon role={f.role} />} onClick={() => p.onMove(f.id)}>
                        <span style={{ paddingLeft: f.depth * 14 }}>{f.name}</span>
                      </MenuItem>
                    ))}
                </MenuGroup>
              </Fragment>
            ))}
          </MenuList>
        </MenuPopover>
      </Menu>

      <ToolbarDivider />
      <ToolbarButton
        icon={p.allSeen ? <MailUnread20Regular /> : <MailRead20Regular />}
        disabled={none}
        onClick={p.onToggleSeen}
      >
        {p.allSeen ? t("Marquer comme non lu") : t("Marquer comme lu")}
      </ToolbarButton>
      <ToolbarButton icon={<Flag20Regular />} disabled={none} onClick={p.onToggleFlag}>
        {p.allFlagged ? t("Retirer l'indicateur") : t("Indicateur")}
      </ToolbarButton>
    </Toolbar>
  );
}
