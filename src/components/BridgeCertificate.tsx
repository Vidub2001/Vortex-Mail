// Certificat de Proton Mail Bridge (lot I3). Bridge chiffre la connexion avec un
// certificat auto-signé, propre à chaque installation : son empreinte est lue sur
// cet ordinateur (commande `bridge_certificate`), montrée à l'utilisateur, puis
// épinglée dans le compte. Le backend refuse ensuite la connexion si elle change.
import { Button, MessageBar, MessageBarActions, MessageBarBody, Spinner } from "@fluentui/react-components";
import { useState } from "react";
import { api } from "../api";
import { t } from "../i18n";
import type { Security } from "../types";

/** Page de téléchargement de Proton Mail Bridge. */
export const BRIDGE_DOWNLOAD = "https://proton.me/mail/bridge";

/** Adresse de la boucle locale, seul hôte admis pour un certificat épinglé (comme `transport::is_loopback`). */
export const isLoopback = (host: string) => /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host.trim()) || host.trim() === "::1";

interface Props {
  /** Serveur IMAP de Bridge sur cet ordinateur. */
  port: number;
  security: Security;
  /** Empreinte déjà retenue, ou vide. */
  value: string;
  onRead: (fingerprint: string) => void;
}

export function BridgeCertificate({ port, security, value, onRead }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const read = async () => {
    setBusy(true);
    setError(null);
    try {
      onRead(await api.bridgeCertificate(port, security));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="bridge-cert">
      {value && (
        <dl className="wizard-servers">
          <dt>{t("Certificat de Bridge")}</dt>
          <dd className="fingerprint">{value}</dd>
        </dl>
      )}
      <div className="oauth-row">
        <Button size="small" onClick={read} disabled={busy}>
          {value ? t("Relire le certificat de Bridge") : t("Lire le certificat de Bridge")}
        </Button>
        {busy && <Spinner size="tiny" />}
      </div>
      {error && (
        <MessageBar intent="error" layout="multiline">
          <MessageBarBody>{error}</MessageBarBody>
          <MessageBarActions>
            <Button size="small" onClick={() => api.openUrl(BRIDGE_DOWNLOAD).catch(() => {})}>
              {t("Télécharger Proton Mail Bridge")}
            </Button>
          </MessageBarActions>
        </MessageBar>
      )}
    </div>
  );
}
