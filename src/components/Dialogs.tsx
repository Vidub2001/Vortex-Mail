import {
  Button,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Field,
  Input,
  Select,
  Spinner,
} from "@fluentui/react-components";
import { useState } from "react";
import { t } from "../i18n";
import type { Account } from "../types";

/** Demande d'un nom (nouveau dossier…). `onSubmit` rejette avec le message d'erreur. */
export function PromptDialog(p: {
  title: string;
  label: string;
  confirm: string;
  /** Valeur de départ (renommer). */
  initial?: string;
  onSubmit: (value: string) => Promise<void>;
  onClose: () => void;
}) {
  const [value, setValue] = useState(p.initial ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    if (!value.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      await p.onSubmit(value.trim());
      p.onClose();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <Dialog open onOpenChange={(_, d) => !d.open && !busy && p.onClose()}>
      <DialogSurface className="small-dialog">
        <DialogBody>
          <DialogTitle>{p.title}</DialogTitle>
          <DialogContent>
            <Field label={p.label} validationMessage={error ?? undefined} validationState={error ? "error" : "none"}>
              <Input
                autoFocus
                value={value}
                disabled={busy}
                onChange={(_, d) => setValue(d.value)}
                onKeyDown={(e) => e.key === "Enter" && submit()}
              />
            </Field>
          </DialogContent>
          <DialogActions>
            <Button appearance="primary" disabled={!value.trim() || busy} onClick={submit}
              icon={busy ? <Spinner size="tiny" /> : undefined}>
              {p.confirm}
            </Button>
            <Button disabled={busy} onClick={p.onClose}>
              {t("Annuler")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}

/** Confirmation d'une action destructrice. */
export function ConfirmDialog(p: {
  title: string;
  message: string;
  confirm: string;
  onConfirm: () => Promise<void>;
  onClose: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      await p.onConfirm();
      p.onClose();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <Dialog open onOpenChange={(_, d) => !d.open && !busy && p.onClose()}>
      <DialogSurface className="small-dialog">
        <DialogBody>
          <DialogTitle>{p.title}</DialogTitle>
          <DialogContent>
            <p className="dialog-text">{p.message}</p>
            {error && <p className="dialog-error">{error}</p>}
          </DialogContent>
          <DialogActions>
            <Button appearance="primary" className="danger-button" disabled={busy} onClick={run}
              icon={busy ? <Spinner size="tiny" /> : undefined}>
              {p.confirm}
            </Button>
            <Button disabled={busy} onClick={p.onClose}>
              {t("Annuler")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}

/** Choix d'une destination dans une liste (déplacer un dossier). */
export function ChoiceDialog(p: {
  title: string;
  label: string;
  confirm: string;
  choices: { value: string; label: string }[];
  onSubmit: (value: string) => Promise<void>;
  onClose: () => void;
}) {
  const [value, setValue] = useState(p.choices[0]?.value ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await p.onSubmit(value);
      p.onClose();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <Dialog open onOpenChange={(_, d) => !d.open && !busy && p.onClose()}>
      <DialogSurface className="small-dialog">
        <DialogBody>
          <DialogTitle>{p.title}</DialogTitle>
          <DialogContent>
            <Field label={p.label} validationMessage={error ?? undefined} validationState={error ? "error" : "none"}>
              <Select value={value} disabled={busy} onChange={(_, d) => setValue(d.value)}>
                {p.choices.map((c) => (
                  <option key={c.value} value={c.value}>
                    {c.label}
                  </option>
                ))}
              </Select>
            </Field>
          </DialogContent>
          <DialogActions>
            <Button appearance="primary" disabled={busy} onClick={submit}
              icon={busy ? <Spinner size="tiny" /> : undefined}>
              {p.confirm}
            </Button>
            <Button disabled={busy} onClick={p.onClose}>
              {t("Annuler")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}

/**
 * Nouveau dossier choisi depuis « Déplacer » du clic droit (lot Q) : son nom
 * et le compte où le créer (premier niveau), celui du message par défaut.
 */
export function NewFolderDialog(p: {
  accounts: Account[];
  initialAccount: string;
  initialName: string;
  onSubmit: (accountId: string, name: string) => Promise<void>;
  onClose: () => void;
}) {
  const [name, setName] = useState(p.initialName);
  const [accountId, setAccountId] = useState(p.accounts.some((a) => a.id === p.initialAccount) ? p.initialAccount : (p.accounts[0]?.id ?? ""));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    if (!name.trim() || !accountId || busy) return;
    setBusy(true);
    setError(null);
    try {
      await p.onSubmit(accountId, name.trim());
      p.onClose();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <Dialog open onOpenChange={(_, d) => !d.open && !busy && p.onClose()}>
      <DialogSurface className="small-dialog">
        <DialogBody>
          <DialogTitle>{t("Nouveau dossier")}</DialogTitle>
          <DialogContent className="new-folder">
            <Field label={t("Nom du dossier")} validationMessage={error ?? undefined} validationState={error ? "error" : "none"}>
              <Input
                autoFocus
                value={name}
                disabled={busy}
                onChange={(_, d) => setName(d.value)}
                onKeyDown={(e) => e.key === "Enter" && submit()}
              />
            </Field>
            {p.accounts.length > 1 && (
              <Field label={t("Dans le compte")}>
                <Select value={accountId} disabled={busy} onChange={(_, d) => setAccountId(d.value)}>
                  {p.accounts.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.name}
                    </option>
                  ))}
                </Select>
              </Field>
            )}
          </DialogContent>
          <DialogActions>
            <Button appearance="primary" disabled={!name.trim() || busy} onClick={submit} icon={busy ? <Spinner size="tiny" /> : undefined}>
              {t("Créer et déplacer")}
            </Button>
            <Button disabled={busy} onClick={p.onClose}>
              {t("Annuler")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
