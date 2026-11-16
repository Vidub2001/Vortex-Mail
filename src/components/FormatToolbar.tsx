// Barre de mise en forme de la rédaction : police, taille, couleurs, styles,
// alignement, insertion (image, lien, tableau), pièce jointe, citation.
import {
  Button,
  Input,
  Menu,
  MenuItem,
  MenuList,
  MenuPopover,
  MenuTrigger,
  Popover,
  PopoverSurface,
  PopoverTrigger,
} from "@fluentui/react-components";
import {
  Attach20Regular,
  ChevronDown12Regular,
  Image20Regular,
  Link20Regular,
  Table20Regular,
  TextAlignCenter20Regular,
  TextAlignLeft20Regular,
  TextAlignRight20Regular,
  TextBold20Regular,
  TextBulletListLtr20Regular,
  TextClearFormatting20Regular,
  TextItalic20Regular,
  TextNumberListLtr20Regular,
  TextQuote20Regular,
  TextStrikethrough20Regular,
  TextUnderline20Regular,
} from "@fluentui/react-icons";
import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { QUOTE_STYLE } from "../compose";
import { colorName } from "../colors";
import { t } from "../i18n";

const COLORS = [
  "#000000", "#444444", "#888888", "#cccccc", "#ffffff",
  "#c62828", "#ef6c00", "#f9a825", "#2e7d32", "#00838f",
  "#1565c0", "#1e88e5", "#6a1b9a", "#ad1457", "#4e342e",
];

const HIGHLIGHTS = ["transparent", "#fff176", "#a5d6a7", "#90caf9", "#f48fb1", "#ffcc80", "#ce93d8", "#e0e0e0"];

interface Props {
  editor: RefObject<HTMLDivElement>;
  disabled: boolean;
  /** Message en texte brut : seuls le format et la pièce jointe restent. */
  plain: boolean;
  onPlainChange: (plain: boolean) => void;
  onAttach: () => void;
}

/**
 * HTML construit par la barre d'outils elle-même (lien, tableau, image) : seul
 * type accepté par les insertions de HTML ci-dessous. Une chaîne quelconque,
 * venue d'un message par exemple, ne passe pas sans `toolbarHtml`, que la
 * relecture repère (relecture complète n° 48).
 */
type ToolbarHtml = string & { readonly __toolbarHtml: unique symbol };
const toolbarHtml = (html: string) => html as ToolbarHtml;

export function FormatToolbar({ editor, disabled, plain, onPlainChange, onAttach }: Props) {
  /** Dernière sélection dans l'éditeur : les menus prennent le focus, on la restaure. */
  const saved = useRef<Range | null>(null);
  const imagePicker = useRef<HTMLInputElement>(null);
  const [state, setState] = useState({ active: new Set<string>() });
  const [textColor, setTextColor] = useState("#c62828");
  const [highlight, setHighlight] = useState("#fff176");
  const [linkOpen, setLinkOpen] = useState(false);
  const [linkUrl, setLinkUrl] = useState("https://");
  const [insertOpen, setInsertOpen] = useState(false);

  const inEditor = (node: Node | null) => !!node && !!editor.current && editor.current.contains(node);

  // Suit la sélection : mémorise la plage et met à jour les boutons actifs.
  useEffect(() => {
    const onChange = () => {
      const sel = window.getSelection();
      if (!sel || sel.rangeCount === 0 || !inEditor(sel.anchorNode)) return;
      saved.current = sel.getRangeAt(0).cloneRange();
      const active = new Set<string>();
      for (const c of ["bold", "italic", "underline", "strikeThrough", "insertUnorderedList", "insertOrderedList", "justifyCenter", "justifyRight"]) {
        try {
          if (document.queryCommandState(c)) active.add(c);
        } catch {
          // commande inconnue du moteur : ignorée
        }
      }
      const el = sel.anchorNode instanceof Element ? sel.anchorNode : sel.anchorNode?.parentElement;
      if (el?.closest("blockquote")) active.add("quote");
      setState({ active });
    };
    document.addEventListener("selectionchange", onChange);
    return () => document.removeEventListener("selectionchange", onChange);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Rend le focus à l'éditeur, à l'endroit de la dernière sélection. */
  const restore = () => {
    const el = editor.current;
    if (!el) return;
    el.focus();
    const sel = window.getSelection();
    if (saved.current && sel) {
      sel.removeAllRanges();
      sel.addRange(saved.current);
    }
  };

  const exec = (command: string, value?: string) => {
    restore();
    document.execCommand("styleWithCSS", false, "true");
    document.execCommand(command, false, value);
    document.dispatchEvent(new Event("selectionchange"));
  };

  // nosemgrep: vortex-raw-html-sink -- n'accepte que du ToolbarHtml, construit plus bas par la barre d'outils, jamais tiré d'un mail
  const insertHtml = (html: ToolbarHtml) => exec("insertHTML", html);

  /**
   * Insère un bloc (tableau, image) après la sélection, sans la remplacer.
   * Par le DOM : `insertHTML` retire les bordures des cellules.
   */
  const insertBlock = (html: ToolbarHtml) => {
    restore();
    const sel = window.getSelection();
    const el = editor.current;
    if (!sel || !el) return;
    let range = sel.rangeCount > 0 && inEditor(sel.getRangeAt(0).startContainer) ? sel.getRangeAt(0) : null;
    if (!range) {
      range = document.createRange();
      range.selectNodeContents(el);
    }
    range.collapse(false);
    // nosemgrep: vortex-raw-html-sink -- n'accepte que du ToolbarHtml (tableau ou image construits par la barre d'outils, image en data: lue par FileReader)
    const fragment = range.createContextualFragment(html);
    const last = fragment.lastChild;
    range.insertNode(fragment);
    if (last) {
      const after = document.createRange();
      after.setStartAfter(last);
      after.collapse(true);
      sel.removeAllRanges();
      sel.addRange(after);
    }
    document.dispatchEvent(new Event("selectionchange"));
  };

  const toggleQuote = () => {
    if (state.active.has("quote")) {
      exec("formatBlock", "div");
      return;
    }
    exec("formatBlock", "blockquote");
    editor.current?.querySelectorAll<HTMLElement>("blockquote:not([style])").forEach((q) => q.setAttribute("style", QUOTE_STYLE));
  };

  const clearFormatting = () => {
    exec("removeFormat");
    exec("unlink");
    exec("formatBlock", "div");
  };

  const insertTable = (rows: number, cols: number) => {
    const cell = '<td style="border:1px solid #9e9e9e;padding:4px 8px;min-width:60px">&nbsp;</td>';
    const row = `<tr>${cell.repeat(cols)}</tr>`;
    insertBlock(toolbarHtml(`<table style="border-collapse:collapse;margin:6px 0">${row.repeat(rows)}</table><div><br></div>`));
  };

  const insertImage = (file: File) => {
    const reader = new FileReader();
    reader.onload = () => insertBlock(toolbarHtml(`<img src="${String(reader.result).replace(/"/g, "%22")}" alt="" style="max-width:100%">`));
    reader.readAsDataURL(file);
  };

  const applyLink = () => {
    setLinkOpen(false);
    const url = linkUrl.trim();
    if (!url || url === "https://") return;
    const href = /^[a-z]+:/i.test(url) ? url : `https://${url}`;
    if (saved.current && !saved.current.collapsed) exec("createLink", href);
    else insertHtml(toolbarHtml(`<a href="${href.replace(/"/g, "&quot;")}">${href.replace(/</g, "&lt;")}</a>&nbsp;`));
  };

  const keep = (e: React.MouseEvent) => e.preventDefault();

  /** Bouton icône + libellé, qui ne prend pas le focus (la sélection reste en place). */
  const tool = (id: string, icon: ReactNode, label: string, onClick: () => void, title = label) => (
    <button
      key={id}
      type="button"
      className={state.active.has(id) ? "fmt-btn active" : "fmt-btn"}
      title={title}
      aria-label={title}
      aria-pressed={state.active.has(id)}
      disabled={disabled}
      onMouseDown={keep}
      onClick={onClick}
    >
      {icon}
      <span>{label}</span>
    </button>
  );

  const formatSwitch = (
    <div className="fmt-group fmt-format">
      <small>{t("Format")}</small>
      <div className="fmt-segmented" role="radiogroup" aria-label={t("Format du message")}>
        {(
          [
            [false, "HTML"],
            [true, t("Texte")],
          ] as const
        ).map(([value, label]) => (
          <button
            key={label}
            type="button"
            role="radio"
            aria-checked={plain === value}
            className={plain === value ? "on" : ""}
            disabled={disabled}
            onMouseDown={keep}
            onClick={() => onPlainChange(value)}
            title={value ? t("Texte brut, sans mise en forme") : t("Texte mis en forme")}
          >
            {label}
          </button>
        ))}
      </div>
    </div>
  );

  if (plain) {
    return (
      <div className="fmt-bar" role="toolbar" aria-label={t("Mise en forme")}>
        <div className="fmt-group">{tool("attach", <Attach20Regular />, t("Pièce jointe"), onAttach, t("Joindre des fichiers"))}</div>
        {formatSwitch}
      </div>
    );
  }

  const swatch = (color: string) => <span className="fmt-swatch" style={{ background: color === "transparent" ? undefined : color }} />;

  return (
    <div className="fmt-bar" role="toolbar" aria-label={t("Mise en forme")}>
      <div className="fmt-group">
        <span className="fmt-split">
          {tool("color", <span className="fmt-color-icon" style={{ borderColor: textColor }}>A</span>, t("Couleur"), () => exec("foreColor", textColor), t("Couleur du texte"))}
          <Popover positioning="below-start" onOpenChange={(_, d) => !d.open && restore()}>
            <PopoverTrigger disableButtonEnhancement>
              <button type="button" className="fmt-drop" disabled={disabled} onMouseDown={keep} aria-label={t("Choisir la couleur du texte")}>
                <ChevronDown12Regular />
              </button>
            </PopoverTrigger>
            <PopoverSurface className="fmt-palette">
              {COLORS.map((c) => (
                <button key={c} type="button" title={colorName(c)} aria-label={colorName(c)} onMouseDown={keep} onClick={() => { setTextColor(c); exec("foreColor", c); }}>
                  {swatch(c)}
                </button>
              ))}
            </PopoverSurface>
          </Popover>
        </span>
        <span className="fmt-split">
          {tool("highlight", <span className="fmt-color-icon highlight" style={{ background: highlight === "transparent" ? undefined : highlight }}>ab</span>, t("Surligner"), () => exec("hiliteColor", highlight), t("Surlignage"))}
          <Popover positioning="below-start" onOpenChange={(_, d) => !d.open && restore()}>
            <PopoverTrigger disableButtonEnhancement>
              <button type="button" className="fmt-drop" disabled={disabled} onMouseDown={keep} aria-label={t("Choisir la couleur de surlignage")}>
                <ChevronDown12Regular />
              </button>
            </PopoverTrigger>
            <PopoverSurface className="fmt-palette small">
              {HIGHLIGHTS.map((c) => (
                <button key={c} type="button" title={c === "transparent" ? t("Aucun") : c} onMouseDown={keep} onClick={() => { setHighlight(c); exec("hiliteColor", c); }}>
                  {c === "transparent" ? <span className="fmt-swatch none" /> : swatch(c)}
                </button>
              ))}
            </PopoverSurface>
          </Popover>
        </span>
      </div>

      <div className="fmt-group">
        {tool("bold", <TextBold20Regular />, t("Gras"), () => exec("bold"), t("Gras (Ctrl+B)"))}
        {tool("italic", <TextItalic20Regular />, t("Italique"), () => exec("italic"), t("Italique (Ctrl+I)"))}
        {tool("underline", <TextUnderline20Regular />, t("Souligné"), () => exec("underline"), t("Souligné (Ctrl+U)"))}
        {tool("strikeThrough", <TextStrikethrough20Regular />, t("Barré"), () => exec("strikeThrough"))}
        {tool("insertUnorderedList", <TextBulletListLtr20Regular />, t("Puces"), () => exec("insertUnorderedList"))}
        {tool("insertOrderedList", <TextNumberListLtr20Regular />, t("Numéros"), () => exec("insertOrderedList"), t("Numérotation"))}
      </div>

      <div className="fmt-group">
        {tool("justifyLeft", <TextAlignLeft20Regular />, t("Gauche"), () => exec("justifyLeft"), t("Aligner à gauche"))}
        {tool("justifyCenter", <TextAlignCenter20Regular />, t("Centre"), () => exec("justifyCenter"), t("Centrer"))}
        {tool("justifyRight", <TextAlignRight20Regular />, t("Droite"), () => exec("justifyRight"), t("Aligner à droite"))}
      </div>

      <div className="fmt-group">
        <Menu open={insertOpen} onOpenChange={(_, d) => { setInsertOpen(d.open); if (!d.open && !linkOpen) restore(); }}>
          <MenuTrigger disableButtonEnhancement>
            <button type="button" className="fmt-insert" disabled={disabled} onMouseDown={keep}>
              {t("Insérer…")} <ChevronDown12Regular />
            </button>
          </MenuTrigger>
          <MenuPopover>
            <MenuList>
              <MenuItem icon={<Image20Regular />} onClick={() => imagePicker.current?.click()}>
                {t("Image")}
              </MenuItem>
              <MenuItem
                icon={<Link20Regular />}
                onClick={() => {
                  const text = saved.current?.toString() ?? "";
                  setLinkUrl(/^https?:\/\//.test(text) ? text : "https://");
                  setLinkOpen(true);
                }}
              >
                {t("Lien")}
              </MenuItem>
              <Menu>
                <MenuTrigger disableButtonEnhancement>
                  <MenuItem icon={<Table20Regular />}>{t("Tableau")}</MenuItem>
                </MenuTrigger>
                <MenuPopover>
                  <TablePicker onPick={(r, c) => { setInsertOpen(false); insertTable(r, c); }} />
                </MenuPopover>
              </Menu>
            </MenuList>
          </MenuPopover>
        </Menu>
        <Popover open={linkOpen} onOpenChange={(_, d) => { setLinkOpen(d.open); if (!d.open) restore(); }} positioning="below-start" trapFocus>
          <PopoverTrigger disableButtonEnhancement>
            <span className="fmt-link-anchor" />
          </PopoverTrigger>
          <PopoverSurface className="fmt-link">
            <Input
              autoFocus
              value={linkUrl}
              onChange={(_, d) => setLinkUrl(d.value)}
              onKeyDown={(e) => e.key === "Enter" && (e.preventDefault(), applyLink())}
              placeholder="https://"
              aria-label={t("Adresse du lien")}
            />
            <Button appearance="primary" onClick={applyLink}>
              {t("Insérer")}
            </Button>
          </PopoverSurface>
        </Popover>
        <input
          ref={imagePicker}
          type="file"
          accept="image/*"
          hidden
          onChange={(e) => {
            const f = e.target.files?.[0];
            if (f) insertImage(f);
            e.target.value = "";
          }}
        />
      </div>

      <div className="fmt-group">
        {tool("attach", <Attach20Regular />, t("Pièce jointe"), onAttach, t("Joindre des fichiers"))}
        {tool("quote", <TextQuote20Regular />, t("Citation"), toggleQuote)}
        {tool("clear", <TextClearFormatting20Regular />, t("Effacer"), clearFormatting, t("Effacer la mise en forme"))}
      </div>
      {formatSwitch}
    </div>
  );
}

/** Grille de choix de la taille du tableau, au survol. */
function TablePicker({ onPick }: { onPick: (rows: number, cols: number) => void }) {
  const [hover, setHover] = useState<[number, number]>([0, 0]);
  return (
    <div className="fmt-table-picker">
      <div className="grid" onMouseLeave={() => setHover([0, 0])}>
        {Array.from({ length: 8 }, (_, r) =>
          Array.from({ length: 8 }, (_, c) => (
            <button
              key={`${r}-${c}`}
              type="button"
              className={r < hover[0] && c < hover[1] ? "on" : ""}
              onMouseEnter={() => setHover([r + 1, c + 1])}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => onPick(r + 1, c + 1)}
              aria-label={`${r + 1} × ${c + 1}`}
            />
          )),
        )}
      </div>
      <div className="label">{hover[0] ? `${hover[0]} × ${hover[1]}` : t("Tableau")}</div>
    </div>
  );
}
