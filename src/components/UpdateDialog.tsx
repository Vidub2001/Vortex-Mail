import { Button, Dialog, DialogActions, DialogBody, DialogContent, DialogSurface, DialogTitle } from "@fluentui/react-components";
import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";
import { api } from "../api";
import { t } from "../i18n";
import type { NewVersion } from "../types";

/** Nouvelle version publiée (lot K1) : lien vers sa page, rien n'est installé ici. */
export function UpdateDialog(p: { found: NewVersion; onLater: () => void; onSkip: () => void }) {
  const [current, setCurrent] = useState("");
  useEffect(() => {
    getVersion().then(setCurrent, () => setCurrent(""));
  }, []);
  return (
    <Dialog open onOpenChange={(_, d) => !d.open && p.onLater()}>
      <DialogSurface className="small-dialog">
        <DialogBody>
          <DialogTitle>{t("Nouvelle version disponible")}</DialogTitle>
          <DialogContent>
            <p className="dialog-text">
              {current
                ? t("Vortex Mail {version} est disponible (vous avez la {current}).", { version: p.found.version, current })
                : t("Vortex Mail {version} est disponible.", { version: p.found.version })}
            </p>
            <p className="muted small">{t("Ces rappels peuvent être désactivés dans Paramètres › Général.")}</p>
          </DialogContent>
          <DialogActions>
            <Button appearance="subtle" onClick={p.onSkip}>
              {t("Ne plus signaler cette version")}
            </Button>
            <Button onClick={p.onLater}>{t("Plus tard")}</Button>
            <Button
              appearance="primary"
              onClick={() => {
                void api.openUrl(p.found.url);
                p.onLater();
              }}
            >
              {t("Voir la nouvelle version")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
