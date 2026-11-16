// Mode portable : mot de passe maître au démarrage (création au premier lancement).
import { Button, Field, FluentProvider, Input, MessageBar, MessageBarBody, Spinner } from "@fluentui/react-components";
import { LockClosed20Regular } from "@fluentui/react-icons";
import { useState } from "react";
import { api } from "../api";
import { t } from "../i18n";
import { darkTheme, lightTheme, useThemeMode } from "../theme";

interface Props {
  /** Premier lancement : le coffre n'existe pas encore, on choisit le mot de passe. */
  creating: boolean;
  onUnlocked: () => void;
}

export function UnlockScreen({ creating, onUnlocked }: Props) {
  const theme = useThemeMode();
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    if (creating && password.length < 8) return setError(t("Mot de passe trop court : 8 caractères au moins."));
    if (creating && password !== confirm) return setError(t("Les deux mots de passe ne sont pas identiques."));
    setBusy(true);
    setError(null);
    try {
      await api.portableUnlock(password);
      onUnlocked();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <FluentProvider theme={theme.dark ? darkTheme : lightTheme} className={theme.dark ? "provider dark" : "provider light"}>
      <div className="unlock-screen">
        <form
          className="unlock-card"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <LockClosed20Regular className="unlock-icon" />
          <h1>Vortex Mail</h1>
          <p className="muted">
            {creating
              ? t("Version portable : choisissez un mot de passe maître. Il protège les mots de passe et connexions de vos comptes, rangés avec Vortex Mail.")
              : t("Version portable : saisissez le mot de passe maître.")}
          </p>
          <Field label={t("Mot de passe maître")}>
            <Input type="password" value={password} autoFocus onChange={(_, d) => setPassword(d.value)} />
          </Field>
          {creating && (
            <Field label={t("Confirmation")}>
              <Input type="password" value={confirm} onChange={(_, d) => setConfirm(d.value)} />
            </Field>
          )}
          {creating && (
            <p className="muted unlock-warning">
              {t("Ce mot de passe ne peut pas être retrouvé : en cas d'oubli, il faudra ressaisir les comptes.")}
            </p>
          )}
          {error && (
            <MessageBar intent="error">
              <MessageBarBody>{error}</MessageBarBody>
            </MessageBar>
          )}
          <Button appearance="primary" type="submit" disabled={busy || !password} icon={busy ? <Spinner size="tiny" /> : undefined}>
            {creating ? t("Créer") : t("Déverrouiller")}
          </Button>
        </form>
      </div>
    </FluentProvider>
  );
}
