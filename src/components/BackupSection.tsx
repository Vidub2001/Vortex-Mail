// Paramètres › Sauvegarde : exporter toute la configuration dans un fichier
// chiffré, et la reprendre sur une autre installation de Vortex Mail.
import { formatDate, formatTime } from "../general";
import { Button, Field, Input, MessageBar, MessageBarBody, MessageBarTitle, Spinner } from "@fluentui/react-components";
import { ArrowDownload20Regular, ArrowUpload20Regular } from "@fluentui/react-icons";
import { useEffect, useRef, useState } from "react";
import { api, type BackupInfo } from "../api";
import { t } from "../i18n";

/**
 * Clés propres à cette installation, jamais sauvegardées ni reprises :
 * copies de secours des brouillons, fenêtres de rédaction ouvertes,
 * messages déjà passés par les règles et messages qu'elles ont déplacés
 * (identifiants de ce cache local), rappels déjà montrés. Reprendre la liste
 * « déjà examinés » d'une vieille sauvegarde ferait passer de nouveau les
 * règles sur tous les messages arrivés depuis (relecture complète n° 41).
 */
export function localOnly(key: string): boolean {
  return (
    key === "vortex.drafts.local" ||
    (key.startsWith("vortex.compose.") && key !== "vortex.compose.format") ||
    key.startsWith("vortex.rules.done") ||
    key === "vortex.rules.moved" ||
    key.startsWith("vortex.reminders.")
  );
}

/** Réglages de l'interface gardés dans la fenêtre (thème, signatures, agenda…). */
function collectSettings(): string {
  const out: Record<string, string> = {};
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (!key || !/^vortex\./.test(key) || localOnly(key)) continue;
      const value = localStorage.getItem(key);
      if (value !== null) out[key] = value;
    }
  } catch {
    /* stockage indisponible : les réglages par défaut seront repris */
  }
  return JSON.stringify(out);
}

function restoreSettings(json: string) {
  try {
    const settings = JSON.parse(json) as Record<string, string>;
    // Remplacement, comme l'annonce l'écran : les réglages actuels absents de
    // la sauvegarde (règles, signatures) sont d'abord retirés (n° 41).
    const current: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key && key.startsWith("vortex.") && !localOnly(key)) current.push(key);
    }
    for (const key of current) localStorage.removeItem(key);
    // Seulement les réglages de l'application (le backend filtre aussi).
    for (const [key, value] of Object.entries(settings)) {
      if (key.startsWith("vortex.") && !localOnly(key) && typeof value === "string") localStorage.setItem(key, value);
    }
  } catch {
    /* sans réglages : ceux par défaut */
  }
}

/** Titre d'une sauvegarde : date et heure de création. */
const backupTitle = (ts: number) => t("Sauvegarde du {date} à {time}", { date: formatDate(ts), time: formatTime(ts) });

export function BackupSection({ accountCount }: { accountCount: number }) {
  // ------------------------------------------------------------ export
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [exporting, setExporting] = useState(false);
  const [exported, setExported] = useState<{ path: string; missing: string[] } | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);

  const tooShort = password.length > 0 && password.length < 8;
  const mismatch = confirm.length > 0 && confirm !== password;
  const canExport = password.length >= 8 && confirm === password && !exporting;

  const doExport = async () => {
    setExporting(true);
    setExported(null);
    setExportError(null);
    try {
      setExported(await api.exportBackup(password, collectSettings()));
      setPassword("");
      setConfirm("");
    } catch (e) {
      setExportError(String(e));
    } finally {
      setExporting(false);
    }
  };

  // ------------------------------------------------------------ import
  const fileInput = useRef<HTMLInputElement>(null);
  const [file, setFile] = useState<{ name: string; content: string } | null>(null);
  const [importPassword, setImportPassword] = useState("");
  const [busy, setBusy] = useState<"check" | "import" | null>(null);
  const [info, setInfo] = useState<BackupInfo | null>(null);
  const [importError, setImportError] = useState<string | null>(null);

  const pickFile = async (f: File | undefined) => {
    setInfo(null);
    setImportError(null);
    if (!f) return;
    if (f.size > 50 * 1024 * 1024) {
      setImportError(t("Fichier trop volumineux pour une sauvegarde de Vortex Mail."));
      return;
    }
    setFile({ name: f.name, content: await f.text() });
  };

  const check = async () => {
    if (!file) return;
    setBusy("check");
    setImportError(null);
    try {
      setInfo(await api.inspectBackup(file.content, importPassword));
    } catch (e) {
      setInfo(null);
      setImportError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const doImport = async () => {
    if (!file) return;
    setBusy("import");
    setImportError(null);
    try {
      restoreSettings(await api.importBackup(file.content, importPassword));
      await api.restartApp();
    } catch (e) {
      setImportError(String(e));
      setBusy(null);
    }
  };

  return (
    <div className="settings-pane">
      <h3 className="settings-pane-title">{t("Sauvegarde")}</h3>
      <div className="about-scroll">
        <fieldset className="account-fieldset">
          <legend>{t("Exporter la configuration")}</legend>
          <p className="backup-text">
            {t(
              "Un seul fichier, chiffré par le mot de passe choisi ici : comptes et mots de passe, connexions Outlook.com et Google, signatures et réglages, agenda et contacts « Sur cet ordinateur ». Le courrier n'y est pas : il se resynchronise depuis les serveurs.",
            )}
          </p>
          <div className="form-row">
            <Field
              label={t("Mot de passe de la sauvegarde")}
              validationState={tooShort ? "error" : "none"}
              validationMessage={tooShort ? t("8 caractères au moins") : undefined}
            >
              <Input type="password" autoComplete="new-password" value={password} onChange={(_, d) => setPassword(d.value)} />
            </Field>
            <Field
              label={t("Confirmer le mot de passe")}
              validationState={mismatch ? "error" : "none"}
              validationMessage={mismatch ? t("Les deux mots de passe diffèrent") : undefined}
            >
              <Input type="password" autoComplete="new-password" value={confirm} onChange={(_, d) => setConfirm(d.value)} />
            </Field>
          </div>
          <p className="backup-hint muted">
            {t("Retenez-le bien : sans lui, la sauvegarde ne peut pas être ouverte. Il protège tous vos mots de passe.")}
          </p>
          <div className="backup-actions">
            <Button
              appearance="primary"
              icon={exporting ? <Spinner size="tiny" /> : <ArrowDownload20Regular />}
              disabled={!canExport}
              onClick={doExport}
            >
              {exporting ? t("Chiffrement…") : t("Exporter")}
            </Button>
          </div>
          {exported && (
            <MessageBar intent="success" layout="multiline">
              <MessageBarBody>
                <MessageBarTitle>{t("Sauvegarde enregistrée")}</MessageBarTitle>
                <code className="provider-url">{exported.path}</code>
              </MessageBarBody>
            </MessageBar>
          )}
          {exported && exported.missing.length > 0 && (
            // Mot de passe inconnu au moment de l'export (trousseau verrouillé…) :
            // la sauvegarde ne le contient pas (relecture C4).
            <MessageBar intent="warning" layout="multiline">
              <MessageBarBody>
                <MessageBarTitle>{t("Mot de passe absent de la sauvegarde")}</MessageBarTitle>
                {t("Comptes concernés : {names}. Leur mot de passe sera à ressaisir après l'import.", { names: exported.missing.join(", ") })}
              </MessageBarBody>
            </MessageBar>
          )}
          {exportError && (
            <MessageBar intent="error" layout="multiline">
              <MessageBarBody>
                <MessageBarTitle>{t("Échec")}</MessageBarTitle>
                {exportError}
              </MessageBarBody>
            </MessageBar>
          )}
        </fieldset>

        <fieldset className="account-fieldset">
          <legend>{t("Importer une sauvegarde")}</legend>
          <p className="backup-text">
            {t("Remplace la configuration de cette installation par celle de la sauvegarde, puis redémarre Vortex Mail.")}
          </p>
          <input
            ref={fileInput}
            type="file"
            accept=".vxbackup"
            hidden
            onChange={(e) => {
              pickFile(e.target.files?.[0]);
              e.target.value = "";
            }}
          />
          <div className="backup-actions">
            <Button icon={<ArrowUpload20Regular />} disabled={busy !== null} onClick={() => fileInput.current?.click()}>
              {t("Choisir un fichier…")}
            </Button>
            {file && <span className="backup-file">{file.name}</span>}
          </div>
          {file && (
            <div className="form-row">
              <Field label={t("Mot de passe de la sauvegarde")}>
                <Input
                  type="password"
                  autoComplete="off"
                  value={importPassword}
                  onChange={(_, d) => {
                    setImportPassword(d.value);
                    setInfo(null);
                  }}
                  onKeyDown={(e) => e.key === "Enter" && importPassword && check()}
                />
              </Field>
              <div className="backup-check">
                <Button
                  icon={busy === "check" ? <Spinner size="tiny" /> : undefined}
                  disabled={!importPassword || busy !== null}
                  onClick={check}
                >
                  {busy === "check" ? t("Vérification…") : t("Ouvrir")}
                </Button>
              </div>
            </div>
          )}
          {info && (
            <MessageBar intent="warning" layout="multiline">
              <MessageBarBody>
                <MessageBarTitle>{backupTitle(info.created * 1000)}</MessageBarTitle>
                <ul className="backup-accounts">
                  {info.accounts.map((a) => (
                    <li key={a.email + a.name}>
                      {a.name} — {a.email}
                    </li>
                  ))}
                </ul>
                {info.localItems > 0 && (
                  <div>
                    {info.localItems > 1
                      ? t("{n} contacts et rendez-vous « Sur cet ordinateur ».", { n: info.localItems })
                      : t("1 contact et rendez-vous « Sur cet ordinateur ».")}
                  </div>
                )}
                <div className="backup-warning">
                  {accountCount > 0
                    ? accountCount > 1
                      ? t("Les {n} comptes actuels et les réglages de cette installation seront remplacés.", { n: accountCount })
                      : t("Le compte actuel et les réglages de cette installation seront remplacés.")
                    : t("Les réglages de cette installation seront remplacés.")}
                </div>
              </MessageBarBody>
            </MessageBar>
          )}
          {info && (
            <div className="backup-actions">
              <Button
                appearance="primary"
                className="danger-button"
                icon={busy === "import" ? <Spinner size="tiny" /> : undefined}
                disabled={busy !== null}
                onClick={doImport}
              >
                {busy === "import" ? t("Import en cours…") : t("Importer et redémarrer")}
              </Button>
            </div>
          )}
          {importError && (
            <MessageBar intent="error" layout="multiline">
              <MessageBarBody>
                <MessageBarTitle>{t("Import impossible")}</MessageBarTitle>
                {importError}
              </MessageBarBody>
            </MessageBar>
          )}
        </fieldset>
        <PortablePassword />
      </div>
    </div>
  );
}

/** Mode portable : changement du mot de passe maître (affiché seulement en mode portable). */
function PortablePassword() {
  const [portable, setPortable] = useState(false);
  const [old, setOld] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    api
      .portableStatus()
      .then((st) => setPortable(st.portable))
      .catch(() => {});
  }, []);
  if (!portable) return null;

  const change = async () => {
    if (next.length < 8) return setMessage({ ok: false, text: t("Mot de passe trop court : 8 caractères au moins.") });
    if (next !== confirm) return setMessage({ ok: false, text: t("Les deux mots de passe ne sont pas identiques.") });
    setBusy(true);
    setMessage(null);
    try {
      await api.portableChangePassword(old, next);
      setOld("");
      setNext("");
      setConfirm("");
      setMessage({ ok: true, text: t("Mot de passe maître changé.") });
    } catch (e) {
      setMessage({ ok: false, text: String(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <fieldset className="account-fieldset">
      <legend>{t("Version portable : mot de passe maître")}</legend>
      <div className="form-row">
        <Field label={t("Mot de passe actuel")}>
          <Input type="password" value={old} onChange={(_, d) => setOld(d.value)} />
        </Field>
      </div>
      <div className="form-row">
        <Field label={t("Nouveau mot de passe")}>
          <Input type="password" value={next} onChange={(_, d) => setNext(d.value)} />
        </Field>
        <Field label={t("Confirmation")}>
          <Input type="password" value={confirm} onChange={(_, d) => setConfirm(d.value)} />
        </Field>
      </div>
      {message && (
        <MessageBar intent={message.ok ? "success" : "error"}>
          <MessageBarBody>{message.text}</MessageBarBody>
        </MessageBar>
      )}
      <div className="mail-signature-actions">
        <Button appearance="primary" disabled={busy || !old || !next} onClick={change} icon={busy ? <Spinner size="tiny" /> : undefined}>
          {t("Changer le mot de passe maître")}
        </Button>
      </div>
    </fieldset>
  );
}

