import { Button, Checkbox, Field, Input, MessageBar, MessageBarBody, Spinner } from "@fluentui/react-components";
import { ArrowUpload20Regular, ShieldCheckmark20Regular } from "@fluentui/react-icons";
import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { dayDate } from "../format";
import { t } from "../i18n";
import { fingerprintText } from "../signature";
import type { SmimeIdentity } from "../types";
import { ConfirmDialog } from "./Dialogs";

/** Contenu d'un fichier en base64 (sans `data:`), par tranches pour les gros fichiers. */
async function fileBase64(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

/**
 * Certificat de signature S/MIME d'un compte (lot D2) : import d'un fichier
 * PKCS #12 (`.p12`, `.pfx`) avec son mot de passe, détails, « Signer tous les
 * messages », retrait. La clé privée reste dans le trousseau du système.
 */
export function SmimeSection({ accountId, email }: { accountId: string; email: string }) {
  const [identity, setIdentity] = useState<SmimeIdentity | null | undefined>(undefined);
  const [file, setFile] = useState<File | null>(null);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const picker = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let alive = true;
    api
      .smimeIdentity(accountId)
      .then((id) => alive && setIdentity(id))
      .catch(() => alive && setIdentity(null));
    return () => {
      alive = false;
    };
  }, [accountId]);

  const doImport = async () => {
    if (!file) return;
    setBusy(true);
    setError(null);
    try {
      setIdentity(await api.smimeImport(accountId, await fileBase64(file), password));
      setFile(null);
      setPassword("");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const setDefault = async (on: boolean) => {
    if (!identity) return;
    setError(null);
    try {
      await api.smimeSignByDefault(accountId, on);
      setIdentity({ ...identity, signByDefault: on });
    } catch (e) {
      setError(String(e));
    }
  };

  if (identity === undefined) return <Spinner size="tiny" />;
  const now = Date.now() / 1000;
  const expired = !!identity && (identity.notAfter < now || identity.notBefore > now);

  return (
    <>
      {identity ? (
        <div className="smime-identity">
          <div className="smime-identity-title">
            <ShieldCheckmark20Regular />
            <strong>{identity.name || identity.addresses[0]}</strong>
            <span className="muted">{identity.addresses.join(", ")}</span>
          </div>
          <dl className="signature-details">
            <dt>{t("Délivré par")}</dt>
            <dd>{identity.issuer}</dd>
            <dt>{t("Validité")}</dt>
            <dd className={expired ? "smime-expired" : undefined}>
              {t("du {from} au {to}", { from: dayDate(identity.notBefore), to: dayDate(identity.notAfter) })}
              {expired && ` — ${t("expiré")}`}
            </dd>
            <dt>{t("Empreinte SHA-256")}</dt>
            <dd className="signature-fingerprint">{fingerprintText(identity.fingerprint)}</dd>
          </dl>
          <Checkbox
            checked={identity.signByDefault}
            label={t("Signer tous les messages de ce compte")}
            onChange={(_, d) => void setDefault(d.checked === true)}
          />
          <div>
            <Button size="small" appearance="subtle" onClick={() => setConfirmRemove(true)}>
              {t("Retirer le certificat")}
            </Button>
          </div>
        </div>
      ) : (
        <div className="google-help">
          {t(
            "Importez le certificat de signature de {email} et sa clé privée (fichier .p12 ou .pfx, délivré par une autorité de certification). La clé est gardée dans le trousseau du système.",
            { email },
          )}
        </div>
      )}

      <input
        ref={picker}
        type="file"
        accept=".p12,.pfx,application/x-pkcs12"
        hidden
        onChange={(e) => {
          setFile(e.target.files?.[0] ?? null);
          setError(null);
          e.target.value = "";
        }}
      />
      <div className="form-row smime-import">
        <Button icon={<ArrowUpload20Regular />} disabled={busy} onClick={() => picker.current?.click()}>
          {file ? file.name : identity ? t("Remplacer le certificat…") : t("Choisir un fichier…")}
        </Button>
        {file && (
          <>
            <Field label={t("Mot de passe du fichier")}>
              <Input
                type="password"
                value={password}
                autoComplete="off"
                onChange={(_, d) => setPassword(d.value)}
                onKeyDown={(e) => e.key === "Enter" && void doImport()}
              />
            </Field>
            <Button appearance="primary" disabled={busy} onClick={() => void doImport()}>
              {busy ? <Spinner size="tiny" /> : t("Importer")}
            </Button>
          </>
        )}
      </div>
      {error && (
        <MessageBar intent="error" layout="multiline">
          <MessageBarBody>{error}</MessageBarBody>
        </MessageBar>
      )}
      {confirmRemove && (
        <ConfirmDialog
          title={t("Retirer le certificat ?")}
          message={t("La clé de signature de {email} sera effacée du trousseau. Gardez votre fichier .p12 pour l'importer de nouveau.", { email })}
          confirm={t("Retirer")}
          onConfirm={async () => {
            await api.smimeRemove(accountId);
            setIdentity(null);
          }}
          onClose={() => setConfirmRemove(false)}
        />
      )}
    </>
  );
}
