// Champ de destinataires avec complétion : contacts et correspondants des messages.
import { Input } from "@fluentui/react-components";
import { Person16Regular, PersonMail16Regular } from "@fluentui/react-icons";
import { forwardRef, useEffect, useRef, useState } from "react";
import { api } from "../api";
import { mailboxEntry, splitAddressList } from "../compose";
import type { Suggestion } from "../types";

interface Props {
  id?: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  "aria-label"?: string;
}

/** Adresse en cours de saisie : après le dernier séparateur hors guillemets. */
function currentToken(value: string): { head: string; token: string } {
  const parts = splitAddressList(value);
  const last = parts[parts.length - 1];
  const head = value.slice(0, value.length - last.length);
  return { head, token: last.trim() };
}

export const AddressInput = forwardRef<HTMLInputElement, Props>(function AddressInput(p, ref) {
  const [items, setItems] = useState<Suggestion[]>([]);
  const [active, setActive] = useState(0);
  const [open, setOpen] = useState(false);
  const seq = useRef(0);

  useEffect(() => {
    const { token } = currentToken(p.value);
    if (token.length < 2 || token.includes("@") && token.endsWith(">")) {
      setItems([]);
      return;
    }
    const n = ++seq.current;
    const timer = window.setTimeout(() => {
      api.suggestAddresses(token).then(
        (list) => {
          if (n !== seq.current) return;
          setItems(list);
          setActive(0);
        },
        () => {},
      );
    }, 120);
    return () => window.clearTimeout(timer);
  }, [p.value]);

  const pick = (s: Suggestion) => {
    const { head } = currentToken(p.value);
    // Nom entre guillemets s'il contient une virgule (« Dupont, Marie ») : sinon
    // la liste serait coupée à la virgule (relecture complète n° 12).
    const entry = mailboxEntry(s.name, s.email);
    p.onChange(`${head}${head ? " " : ""}${entry}; `);
    setItems([]);
    setOpen(false);
  };

  const shown = open && items.length > 0;

  return (
    <div className="address-field">
      <Input
        id={p.id}
        ref={ref}
        appearance="underline"
        value={p.value}
        placeholder={p.placeholder}
        aria-label={p["aria-label"]}
        autoComplete="off"
        onChange={(_, d) => {
          p.onChange(d.value);
          setOpen(true);
        }}
        onBlur={() => window.setTimeout(() => setOpen(false), 150)}
        onKeyDown={(e) => {
          if (!shown) return;
          if (e.key === "ArrowDown") {
            e.preventDefault();
            setActive((a) => (a + 1) % items.length);
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setActive((a) => (a - 1 + items.length) % items.length);
          } else if (e.key === "Enter" || e.key === "Tab") {
            e.preventDefault();
            pick(items[active]);
          } else if (e.key === "Escape") {
            setOpen(false);
          }
        }}
      />
      {shown && (
        <div className="address-suggestions" role="listbox">
          {items.map((s, i) => (
            <button
              key={s.email}
              type="button"
              role="option"
              aria-selected={i === active}
              className={i === active ? "on" : undefined}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => pick(s)}
            >
              {s.contact ? <Person16Regular /> : <PersonMail16Regular />}
              <span>{s.name || s.email}</span>
              {s.name && <span className="muted">{s.email}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
});
