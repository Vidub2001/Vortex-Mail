// Connexion Google ou Outlook.com expirée ou révoquée : bandeau « Reconnecter »
// (événement `auth-required` envoyé par l'application), au lieu d'une simple erreur.
import { Button, MessageBar, MessageBarActions, MessageBarBody, MessageBarTitle, Spinner } from "@fluentui/react-components";
import { Dismiss20Regular } from "@fluentui/react-icons";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { api, runningInTauri } from "../api";
import { t } from "../i18n";

type Provider = "google" | "o365";
interface Pending {
  provider: Provider;
  user: string;
}

interface Props {
  /** Après une reconnexion réussie : resynchroniser. */
  onReconnected: () => void;
  notify: (title: string, body?: string, intent?: "success" | "error" | "info") => void;
}

export function ReconnectBar({ onReconnected, notify }: Props) {
  const [pending, setPending] = useState<Pending[]>([]);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    if (!runningInTauri) return;
    const add = (p: Pending) =>
      setPending((list) =>
        list.some((x) => x.provider === p.provider && x.user.toLowerCase() === p.user.toLowerCase()) ? list : [...list, p],
      );
    const unlisten = listen<Pending>("auth-required", (e) => add(e.payload));
    // Connexions déjà signalées avant l'ouverture de la fenêtre (synchronisation du démarrage).
    unlisten.then(() => api.authPending().then((list) => list.forEach(add)).catch(() => {}), () => {});
    return () => {
      unlisten.then((f) => f(), () => {});
    };
  }, []);

  if (pending.length === 0) return null;
  const item = pending[0];
  const key = `${item.provider}|${item.user}`;
  const service = item.provider === "google" ? "Google" : "Outlook.com";
  const drop = () => setPending((list) => list.slice(1));

  const reconnect = async () => {
    setBusy(key);
    try {
      if (item.provider === "google") {
        // Compte Gmail réglé en OAuth2 : la connexion doit aussi autoriser le courrier.
        const configs = await api.listAccountConfigs().catch(() => []);
        const mail = configs.some(
          (c) => c.imap.auth === "oauth2" && c.imap.username.trim().toLowerCase() === item.user.toLowerCase(),
        );
        await api.googleSignIn(item.user, "", "", mail);
      } else {
        await api.oauthSignIn(item.user, "");
      }
      drop();
      notify(t("Connexion rétablie"), item.user);
      onReconnected();
    } catch (e) {
      notify(t("Reconnexion impossible"), String(e), "error");
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="reconnect-bar" role="alert">
      <MessageBar intent="warning" layout="multiline">
        <MessageBarBody>
          <MessageBarTitle>{t("Connexion {service} à refaire", { service })}</MessageBarTitle>
          {t("{user} : l'autorisation a expiré ou a été retirée. Reconnectez le compte pour reprendre la synchronisation.", {
            user: item.user,
          })}
        </MessageBarBody>
        <MessageBarActions
          containerAction={
            <Button appearance="transparent" aria-label={t("Plus tard")} title={t("Plus tard")} icon={<Dismiss20Regular />} onClick={drop} />
          }
        >
          <Button appearance="primary" disabled={busy !== null} icon={busy ? <Spinner size="tiny" /> : undefined} onClick={reconnect}>
            {busy ? t("En attente du navigateur…") : t("Reconnecter")}
          </Button>
        </MessageBarActions>
      </MessageBar>
    </div>
  );
}
