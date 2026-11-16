import {
  Archive20Regular,
  Delete20Regular,
  Drafts20Regular,
  Folder20Regular,
  MailInbox20Regular,
  Send20Regular,
  Warning20Regular,
} from "@fluentui/react-icons";
import type { FolderRole } from "../types";

export function FolderIcon({ role }: { role: FolderRole }) {
  switch (role) {
    case "inbox":
      return <MailInbox20Regular />;
    case "drafts":
      return <Drafts20Regular />;
    case "sent":
      return <Send20Regular />;
    case "archive":
      return <Archive20Regular />;
    case "trash":
      return <Delete20Regular />;
    case "junk":
      return <Warning20Regular />;
    default:
      return <Folder20Regular />;
  }
}
