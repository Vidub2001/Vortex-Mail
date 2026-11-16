// Dossiers unifiés : un dossier virtuel par rôle, qui regroupe le dossier
// de même rôle de chaque compte.
import { t } from "./i18n";
import type { Folder, FolderRole, MessageSummary } from "./types";

const PREFIX = "unified:";

/** Ordre et noms affichés. */
export const UNIFIED_ROLES: { role: FolderRole; name: string }[] = [
  { role: "inbox", name: t("Boîte de réception") },
  { role: "drafts", name: t("Brouillons") },
  { role: "sent", name: t("Éléments envoyés") },
  { role: "archive", name: t("Archive") },
  { role: "junk", name: t("Courrier indésirable") },
  { role: "trash", name: t("Éléments supprimés") },
];

export const unifiedId = (role: FolderRole) => `${PREFIX}${role}`;

export const isUnified = (id: string | null | undefined): id is string => !!id && id.startsWith(PREFIX);

export function unifiedRole(id: string): FolderRole | null {
  const role = id.slice(PREFIX.length) as FolderRole;
  return UNIFIED_ROLES.some((r) => r.role === role) ? role : null;
}

/** Dossiers réels regroupés sous un dossier unifié, dans l'ordre des comptes. */
export function members(id: string, folders: Folder[]): Folder[] {
  const role = unifiedRole(id);
  return role ? folders.filter((f) => f.role === role) : [];
}

/** Dossier virtuel avec les compteurs cumulés de ses membres. */
export function unifiedFolder(id: string, folders: Folder[]): Folder | undefined {
  const role = unifiedRole(id);
  const entry = UNIFIED_ROLES.find((r) => r.role === role);
  const list = members(id, folders);
  if (!entry || list.length === 0) return undefined;
  return {
    id,
    accountId: "",
    name: entry.name,
    path: "",
    parentId: null,
    depth: 0,
    role: entry.role,
    unread: list.reduce((n, f) => n + f.unread, 0),
    total: list.reduce((n, f) => n + f.total, 0),
    subscribed: true,
  };
}

/** Fusionne les listes de plusieurs dossiers, du plus récent au plus ancien. */
export function mergeLists(lists: MessageSummary[][]): MessageSummary[] {
  return lists.flat().sort((a, b) => b.date - a.date);
}
