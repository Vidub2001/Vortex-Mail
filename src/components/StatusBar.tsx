import { Spinner } from "@fluentui/react-components";
import { ArrowSync16Regular, CheckmarkCircle16Regular, CloudCheckmark16Regular, CloudOff16Regular } from "@fluentui/react-icons";
import { useEffect, useState } from "react";
import { formatTime } from "../general";
import { t } from "../i18n";

/**
 * Synchronisation plus longue que cela : signalée comme une attente. En deçà,
 * elle se fait sans indicateur, la liste en cache étant déjà affichée (lot E2).
 */
const SLOW_SYNC_MS = 8_000;

interface Props {
  offline: boolean;
  syncing: boolean;
  offlinePending: number | null;
  /** Messages concernés par des actions faites hors connexion, pas encore sur le serveur. */
  pendingActions: number;
  accountCount: number;
}

/** Barre d'état : connexion, synchronisation, lecture hors connexion. */
export function StatusBar(p: Props) {
  // Heure de la dernière synchronisation terminée, et synchronisation qui traîne.
  const [lastSync, setLastSync] = useState<number | null>(null);
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    if (!p.syncing) {
      setSlow(false);
      return;
    }
    const timer = window.setTimeout(() => setSlow(true), SLOW_SYNC_MS);
    return () => {
      window.clearTimeout(timer);
      setLastSync(Date.now());
    };
  }, [p.syncing]);
  return (
    <footer className="statusbar">
      <span className={p.offline ? "status-dot off" : "status-dot on"} />
      <span>{p.offline ? t("Hors ligne — messages enregistrés") : t("Connecté")}</span>
      {!p.offline && p.syncing && slow ? (
        <span className="status-item">
          <Spinner size="extra-tiny" /> {t("Synchronisation…")}
        </span>
      ) : (
        !p.offline &&
        lastSync !== null && (
          <span className="status-item" title={t("Les messages sont relevés en arrière-plan")}>
            <CheckmarkCircle16Regular /> {t("À jour à {time}", { time: formatTime(lastSync) })}
          </span>
        )
      )}
      {!p.offline && p.offlinePending !== null && p.accountCount > 0 && (
        <span className="status-item" title={t("Contenu des messages disponible sans connexion (hors corbeille et indésirables)")}>
          {p.offlinePending > 0 ? (
            <>
              <Spinner size="extra-tiny" /> {t("Hors connexion : {n} à télécharger", { n: p.offlinePending })}
            </>
          ) : (
            <>
              <CloudCheckmark16Regular /> {t("Disponible hors connexion")}
            </>
          )}
        </span>
      )}
      {p.offline && <CloudOff16Regular />}
      {p.pendingActions > 0 && (
        <span className="status-item pending" title={t("Lu, indicateurs, déplacements faits hors connexion : envoyés au serveur au retour du réseau")}>
          <ArrowSync16Regular />{" "}
          {p.pendingActions > 1
            ? t("{n} actions en attente", { n: p.pendingActions })
            : t("{n} action en attente", { n: p.pendingActions })}
        </span>
      )}
      <span className="status-spacer" />
      <span className="status-app">
        {p.accountCount > 1
          ? t("Vortex Mail · {n} comptes", { n: p.accountCount })
          : t("Vortex Mail · {n} compte", { n: p.accountCount })}
      </span>
    </footer>
  );
}
