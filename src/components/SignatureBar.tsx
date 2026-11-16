import { Button, Spinner } from "@fluentui/react-components";
import { ShieldCheckmark20Regular, ShieldError20Regular, ShieldQuestion20Regular } from "@fluentui/react-icons";
import { useState } from "react";
import { dayDate, longDate } from "../format";
import { t } from "../i18n";
import { fingerprintText, signerLabel, tone, verdictText, whyText, type SignatureInfo } from "../signature";

/**
 * Barre de signature S/MIME (lot D1) : résultat de la vérification faite au
 * téléchargement, détails du certificat et « Vérifier de nouveau ».
 */
export function SignatureBar({ signature, onCheck }: { signature: SignatureInfo; onCheck: () => Promise<void> }) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const kind = tone(signature);
  const why = signature.verdict === "verified" ? "" : whyText(signature.why);
  const Icon = kind === "good" ? ShieldCheckmark20Regular : kind === "danger" ? ShieldError20Regular : ShieldQuestion20Regular;
  return (
    <div className={`signature-bar signature-${kind}`} role="status">
      <div className="signature-line">
        <Icon />
        <span className="signature-text">
          {verdictText(signature)}
          {why && <> {why}</>}
        </span>
        <Button size="small" appearance="subtle" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
          {open ? t("Masquer les détails") : t("Détails")}
        </Button>
        <Button
          size="small"
          appearance="subtle"
          disabled={busy}
          icon={busy ? <Spinner size="extra-tiny" /> : undefined}
          onClick={async () => {
            setBusy(true);
            try {
              await onCheck();
            } finally {
              setBusy(false);
            }
          }}
        >
          {t("Vérifier de nouveau")}
        </Button>
      </div>
      {open && (
        <dl className="signature-details">
          <dt>{t("Signataire")}</dt>
          <dd>{signerLabel(signature)}</dd>
          {signature.addresses.length > 1 && (
            <>
              <dt>{t("Adresses du certificat")}</dt>
              <dd>{signature.addresses.join(", ")}</dd>
            </>
          )}
          {signature.issuer && (
            <>
              <dt>{t("Délivré par")}</dt>
              <dd>{signature.issuer}</dd>
            </>
          )}
          {signature.notAfter > 0 && (
            <>
              <dt>{t("Validité")}</dt>
              <dd>{t("du {from} au {to}", { from: dayDate(signature.notBefore), to: dayDate(signature.notAfter) })}</dd>
            </>
          )}
          {signature.signedAt !== null && (
            <>
              <dt>{t("Signé le")}</dt>
              <dd>{t("{date} (date annoncée par l'expéditeur)", { date: longDate(signature.signedAt) })}</dd>
            </>
          )}
          {signature.fingerprint && (
            <>
              <dt>{t("Empreinte SHA-256")}</dt>
              <dd className="signature-fingerprint">{fingerprintText(signature.fingerprint)}</dd>
            </>
          )}
          <dt>{t("Révocation")}</dt>
          <dd>
            {signature.why === "revoked"
              ? t("Certificat révoqué par son autorité.")
              : signature.revocationChecked
                ? t("Vérifiée : le certificat n'est pas dans la liste des certificats révoqués de son autorité.")
                : t("Non vérifiée : la liste de son autorité n'est pas encore téléchargée, ou elle n'en publie pas.")}
          </dd>
        </dl>
      )}
    </div>
  );
}
