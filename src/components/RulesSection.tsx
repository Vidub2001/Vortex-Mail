// Paramètres › Règles : liste ordonnée des règles, éditeur, application à un dossier.
import { Button, Checkbox, Field, Input, MessageBar, MessageBarBody, Select, Spinner, Switch } from "@fluentui/react-components";
import {
  Add20Regular,
  ArrowDown20Regular,
  ArrowUp20Regular,
  Delete20Regular,
  Dismiss20Regular,
  Edit20Regular,
  Play20Regular,
} from "@fluentui/react-icons";
import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { t } from "../i18n";
import {
  ACTIONS,
  ATTACHMENT_OPS,
  FIELDS,
  TEXT_OPS,
  folderLabel,
  newRule,
  ruleReady,
  runRulesOnFolder,
  saveRules,
  useRules,
  type Rule,
  type RuleAction,
  type RuleCondition,
} from "../rules";
import type { Account, Folder } from "../types";

interface Props {
  accounts: Account[];
  /** Règle à créer, préremplie depuis un message. */
  initialRule?: Rule | null;
  notify: (title: string, body?: string, intent?: "success" | "error" | "info") => void;
}

export function RulesSection({ accounts, notify, initialRule = null }: Props) {
  const rules = useRules();
  const [folders, setFolders] = useState<Folder[]>([]);
  const [editing, setEditing] = useState<Rule | null>(initialRule);
  const [target, setTarget] = useState("");
  const [running, setRunning] = useState(false);

  useEffect(() => {
    api
      .listFolders()
      .then((l) => {
        setFolders(l.folders);
        setTarget((t) => t || l.folders.find((f) => f.role === "inbox")?.id || "");
      })
      .catch(() => setFolders([]));
  }, []);

  const accountName = (id: string) => accounts.find((a) => a.id === id)?.name ?? id;
  const labelOf = (id: string | undefined) => {
    const f = folders.find((x) => x.id === id);
    return f ? folderLabel(f, folders, accountName) : t("(dossier introuvable)");
  };

  /** Résumé lisible d'une règle. */
  const summary = (r: Rule) => {
    const conds = r.conditions
      .filter((c) => c.field === "attachment" || c.value.trim())
      .map((c) => {
        const field = FIELDS.find((f) => f.value === c.field)?.label.toLowerCase() ?? "";
        if (c.field === "attachment") return c.op === "no" ? t("pièce jointe absente") : t("pièce jointe présente");
        return t("{field} {op} « {value} »", { field, op: TEXT_OPS.find((o) => o.value === c.op)?.label ?? "", value: c.value.trim() });
      })
      .join(r.match === "all" ? ` ${t("et")} ` : ` ${t("ou")} `);
    const acts = r.actions
      .map((a) =>
        a.type === "move" ? t("déplacer vers {folder}", { folder: labelOf(a.folderId) }) : (ACTIONS.find((x) => x.value === a.type)?.label.toLowerCase() ?? ""),
      )
      .join(", ");
    return t("Si {conditions} → {actions}", { conditions: conds || "…", actions: acts || "…" });
  };

  const put = (rule: Rule) => {
    const exists = rules.some((r) => r.id === rule.id);
    saveRules(exists ? rules.map((r) => (r.id === rule.id ? rule : r)) : [...rules, rule]);
  };

  const moveRule = (index: number, delta: number) => {
    const list = [...rules];
    const [r] = list.splice(index, 1);
    list.splice(index + delta, 0, r);
    saveRules(list);
  };

  const runNow = async () => {
    const folder = folders.find((f) => f.id === target);
    if (!folder) return;
    setRunning(true);
    try {
      const report = await runRulesOnFolder(folder);
      if (report.failed.length) notify(t("Règles appliquées en partie"), report.failed.slice(0, 3).join("\n"), "error");
      else
        notify(
          t("Règles appliquées"),
          report.matched === 0
            ? t("Aucun message concerné.")
            : report.matched > 1
              ? report.moved > 1
                ? t("{matched} messages concernés, {moved} déplacés.", { matched: report.matched, moved: report.moved })
                : t("{matched} messages concernés, {moved} déplacé.", { matched: report.matched, moved: report.moved })
              : report.moved > 1
                ? t("1 message concerné, {moved} déplacés.", { moved: report.moved })
                : t("1 message concerné, {moved} déplacé.", { moved: report.moved }),
        );
      window.dispatchEvent(new Event("vortex:rules-applied"));
    } catch (e) {
      notify(t("Règles non appliquées"), String(e), "error");
    } finally {
      setRunning(false);
    }
  };

  if (editing) {
    return (
      <RuleEditor
        rule={editing}
        isNew={!rules.some((x) => x.id === editing.id)}
        accounts={accounts}
        folders={folders}
        accountName={accountName}
        onCancel={() => setEditing(null)}
        onSave={(r) => {
          put(r);
          setEditing(null);
        }}
      />
    );
  }

  return (
    <div className="settings-pane">
      <h3 className="settings-pane-title">{t("Règles")}</h3>
      <div className="about-scroll">
        <p className="muted settings-hint rules-intro">
          {t(
            "Les règles classent les nouveaux messages des boîtes de réception dès leur arrivée, dans l'ordre de la liste. Le déplacement peut viser un dossier d'un autre compte.",
          )}
        </p>
        <div className="rules-toolbar">
          <Button appearance="primary" icon={<Add20Regular />} onClick={() => setEditing(newRule())}>
            {t("Nouvelle règle")}
          </Button>
        </div>

        {rules.length === 0 ? (
          <p className="muted rules-empty">{t("Aucune règle pour l'instant.")}</p>
        ) : (
          <ol className="rules-list">
            {rules.map((r, i) => (
              <li key={r.id} className={r.enabled ? "rule-row" : "rule-row off"}>
                <Switch
                  checked={r.enabled}
                  onChange={(_, d) => put({ ...r, enabled: d.checked })}
                  aria-label={r.enabled ? t("Désactiver") : t("Activer")}
                />
                <button type="button" className="rule-text" onClick={() => setEditing(r)}>
                  <span className="rule-name">
                    {r.name || t("Règle sans nom")}
                    {r.account !== "all" && <span className="rule-account"> · {accountName(r.account)}</span>}
                    {r.enabled && !ruleReady(r) && <span className="rule-warning"> · {t("incomplète")}</span>}
                  </span>
                  <span className="muted rule-summary">{summary(r)}</span>
                </button>
                <span className="rule-buttons">
                  <Button appearance="subtle" size="small" icon={<ArrowUp20Regular />} disabled={i === 0} onClick={() => moveRule(i, -1)} aria-label={t("Monter")} title={t("Monter")} />
                  <Button
                    appearance="subtle"
                    size="small"
                    icon={<ArrowDown20Regular />}
                    disabled={i === rules.length - 1}
                    onClick={() => moveRule(i, 1)}
                    aria-label={t("Descendre")}
                    title={t("Descendre")}
                  />
                  <Button appearance="subtle" size="small" icon={<Edit20Regular />} onClick={() => setEditing(r)} aria-label={t("Modifier")} title={t("Modifier")} />
                  <Button
                    appearance="subtle"
                    size="small"
                    icon={<Delete20Regular />}
                    onClick={() => saveRules(rules.filter((x) => x.id !== r.id))}
                    aria-label={t("Supprimer")}
                    title={t("Supprimer")}
                  />
                </span>
              </li>
            ))}
          </ol>
        )}

        <fieldset className="account-fieldset">
          <legend>{t("Appliquer maintenant")}</legend>
          <p className="muted settings-hint">{t("Passe tous les messages d'un dossier par les règles actives.")}</p>
          <div className="rules-run">
            <FolderSelect folders={folders} accountName={accountName} value={target} onChange={setTarget} />
            <Button icon={running ? <Spinner size="tiny" /> : <Play20Regular />} disabled={running || !target || rules.length === 0} onClick={runNow}>
              {t("Appliquer")}
            </Button>
          </div>
        </fieldset>
      </div>
    </div>
  );
}

/** Liste des dossiers, groupés par compte, en arborescence. */
function FolderSelect({
  folders,
  accountName,
  value,
  onChange,
  placeholder,
}: {
  folders: Folder[];
  accountName: (id: string) => string;
  value: string;
  onChange: (id: string) => void;
  placeholder?: string;
}) {
  const groups = useMemo(() => {
    const map = new Map<string, Folder[]>();
    for (const f of folders) map.set(f.accountId, [...(map.get(f.accountId) ?? []), f]);
    return [...map.entries()];
  }, [folders]);
  return (
    <Select value={value} onChange={(_, d) => onChange(d.value)} className="rules-folder" aria-label={t("Dossier")}>
      {placeholder !== undefined && <option value="">{placeholder}</option>}
      {groups.map(([account, list]) => (
        <optgroup key={account} label={accountName(account)}>
          {list.map((f) => (
            <option key={f.id} value={f.id}>
              {"  ".repeat(f.depth)}
              {f.name}
            </option>
          ))}
        </optgroup>
      ))}
    </Select>
  );
}

function RuleEditor({
  rule,
  isNew,
  accounts,
  folders,
  accountName,
  onCancel,
  onSave,
}: {
  rule: Rule;
  isNew: boolean;
  accounts: Account[];
  folders: Folder[];
  accountName: (id: string) => string;
  onCancel: () => void;
  onSave: (r: Rule) => void;
}) {
  const [r, setR] = useState<Rule>(rule);
  const [error, setError] = useState<string | null>(null);
  const set = (patch: Partial<Rule>) => setR((cur) => ({ ...cur, ...patch }));
  const setCondition = (i: number, patch: Partial<RuleCondition>) =>
    set({ conditions: r.conditions.map((c, j) => (j === i ? { ...c, ...patch } : c)) });
  const setAction = (i: number, patch: Partial<RuleAction>) => set({ actions: r.actions.map((a, j) => (j === i ? { ...a, ...patch } : a)) });

  const save = () => {
    const conditions = r.conditions.filter((c) => c.field === "attachment" || c.value.trim() !== "");
    const actions = r.actions.filter((a) => a.type !== "move" || !!a.folderId);
    if (conditions.length === 0) return setError(t("Indiquez au moins une condition (avec une valeur)."));
    if (actions.length === 0) return setError(t("Choisissez au moins une action (et le dossier de destination)."));
    const name = r.name.trim() || conditions.map((c) => (c.field === "attachment" ? t("pièce jointe") : c.value.trim())).join(", ");
    onSave({ ...r, name, conditions, actions });
  };

  return (
    <div className="settings-pane">
      <h3 className="settings-pane-title">{isNew ? t("Nouvelle règle") : t("Modifier la règle")}</h3>
      <div className="about-scroll rule-editor">
        <div className="settings-row">
          <Field label={t("Nom")}>
            <Input value={r.name} onChange={(_, d) => set({ name: d.value })} placeholder={t("ex. Factures")} autoFocus />
          </Field>
          <Field label={t("Messages reçus par")}>
            <Select value={r.account} onChange={(_, d) => set({ account: d.value })}>
              <option value="all">{t("Tous les comptes")}</option>
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name} — {a.email}
                </option>
              ))}
            </Select>
          </Field>
        </div>

        <fieldset className="account-fieldset">
          <legend>{t("Conditions")}</legend>
          <div className="rule-match">
            <span>{t("Si")}</span>
            <Select value={r.match} onChange={(_, d) => set({ match: d.value as Rule["match"] })} aria-label={t("Combinaison")}>
              <option value="all">{t("toutes les conditions")}</option>
              <option value="any">{t("au moins une condition")}</option>
            </Select>
            <span>{t("sont remplies :")}</span>
          </div>
          {r.conditions.map((c, i) => (
            <div key={i} className="rule-line">
              <Select
                value={c.field}
                onChange={(_, d) => {
                  const field = d.value as RuleCondition["field"];
                  setCondition(i, { field, op: field === "attachment" ? "yes" : c.op === "yes" || c.op === "no" ? "contains" : c.op });
                }}
                aria-label={t("Champ")}
              >
                {FIELDS.map((f) => (
                  <option key={f.value} value={f.value}>
                    {f.label}
                  </option>
                ))}
              </Select>
              <Select value={c.op} onChange={(_, d) => setCondition(i, { op: d.value as RuleCondition["op"] })} aria-label={t("Opérateur")}>
                {(c.field === "attachment" ? ATTACHMENT_OPS : TEXT_OPS).map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </Select>
              {c.field === "attachment" ? (
                <span />
              ) : (
                <Input value={c.value} onChange={(_, d) => setCondition(i, { value: d.value })} placeholder={t("texte")} aria-label={t("Valeur")} />
              )}
              <Button
                appearance="subtle"
                icon={<Dismiss20Regular />}
                disabled={r.conditions.length === 1}
                onClick={() => set({ conditions: r.conditions.filter((_, j) => j !== i) })}
                aria-label={t("Retirer la condition")}
                title={t("Retirer")}
              />
            </div>
          ))}
          <Button
            appearance="subtle"
            icon={<Add20Regular />}
            className="rule-add"
            onClick={() => set({ conditions: [...r.conditions, { field: "subject", op: "contains", value: "" }] })}
          >
            {t("Ajouter une condition")}
          </Button>
          <p className="muted settings-hint">
            {t("Majuscules et accents ne comptent pas. « Début du message » : les premières lignes du texte.")}
          </p>
        </fieldset>

        <fieldset className="account-fieldset">
          <legend>{t("Actions")}</legend>
          {r.actions.map((a, i) => (
            <div key={i} className="rule-line actions">
              <Select
                value={a.type}
                onChange={(_, d) => setAction(i, { type: d.value as RuleAction["type"], folderId: a.folderId ?? "" })}
                aria-label={t("Action")}
              >
                {ACTIONS.map((x) => (
                  <option key={x.value} value={x.value}>
                    {x.label}
                  </option>
                ))}
              </Select>
              {a.type === "move" ? (
                <FolderSelect
                  folders={folders}
                  accountName={accountName}
                  value={a.folderId ?? ""}
                  onChange={(id) => setAction(i, { folderId: id })}
                  placeholder={t("Choisir un dossier…")}
                />
              ) : (
                <span />
              )}
              <Button
                appearance="subtle"
                icon={<Dismiss20Regular />}
                disabled={r.actions.length === 1}
                onClick={() => set({ actions: r.actions.filter((_, j) => j !== i) })}
                aria-label={t("Retirer l'action")}
                title={t("Retirer")}
              />
            </div>
          ))}
          <Button
            appearance="subtle"
            icon={<Add20Regular />}
            className="rule-add"
            onClick={() => set({ actions: [...r.actions, { type: "markRead" }] })}
          >
            {t("Ajouter une action")}
          </Button>
          {r.actions.some((a) => a.type === "move" && folders.find((f) => f.id === a.folderId)?.role === "inbox") && (
            <p className="muted settings-hint">
              {t("Vers une boîte de réception : les règles de cette boîte ne s'appliqueront pas à ces messages, pour éviter qu'ils circulent d'une boîte à l'autre.")}
            </p>
          )}
          <Checkbox
            checked={r.stop}
            onChange={(_, d) => set({ stop: d.checked === true })}
            label={t("Ne pas appliquer les règles suivantes à ces messages")}
          />
          <Checkbox checked={r.enabled} onChange={(_, d) => set({ enabled: d.checked === true })} label={t("Règle active")} />
        </fieldset>

        {error && (
          <MessageBar intent="warning">
            <MessageBarBody>{error}</MessageBarBody>
          </MessageBar>
        )}
        <div className="rule-editor-actions">
          <Button onClick={onCancel}>{t("Annuler")}</Button>
          <Button appearance="primary" onClick={save}>
            {t("Enregistrer")}
          </Button>
        </div>
      </div>
    </div>
  );
}
