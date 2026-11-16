import { Button, Spinner } from "@fluentui/react-components";
import { Delete16Regular, ShieldCheckmark16Regular, ShieldQuestion16Regular } from "@fluentui/react-icons";
import { useEffect, useState } from "react";
import { api } from "../api";
import { dayDate } from "../format";
import { t } from "../i18n";
import { fingerprintText } from "../signature";
import type { CorrespondentInfo } from "../types";

/**
 * Certificats des correspondants (lot D3b) : gardés à chaque message signé
 * reçu, ils servent à leur écrire chiffré. Ceux qui ne viennent pas d'une
 * autorité de confiance sont signalés ; chacun peut être oublié.
 */
export function CorrespondentsSection() {
  const [list, setList] = useState<CorrespondentInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    api
      .smimeCorrespondents()
      .then((l) => alive && setList(l))
      .catch((e) => alive && setError(String(e)));
    return () => {
      alive = false;
    };
  }, []);

  const forget = async (address: string) => {
    try {
      await api.smimeForgetCorrespondent(address);
      setList((l) => (l ? l.filter((c) => c.address !== address) : l));
    } catch (e) {
      setError(String(e));
    }
  };

  if (error) return <p className="muted">{error}</p>;
  if (!list) return <Spinner size="tiny" />;
  if (list.length === 0) {
    return <p className="muted">{t("Aucun pour l'instant : ils sont gardés à chaque message signé reçu.")}</p>;
  }
  const now = Date.now() / 1000;
  return (
    <ul className="correspondents">
      {list.map((c) => (
        <li key={c.address}>
          {c.verified ? (
            <ShieldCheckmark16Regular className="correspondent-verified" aria-label={t("Autorité de confiance")} />
          ) : (
            <ShieldQuestion16Regular className="correspondent-unverified" aria-label={t("Autorité inconnue")} />
          )}
          <span className="correspondent-text">
            <strong>{c.address}</strong>
            <span className="muted">
              {" "}
              {c.name && c.name !== c.address ? `${c.name} — ` : ""}
              {c.notAfter < now
                ? t("expiré le {date}", { date: dayDate(c.notAfter) })
                : t("valide jusqu'au {date}", { date: dayDate(c.notAfter) })}
              {!c.verified && ` — ${t("autorité inconnue")}`}
            </span>
            {/* Empreinte : à comparer avec le correspondant pour un certificat d'autorité inconnue (lot D4). */}
            {!c.verified && <span className="signature-fingerprint correspondent-fingerprint">{fingerprintText(c.fingerprint)}</span>}
          </span>
          <Button
            size="small"
            appearance="subtle"
            icon={<Delete16Regular />}
            title={t("Oublier ce certificat")}
            aria-label={t("Oublier le certificat de {address}", { address: c.address })}
            onClick={() => void forget(c.address)}
          />
        </li>
      ))}
    </ul>
  );
}
