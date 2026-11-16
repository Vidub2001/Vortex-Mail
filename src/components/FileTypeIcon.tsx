import {
  Calendar24Regular,
  Document24Regular,
  DocumentPdf24Regular,
  DocumentText24Regular,
  FolderZip24Regular,
  Image24Regular,
  Mail24Regular,
  SlideText24Regular,
  Table24Regular,
  Video24Regular,
} from "@fluentui/react-icons";

/** Icône et couleur selon le type de fichier (cartes de pièces jointes). */
export function FileTypeIcon({ name, contentType }: { name: string; contentType: string }) {
  const ext = name.toLowerCase().split(".").pop() ?? "";
  const t = contentType.toLowerCase();
  const pick = (): [typeof Document24Regular, string] => {
    if (t === "application/pdf" || ext === "pdf") return [DocumentPdf24Regular, "#c4314b"];
    if (t.startsWith("image/")) return [Image24Regular, "#8764b8"];
    if (t.startsWith("video/")) return [Video24Regular, "#ca5010"];
    if (t === "message/rfc822" || ext === "eml") return [Mail24Regular, "#0f6cbd"];
    if (t === "text/calendar" || ext === "ics") return [Calendar24Regular, "#0f6cbd"];
    if (["xls", "xlsx", "ods", "csv"].includes(ext)) return [Table24Regular, "#107c41"];
    if (["ppt", "pptx", "odp"].includes(ext)) return [SlideText24Regular, "#c43e1c"];
    if (["doc", "docx", "odt", "rtf"].includes(ext)) return [DocumentText24Regular, "#185abd"];
    if (["zip", "7z", "rar", "gz", "tgz", "xz", "bz2", "tar"].includes(ext)) return [FolderZip24Regular, "#986f0b"];
    if (t.startsWith("text/")) return [DocumentText24Regular, "#616161"];
    return [Document24Regular, "#616161"];
  };
  const [Icon, color] = pick();
  return <Icon className="file-icon" style={{ color }} aria-hidden />;
}
