// Contacts : liste à gauche (recherche, regroupée par initiale), fiche à droite.
import { Avatar, Button, Field, Input, SearchBox, Select, Spinner, Textarea } from "@fluentui/react-components";
import { Add20Regular, Delete20Regular, Mail20Regular, PersonAdd20Regular, Save20Regular } from "@fluentui/react-icons";
import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { locale, t } from "../i18n";
import type { Account, Contact, PimCollection } from "../types";
import { contactName, useGeneral } from "../general";
import { ConfirmDialog } from "./Dialogs";
import { PimTransfer } from "./PimTransfer";

interface Props {
  accounts: Account[];
  /** Écrire à ce contact (nouveau message). */
  onWrite: (email: string, name: string) => void;
  notify: (title: string, body?: string, intent?: "success" | "error" | "info") => void;
  /** Incrémenté après une synchronisation : la liste est relue. */
  version: number;
}

const SOURCE_KEY = "vortex.contacts.source";

const empty = (): Contact => ({
  id: "",
  source: "local",
  uid: "",
  name: "",
  givenName: "",
  familyName: "",
  emails: [""],
  phones: [""],
  organization: "",
  note: "",
});

/** Prénom et nom de la fiche ; à défaut, déduits du nom affiché (dernier mot = nom). */
function nameParts(c: Contact): { givenName: string; familyName: string } {
  if (c.givenName?.trim() || c.familyName?.trim()) return { givenName: c.givenName ?? "", familyName: c.familyName ?? "" };
  const name = c.name.trim();
  const cut = name.lastIndexOf(" ");
  return cut < 0 ? { givenName: "", familyName: name } : { givenName: name.slice(0, cut), familyName: name.slice(cut + 1) };
}

export function ContactsView({ accounts, onWrite, notify, version }: Props) {
  const { nameOrder } = useGeneral();
  const display = (c: Contact) => contactName(c, nameOrder);
  /** Nom de la fiche en cours de saisie, dans l'ordre choisi. */
  const liveLabel = (c: Contact) => {
    const given = (c.givenName ?? "").trim();
    const family = (c.familyName ?? "").trim();
    if (given && family) return contactName({ name: c.name, givenName: given, familyName: family }, nameOrder);
    return given || family || c.name;
  };
  const [list, setList] = useState<Contact[] | null>(null);
  const [books, setBooks] = useState<PimCollection[]>([]);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [draft, setDraft] = useState<Contact | null>(null);
  const [book, setBook] = useState("local:contacts");
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  /**
   * Prénom et nom montrés à l'ouverture de la fiche : tant qu'ils ne changent
   * pas, le nom affiché d'origine est gardé (« Dupont, Jean (ACME) »,
   * « Mairie de Lyon »), et une fiche sans prénom ni nom séparés n'en reçoit
   * pas de déduits (relecture du 12 novembre, n° 43).
   */
  const [opened, setOpened] = useState({ givenName: "", familyName: "", derived: false });
  /** Compte affiché (« all » : tous), retenu d'une ouverture à l'autre. */
  const [source, setSourceState] = useState<string>(() => {
    try {
      return localStorage.getItem(SOURCE_KEY) ?? "all";
    } catch {
      return "all";
    }
  });
  const setSource = (value: string) => {
    setSourceState(value);
    try {
      localStorage.setItem(SOURCE_KEY, value);
    } catch {
      /* stockage indisponible : choix gardé pour cette session */
    }
  };

  const reload = async () => {
    const [contacts, collections] = await Promise.all([api.contacts(), api.pimCollections()]);
    setList(contacts);
    setBooks(collections.filter((c) => c.kind === "contacts"));
  };

  useEffect(() => {
    reload().catch((e) => notify(t("Contacts illisibles"), String(e), "error"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [version]);

  const accountName = (id: string) => (id === "local" ? t("Sur cet ordinateur") : (accounts.find((a) => a.id === id)?.name ?? id));
  const bookLabel = (c: PimCollection) => (c.account === "local" ? c.name : `${c.name} — ${accountName(c.account)}`);

  /** Comptes ayant un carnet ou des contacts, dans l'ordre des comptes, « Sur cet ordinateur » en dernier. */
  const sources = useMemo(() => {
    const ids = new Set([...books.map((b) => b.account), ...(list ?? []).map((c) => c.source)]);
    const ordered = accounts.filter((a) => ids.has(a.id)).map((a) => a.id);
    if (ids.has("local")) ordered.push("local");
    return ordered.map((id) => ({
      id,
      name: accountName(id),
      color: id === "local" ? "#6b7280" : (accounts.find((a) => a.id === id)?.color ?? "#6b7280"),
      count: (list ?? []).filter((c) => c.source === id).length,
    }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [books, list, accounts]);
  const current = source === "all" || sources.some((x) => x.id === source) ? source : "all";
  const colorOf = (id: string) => sources.find((x) => x.id === id)?.color;

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    const all = (list ?? []).filter((c) => current === "all" || c.source === current);
    return q
      ? all.filter((c) =>
          [c.name, c.givenName ?? "", c.familyName ?? "", c.organization, ...c.emails, ...c.phones].some((s) =>
            s.toLowerCase().includes(q),
          ),
        )
      : all;
  }, [list, query, current]);

  const groups = useMemo(() => {
    const map = new Map<string, Contact[]>();
    // Tri et initiales selon l'ordre choisi (Paramètres › Général).
    const sorted = shown
      .map((c) => ({ c, label: display(c) || c.emails[0] || "#" }))
      .sort((a, b) => a.label.localeCompare(b.label, locale(), { sensitivity: "base" }));
    for (const { c, label } of sorted) {
      const letter = label.trim().charAt(0).toUpperCase();
      const key = /\p{L}/u.test(letter) ? letter.normalize("NFD").charAt(0) : "#";
      map.set(key, [...(map.get(key) ?? []), c]);
    }
    return [...map.entries()].sort(([a], [b]) => a.localeCompare(b, locale()));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shown, nameOrder]);

  /** Fiche affichée dans le formulaire, prénom et nom déduits si besoin. */
  const show = (c: Contact) => {
    const parts = nameParts(c);
    setOpened({ ...parts, derived: !c.givenName?.trim() && !c.familyName?.trim() });
    setDraft({ ...c, ...parts, emails: c.emails.length ? c.emails : [""], phones: c.phones.length ? c.phones : [""] });
  };

  const open = (c: Contact) => {
    setSelected(c.id);
    show(c);
    // Carnet d'origine : celui dont l'adresse préfixe celle de la fiche.
    const where = books.find((b) => b.account === c.source && c.id.startsWith(`${b.account}|${b.url}`));
    setBook(c.source === "local" ? "local:contacts" : (where ?? books.find((b) => b.account === c.source))?.url ?? "local:contacts");
  };

  const create = () => {
    setSelected(null);
    setOpened({ givenName: "", familyName: "", derived: false });
    setDraft(empty());
    // Carnet du compte affiché, sinon le premier carnet synchronisé.
    const preferred = current !== "all" ? books.find((b) => b.account === current) : undefined;
    setBook((preferred ?? books.find((b) => b.account !== "local"))?.url ?? "local:contacts");
  };

  const save = async () => {
    if (!draft) return;
    const givenName = (draft.givenName ?? "").trim();
    const familyName = (draft.familyName ?? "").trim();
    const unchanged = sameNames;
    const contact = {
      ...draft,
      givenName: unchanged && opened.derived ? "" : givenName,
      familyName: unchanged && opened.derived ? "" : familyName,
      // Nom affiché d'origine gardé si prénom et nom n'ont pas changé ; sinon
      // « Prénom Nom » (usage vCard). L'ordre à l'écran suit le réglage.
      name: unchanged && draft.name.trim() ? draft.name : [givenName, familyName].filter(Boolean).join(" "),
      emails: draft.emails.map((e) => e.trim()).filter(Boolean),
      phones: draft.phones.map((p) => p.trim()).filter(Boolean),
    };
    if (!contact.name && contact.emails.length === 0) return notify(t("Contact vide"), t("Indiquez au moins un nom ou une adresse."), "info");
    if (!contact.name) contact.name = contact.emails[0];
    setBusy(true);
    try {
      const saved = await api.saveContact(contact, book);
      await reload();
      setSelected(saved.id);
      show(saved);
      notify(t("Contact enregistré"), saved.name);
    } catch (e) {
      notify(t("Contact non enregistré"), String(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!draft?.id) return setDraft(null);
    setBusy(true);
    try {
      await api.pimDelete(draft.id);
      await reload();
      setDraft(null);
      setSelected(null);
      notify(t("Contact supprimé"), draft.name);
    } catch (e) {
      notify(t("Suppression impossible"), String(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const setList_ = (field: "emails" | "phones", index: number, value: string) =>
    setDraft((d) => (d ? { ...d, [field]: d[field].map((x, i) => (i === index ? value : x)) } : d));

  const sameNames =
    !!draft?.id && (draft.givenName ?? "").trim() === opened.givenName.trim() && (draft.familyName ?? "").trim() === opened.familyName.trim();
  // Prénom et nom inchangés : le nom tel qu'enregistré (celui de la liste).
  const draftLabel = !draft
    ? ""
    : sameNames && opened.derived
      ? draft.name
      : sameNames
        ? display(draft)
        : liveLabel(draft);
  return (
    <div className="pim-view contacts-view">
      <section className="pane contacts-list">
        <div className="pim-toolbar">
          <Button appearance="primary" icon={<PersonAdd20Regular />} onClick={create}>
            {t("Nouveau contact")}
          </Button>
          <PimTransfer
            kind="contacts"
            collections={books}
            accounts={accounts}
            notify={notify}
            onImported={() => void reload().catch((e) => notify(t("Contacts illisibles"), String(e), "error"))}
          />
        </div>
        <SearchBox className="pim-search" placeholder={t("Rechercher un contact")} value={query} onChange={(_, d) => setQuery(d.value)} />
        <div className="contacts-scroll">
          {list === null && <Spinner size="small" className="list-loading" />}
          {list !== null && shown.length === 0 && <div className="list-empty">{query ? t("Aucun résultat.") : t("Aucun contact.")}</div>}
          {groups.map(([letter, items]) => (
            <div key={letter}>
              <div className="group-label">{letter}</div>
              {items.map((c) => (
                <button key={c.id} type="button" className={c.id === selected ? "contact-row selected" : "contact-row"} onClick={() => open(c)}>
                  <Avatar name={display(c)} size={32} color="colorful" />
                  <span className="contact-text">
                    <span className="contact-name">
                      {current === "all" && sources.length > 1 && (
                        <span className="account-dot" style={{ background: colorOf(c.source) }} title={accountName(c.source)} />
                      )}
                      {display(c)}
                    </span>
                    <span className="muted">{c.emails[0] ?? c.organization}</span>
                  </span>
                </button>
              ))}
            </div>
          ))}
        </div>
      </section>

      <section className="pane contact-detail">
          {sources.length > 1 && (
            <div className="contact-sources" role="tablist" aria-label={t("Compte")}>
              <button
                type="button"
                role="tab"
                aria-selected={current === "all"}
                className={current === "all" ? "contact-source on" : "contact-source"}
                onClick={() => setSource("all")}
              >
                {t("Tous")} <span className="contact-count">{list?.length ?? 0}</span>
              </button>
              {sources.map((x) => (
                <button
                  key={x.id}
                  type="button"
                  role="tab"
                  aria-selected={current === x.id}
                  className={current === x.id ? "contact-source on" : "contact-source"}
                  onClick={() => setSource(x.id)}
                  title={x.name}
                >
                  <span className="account-dot" style={{ background: x.color }} />
                  <span className="contact-source-name">{x.name}</span>
                  <span className="contact-count">{x.count}</span>
                </button>
              ))}
            </div>
          )}
        <div className="contact-detail-body">
        {!draft ? (
          <div className="reading-empty">
            <PersonAdd20Regular />
            <p>{t("Sélectionnez un contact")}</p>
            <p className="muted">{t("ou créez-en un nouveau")}</p>
          </div>
        ) : (
          <div className="contact-form">
            <div className="contact-head">
              <Avatar name={draftLabel || "?"} size={56} color="colorful" />
              <h2>{draftLabel || t("Nouveau contact")}</h2>
              <span className="contact-actions">
                {draft.emails[0] && (
                  <Button icon={<Mail20Regular />} onClick={() => onWrite(draft.emails[0], draft.name)}>
                    {t("Écrire")}
                  </Button>
                )}
                <Button appearance="primary" icon={busy ? <Spinner size="tiny" /> : <Save20Regular />} disabled={busy} onClick={save}>
                  {t("Enregistrer")}
                </Button>
                <Button icon={<Delete20Regular />} disabled={busy} onClick={() => (draft.id ? setConfirmDelete(true) : remove())}>
                  {draft.id ? t("Supprimer") : t("Annuler")}
                </Button>
              </span>
            </div>
            <Field label={t("Carnet d'adresses")}>
              <Select value={book} onChange={(_, d) => setBook(d.value)}>
                {books.map((b) => (
                  <option key={b.url} value={b.url}>
                    {bookLabel(b)}
                  </option>
                ))}
              </Select>
            </Field>
            <div className={nameOrder === "last-first" ? "contact-names reversed" : "contact-names"}>
              <Field label={t("Prénom")}>
                <Input
                  value={draft.givenName ?? ""}
                  onChange={(_, d) => setDraft({ ...draft, givenName: d.value })}
                  autoFocus={!draft.id && nameOrder === "first-last"}
                />
              </Field>
              <Field label={t("Nom")}>
                <Input
                  value={draft.familyName ?? ""}
                  onChange={(_, d) => setDraft({ ...draft, familyName: d.value })}
                  autoFocus={!draft.id && nameOrder === "last-first"}
                />
              </Field>
            </div>
            <Field label={t("Société")}>
              <Input value={draft.organization} onChange={(_, d) => setDraft({ ...draft, organization: d.value })} />
            </Field>
            {(["emails", "phones"] as const).map((field) => (
              <Field key={field} label={field === "emails" ? t("Adresses e-mail") : t("Téléphones")}>
                <div className="contact-multi">
                  {draft[field].map((v, i) => (
                    <Input key={i} value={v} type={field === "emails" ? "email" : "tel"} onChange={(_, d) => setList_(field, i, d.value)} />
                  ))}
                  <Button size="small" appearance="transparent" icon={<Add20Regular />} onClick={() => setDraft({ ...draft, [field]: [...draft[field], ""] })}>
                    {field === "emails" ? t("Ajouter une adresse") : t("Ajouter un numéro")}
                  </Button>
                </div>
              </Field>
            ))}
            <Field label={t("Notes")}>
              <Textarea value={draft.note} rows={4} resize="vertical" onChange={(_, d) => setDraft({ ...draft, note: d.value })} />
            </Field>
          </div>
        )}
        </div>
      </section>
      {confirmDelete && draft && (
        <ConfirmDialog
          title={t("Supprimer le contact")}
          message={t("La fiche de « {name} » sera supprimée.", { name: draftLabel || draft.emails[0] || t("(sans titre)") })}
          confirm={t("Supprimer")}
          onConfirm={remove}
          onClose={() => setConfirmDelete(false)}
        />
      )}
    </div>
  );
}
