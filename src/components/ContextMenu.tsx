import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

export interface ContextMenuItem {
  label: string;
  icon?: ReactNode;
  danger?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  /**
   * Sous-menu ouvert à droite au survol de l'entrée (lot Q : « Déplacer
   * vers… ») ; reçoit de quoi fermer tout le menu une fois le choix fait.
   */
  panel?: (close: () => void) => ReactNode;
}

interface Props {
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
}

/** Menu contextuel (clic droit), placé au curseur et gardé dans la fenêtre. */
export function ContextMenu({ x, y, items, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef(new Map<string, HTMLButtonElement>());
  const [pos, setPos] = useState({ left: x, top: y });
  /** Entrée dont le sous-menu est ouvert. */
  const [open, setOpen] = useState<string | null>(null);
  const [panelPos, setPanelPos] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    setPos({
      left: Math.min(x, window.innerWidth - width - 8),
      top: Math.min(y, window.innerHeight - height - 8),
    });
  }, [x, y]);

  // Sous-menu à droite du menu, à hauteur de son entrée ; à gauche, ou plus
  // haut, s'il sortirait de la fenêtre.
  useLayoutEffect(() => {
    const menu = ref.current;
    const panel = panelRef.current;
    const item = open ? itemRefs.current.get(open) : undefined;
    if (!menu || !panel || !item) {
      setPanelPos(null);
      return;
    }
    const m = menu.getBoundingClientRect();
    const { width, height } = panel.getBoundingClientRect();
    const right = m.right - 4;
    const left = right + width > window.innerWidth - 8 ? Math.max(8, m.left - width + 4) : right;
    const top = Math.max(8, Math.min(item.getBoundingClientRect().top - 4, window.innerHeight - height - 8));
    setPanelPos({ left: left - m.left, top: top - m.top });
  }, [open, pos]);

  useEffect(() => {
    const close = (e: Event) => {
      if (e.type === "keydown" && (e as KeyboardEvent).key !== "Escape") return;
      if (e.type === "mousedown" && ref.current?.contains(e.target as Node)) return;
      onClose();
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", close);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  }, [onClose]);

  const openItem = items.find((item) => item.label === open && item.panel && !item.disabled);

  return (
    <div
      ref={ref}
      className="context-menu"
      role="menu"
      tabIndex={-1}
      style={pos}
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((item) => (
        <button
          key={item.label}
          ref={(el) => {
            if (el) itemRefs.current.set(item.label, el);
            else itemRefs.current.delete(item.label);
          }}
          type="button"
          role="menuitem"
          aria-haspopup={item.panel ? "true" : undefined}
          aria-expanded={item.panel ? open === item.label : undefined}
          className={[
            "context-item",
            item.danger ? "danger" : "",
            item.panel ? "has-panel" : "",
            open === item.label ? "open" : "",
          ]
            .filter(Boolean)
            .join(" ")}
          disabled={item.disabled}
          onMouseEnter={() => setOpen(item.panel ? item.label : null)}
          onClick={() => {
            if (item.panel) {
              setOpen(item.label);
              return;
            }
            onClose();
            item.onClick?.();
          }}
        >
          <span className="context-icon">{item.icon}</span>
          {item.label}
          {item.panel && <span className="context-chevron" aria-hidden="true">›</span>}
        </button>
      ))}
      {openItem?.panel && (
        <div
          ref={panelRef}
          className="context-panel"
          style={panelPos ?? { visibility: "hidden" }}
          onMouseEnter={() => setOpen(openItem.label)}
        >
          {openItem.panel(onClose)}
        </div>
      )}
    </div>
  );
}
