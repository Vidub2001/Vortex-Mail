// Assistant d'ajout de compte (lot I2) : le nom et l'adresse suffisent. Les
// réglages sont cherchés par le backend (commande `detect_account`, lot I1),
// montrés avec leur source, puis le compte est testé et ajouté. Le formulaire
// complet reste accessible (« Modifier à la main »), prérempli. Proton Mail
// passe par Bridge, dont le certificat est confirmé puis épinglé (lot I3).
import { Button, Checkbox, Field, Input, MessageBar, MessageBarBody, MessageBarTitle, Spinner } from "@fluentui/react-components";
import { ArrowLeft20Regular, CheckmarkCircle16Filled, DismissCircle16Filled } from "@fluentui/react-icons";
import { useState } from "react";
import { api } from "../api";
import { t } from "../i18n";
import { isPlausibleEmail } from "../text";
import { BridgeCertificate } from "./BridgeCertificate";
import type { AccountConfig, AuthMethod, Detected, DetectedServer, Security, ServerConfig, TestReport } from "../types";

type Notify = (title: string, body?: string, intent?: "success" | "error" | "info") => void;

interface Props {
  color: string;
  /** Formulaire complet, prérempli avec ce qui est connu. */
  onManual: (config: AccountConfig) => void;
  onCancel: () => void;
  onSaved: () => void;
  notify: Notify;
}

type Step = { kind: "address" } | { kind: "searching" } | { kind: "found"; found: Detected } | { kind: "failed"; message: string };

/** D'où viennent les réglages, en clair. */
export function sourceLabel(source: Detected["source"]): string {
  switch (source) {
    case "builtin":
      return t("fournisseur connu de Vortex Mail");
    case "ispdb":
      return t("base des fournisseurs de Thunderbird (copie intégrée)");
    case "mx":
      return t("hébergeur reconnu par le DNS du domaine");
    case "srv":
      return t("annoncés par le DNS du domaine");
    case "domain":
      return t("publiés par le domaine");
    case "guess":
      return t("noms habituels, vérifiés sans mot de passe");
  }
}

function securityLabel(s: Security): string {
  return s === "sslTls" ? "SSL/TLS" : s === "startTls" ? "STARTTLS" : s;
}

const serverLine = (s: DetectedServer) => `${s.host}:${s.port} (${securityLabel(s.security)})`;

/** Compte construit à partir des réglages trouvés ; `pin` : empreinte du certificat de Bridge confirmée, ou vide. */
export function configFrom(found: Detected, email: string, displayName: string, name: string, color: string, password: string, pin = ""): AccountConfig {
  // Gmail : mot de passe d'application par défaut ; la connexion avec Google passe
  // par le formulaire complet (application Google de l'utilisateur), voir `withGoogle`.
  const auth: AuthMethod = found.login === "outlook" ? "oauth2" : "normal";
  const server = (s: DetectedServer): ServerConfig => ({
    host: s.host,
    port: s.port,
    security: s.security,
    auth,
    username: found.username,
    password,
    pinnedCert: pin,
  });
  return {
    id: "",
    name,
    email,
    displayName,
    color,
    imap: server(found.imap),
    smtp: server(found.smtp),
    smtpSameCredentials: true,
    oauthClientId: "",
    davUrl: "",
    cardUrl: "",
    offlineBodies: true,
  };
}

/** Nom proposé pour le compte : celui du fournisseur, sinon le domaine. */
function defaultName(found: Detected | null, email: string): string {
  if (found?.provider) return found.provider;
  const domain = email.split("@")[1] ?? "";
  return domain.split(".")[0].replace(/^./, (c) => c.toUpperCase());
}

export function AccountWizard({ color, onManual, onCancel, onSaved, notify }: Props) {
  const [step, setStep] = useState<Step>({ kind: "address" });
  const [displayName, setDisplayName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<TestReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** Certificat de Bridge lu sur cet ordinateur, et confirmé par l'utilisateur. */
  const [cert, setCert] = useState("");
  const [trusted, setTrusted] = useState(false);

  const address = email.trim();
  const valid = isPlausibleEmail(address);
  const found = step.kind === "found" ? step.found : null;
  const pin = trusted ? cert : "";

  /** Formulaire complet : réglages trouvés, ou seulement l'adresse et le nom. */
  const manual = () => {
    if (found) {
      onManual(configFrom(found, address, displayName.trim(), name.trim() || defaultName(found, address), color, password, pin));
      return;
    }
    const unknown: Detected = {
      source: "guess",
      provider: "",
      imap: { host: "", port: 993, security: "sslTls" },
      smtp: { host: "", port: 465, security: "sslTls" },
      username: address,
      login: "password",
      outsideDomain: false,
      helpUrl: "",
    };
    onManual(configFrom(unknown, address, displayName.trim(), defaultName(null, address), color, ""));
  };

  /** Gmail avec agenda et contacts : formulaire complet en connexion Google (`OAuth2`). */
  const withGoogle = () => {
    if (!found) return;
    const config = configFrom(found, address, displayName.trim(), name.trim() || defaultName(found, address), color, "");
    onManual({ ...config, imap: { ...config.imap, auth: "oauth2" }, smtp: { ...config.smtp, auth: "oauth2" } });
  };

  const search = async () => {
    setStep({ kind: "searching" });
    setReport(null);
    setError(null);
    setCert("");
    setTrusted(false);
    try {
      const detected = await api.detectAccount(address);
      setName(defaultName(detected, address));
      setStep({ kind: "found", found: detected });
    } catch (e) {
      setStep({ kind: "failed", message: String(e) });
    }
  };

  const connect = async () => {
    if (!found) return;
    setBusy(true);
    setReport(null);
    setError(null);
    const config = configFrom(found, address, displayName.trim(), name.trim() || defaultName(found, address), color, password, pin);
    try {
      const tested = await api.testAccount(config);
      if (!tested.imap.ok || !tested.smtp.ok) {
        setReport(tested);
        return;
      }
      const account = await api.addAccount(config);
      notify(t("Compte ajouté"), `${account.name} (${account.email})`);
      onSaved();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const check = (label: string, r: { ok: boolean; message: string }) => (
    <div className={r.ok ? "check-line ok" : "check-line ko"}>
      {r.ok ? <CheckmarkCircle16Filled /> : <DismissCircle16Filled />}
      <strong>{label}</strong> {r.message}
    </div>
  );

  const browserLogin = found?.login === "outlook";
  const google = found?.login === "google";
  const appPassword = google || found?.login === "appPassword";
  const bridge = found?.login === "protonBridge";

  return (
    <div className="settings-pane">
      <div className="settings-breadcrumb">
        <Button appearance="subtle" icon={<ArrowLeft20Regular />} aria-label={t("Retour")} title={t("Retour")} onClick={onCancel} />
        <h3 className="settings-pane-title">{t("Ajouter un compte")}</h3>
      </div>

      <div className="settings-form wizard">
        <fieldset className="account-fieldset">
          <legend>{t("Votre adresse")}</legend>
          <div className="form-row">
            <Field label={t("Votre nom")}>
              <Input value={displayName} title={t("Vu par les destinataires de vos messages")} disabled={step.kind === "searching"} onChange={(_, d) => setDisplayName(d.value)} />
            </Field>
            <Field label={t("Adresse e-mail")} required>
              <Input
                type="email"
                value={email}
                placeholder={t("prenom.nom@exemple.fr")}
                disabled={step.kind === "searching"}
                onChange={(_, d) => {
                  setEmail(d.value);
                  if (step.kind !== "address") setStep({ kind: "address" });
                }}
                onKeyDown={(e) => e.key === "Enter" && valid && search()}
              />
            </Field>
          </div>
        </fieldset>

        {step.kind === "searching" && (
          <div className="wizard-search" aria-live="polite">
            <Spinner size="small" label={t("Recherche des réglages de {domain}…", { domain: address.split("@")[1] ?? "" })} labelPosition="after" />
            <p className="muted small">
              {t(
                "Fournisseurs connus, base de Thunderbird intégrée, DNS du domaine, fichier publié par le domaine, puis noms habituels. Votre mot de passe n'est envoyé nulle part pendant la recherche.",
              )}
            </p>
          </div>
        )}

        {step.kind === "failed" && (
          <MessageBar intent="warning" layout="multiline">
            <MessageBarBody>
              <MessageBarTitle>{t("Réglages introuvables")}</MessageBarTitle>
              {step.message}
            </MessageBarBody>
          </MessageBar>
        )}

        {found && (
          <fieldset className="account-fieldset wizard-card">
            <legend>{found.provider || t("Réglages trouvés")}</legend>
            <p className="muted small">{t("Source : {source}", { source: sourceLabel(found.source) })}</p>
            <dl className="wizard-servers">
              <dt>{t("Réception (IMAP)")}</dt>
              <dd>{serverLine(found.imap)}</dd>
              <dt>{t("Envoi (SMTP)")}</dt>
              <dd>{serverLine(found.smtp)}</dd>
              <dt>{t("Identifiant")}</dt>
              <dd>{found.username}</dd>
            </dl>

            {found.outsideDomain && (
              <MessageBar intent="warning" layout="multiline">
                <MessageBarBody>
                  <MessageBarTitle>{t("Serveurs hors de votre domaine")}</MessageBarTitle>
                  {t(
                    "Le DNS du domaine désigne des serveurs d'un autre domaine, souvent ceux d'un hébergeur. Vérifiez qu'ils sont bien ceux de votre fournisseur avant d'y envoyer votre mot de passe.",
                  )}
                </MessageBarBody>
              </MessageBar>
            )}

            {bridge && (
              <MessageBar intent="info" layout="multiline">
                <MessageBarBody>
                  <MessageBarTitle>{t("Proton Mail passe par Proton Mail Bridge")}</MessageBarTitle>
                  {t(
                    "Bridge (inclus dans les abonnements payants de Proton) relaie votre courrier sur cet ordinateur. Lancez-le et connectez-y votre compte Proton, puis lisez son certificat.",
                  )}
                </MessageBarBody>
              </MessageBar>
            )}

            {bridge && (
              <BridgeCertificate
                port={found.imap.port}
                security={found.imap.security}
                value={cert}
                onRead={(fingerprint) => {
                  setCert(fingerprint);
                  setTrusted(false);
                  setReport(null);
                }}
              />
            )}

            {bridge && cert && (
              <Checkbox
                checked={trusted}
                onChange={(_, d) => setTrusted(d.checked === true)}
                label={t("Ce certificat est celui de Bridge sur cet ordinateur : Vortex Mail le retient et refusera la connexion s'il change.")}
              />
            )}

            {browserLogin && (
              <MessageBar intent="info" layout="multiline">
                <MessageBarBody>
                  {t("Connexion à Outlook.com dans votre navigateur : aucun mot de passe n'est saisi ici. Continuez pour la terminer.")}
                </MessageBarBody>
              </MessageBar>
            )}

            {google && (
              <MessageBar intent="info" layout="multiline">
                <MessageBarBody>
                  {t(
                    "Courrier seul : collez un mot de passe d'application Google ci-dessous. Courrier, agenda et contacts : « Se connecter avec Google » ouvre le formulaire complet, pour votre propre application Google.",
                  )}
                </MessageBarBody>
              </MessageBar>
            )}

            {!browserLogin && (!bridge || trusted) && (
              <div className="form-row wizard-row">
                <Field
                  label={bridge ? t("Mot de passe de Bridge") : appPassword ? t("Mot de passe d'application") : t("Mot de passe")}
                  hint={
                    bridge
                      ? t("Affiché par Bridge dans les détails de votre compte (réglages IMAP et SMTP) : ce n'est pas votre mot de passe Proton.")
                      : google
                        ? t("À créer sur myaccount.google.com/apppasswords (validation en deux étapes requise) : votre mot de passe Google est refusé.")
                        : appPassword
                          ? t("À créer dans les réglages de sécurité de votre compte : votre mot de passe habituel est refusé.")
                          : undefined
                  }
                  required
                >
                  <Input type="password" value={password} onChange={(_, d) => setPassword(d.value)} onKeyDown={(e) => e.key === "Enter" && password && connect()} />
                </Field>
                <Field label={t("Nom du compte dans Vortex Mail")}>
                  <Input value={name} onChange={(_, d) => setName(d.value)} />
                </Field>
              </div>
            )}

            {found.helpUrl && (
              <div>
                <Button appearance="transparent" size="small" className="inline-link" onClick={() => api.openUrl(found.helpUrl).catch(() => {})}>
                  {t("Aide de votre fournisseur")}
                </Button>
              </div>
            )}
          </fieldset>
        )}
      </div>

      {(busy || report || error) && (
        <div className="settings-status" aria-live="polite">
          {busy && <Spinner size="tiny" label={t("Connexion aux serveurs…")} labelPosition="after" />}
          {!busy && error && (
            <MessageBar intent="error" layout="multiline">
              <MessageBarBody>
                <MessageBarTitle>{t("Échec")}</MessageBarTitle>
                {error}
              </MessageBarBody>
            </MessageBar>
          )}
          {!busy && report && (
            <div className="test-report failure">
              {check(t("IMAP :"), report.imap)}
              {check(t("SMTP :"), report.smtp)}
              {appPassword && <p className="muted small">{t("Ce fournisseur demande un mot de passe d'application, pas votre mot de passe habituel.")}</p>}
            </div>
          )}
        </div>
      )}

      <div className="settings-actions">
        {(step.kind === "found" || step.kind === "failed") && (
          <Button onClick={manual} disabled={busy}>
            {t("Modifier à la main")}
          </Button>
        )}
        <span className="spacer" />
        {step.kind === "address" && (
          <Button appearance="primary" disabled={!valid} onClick={search}>
            {t("Continuer")}
          </Button>
        )}
        {found && !browserLogin && (
          <Button appearance="primary" disabled={busy || !password || (bridge && !pin)} onClick={connect}>
            {t("Se connecter")}
          </Button>
        )}
        {found && browserLogin && (
          <Button appearance="primary" onClick={manual}>
            {t("Continuer")}
          </Button>
        )}
        {google && (
          <Button onClick={withGoogle} disabled={busy}>
            {t("Se connecter avec Google")}
          </Button>
        )}
        <Button onClick={onCancel} disabled={busy}>
          {t("Annuler")}
        </Button>
      </div>
    </div>
  );
}
