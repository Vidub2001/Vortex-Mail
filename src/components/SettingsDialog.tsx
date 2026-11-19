import { GeneralSection } from "./GeneralSection";
import { isOutlookDomain } from "../providers";
import { AccountWizard } from "./AccountWizard";
import { BridgeCertificate, isLoopback } from "./BridgeCertificate";
import { AgendaSection } from "./AgendaSection";
import { RulesSection } from "./RulesSection";
import { SmimeSection } from "./SmimeSection";
import { CorrespondentsSection } from "./CorrespondentsSection";
import type { Rule } from "../rules";
import {
  Button,
  Checkbox,
  Dialog,
  DialogSurface,
  Field,
  Input,
  Menu,
  MenuItem,
  MenuList,
  MenuPopover,
  MenuTrigger,
  MessageBar,
  MessageBarActions,
  MessageBarBody,
  MessageBarTitle,
  Select,
  Spinner,
  Textarea,
} from "@fluentui/react-components";
import {
  Add20Regular,
  ArrowLeft20Regular,
  ArrowSync20Regular,
  CheckmarkCircle16Filled,
  Delete20Regular,
  Dismiss20Regular,
  DismissCircle16Filled,
  Edit20Regular,
  Info20Regular,
  Mail20Regular,
  MoreHorizontal20Regular,
  PersonMail20Regular,
  Settings20Regular,
  CalendarLtr20Regular,
  Filter20Regular,
} from "@fluentui/react-icons";
import { useEffect, useState, type ReactNode } from "react";
import { api } from "../api";
import { colorName } from "../colors";
import { t } from "../i18n";
import { AboutSection } from "./AboutSection";
import { BackupSection } from "./BackupSection";
import { DEFAULT_ATTACHMENT_WORDS, SYNC_CHOICES, loadSettings, saveSettings, useMailSettings, type MailSettings } from "../settings";
import type { ThemeMode } from "../theme";
import type { Account, AccountConfig, AuthMethod, Security, ServerConfig, TestReport } from "../types";

// ------------------------------------------------------------------ constantes

const COLORS = ["#0f6cbd", "#c4314b", "#107c10", "#8764b8", "#ca5010", "#038387"];


type ProviderNote = { intent: "info" | "warning"; title: string; body: string; link?: [string, string] };

/** Particularités des grands fournisseurs, affichées sous l'adresse. */
function providerNote(domain: string): ProviderNote | null {
  if (domain === "gmail.com" || domain === "googlemail.com") {
    return {
      intent: "info",
      title: t("Gmail : mot de passe d'application ou OAuth2"),
      body: t("saisissez un mot de passe d'application (validation en deux étapes requise, IMAP activé dans Gmail), ou choisissez l'authentification OAuth2 pour vous connecter avec Google, sans mot de passe."),
      link: [t("Créer un mot de passe d'application :"), "https://myaccount.google.com/apppasswords"],
    };
  }
  if (isOutlookDomain(domain)) {
    return {
      intent: "info",
      title: t("Outlook.com / O365 : connexion OAuth2"),
      body: t("saisissez l'ID d'application (inscription de l'application dans le portail d'administration O365) puis cliquez sur « Se connecter à Outlook.com »."),
    };
  }
  return null;
}

const SECURITY_OPTIONS: { value: Security; label: string }[] = [
  { value: "sslTls", label: "SSL/TLS" },
  { value: "startTls", label: "STARTTLS" },
  { value: "auto", label: t("Auto") },
  { value: "none", label: t("Aucun (non recommandé)") },
];

const AUTH_OPTIONS: { value: AuthMethod; label: string }[] = [
  { value: "normal", label: t("Mot de passe normal") },
  { value: "plain", label: "AUTHENTICATE PLAIN" },
  { value: "oauth2", label: t("OAuth2 (connexion par le navigateur)") },
];

// L'ID d'application est désormais gardé dans le trousseau du système :
// on efface l'ancienne copie qu'une version précédente laissait dans le navigateur.
try {
  localStorage.removeItem("fluxmail.microsoftClientId");
} catch {
  // stockage indisponible : rien à effacer
}


const DEFAULT_PORTS: Record<"imap" | "smtp", Record<Security, number>> = {
  imap: { sslTls: 993, startTls: 143, auto: 993, none: 143 },
  smtp: { sslTls: 465, startTls: 587, auto: 465, none: 25 },
};

type Notify = (title: string, body?: string, intent?: "success" | "error" | "info") => void;

// ------------------------------------------------------------------ fenêtre

type Section = "accounts" | "general" | "mail" | "rules" | "agenda" | "backup" | "about";

interface Props {
  open: boolean;
  onClose: () => void;
  /** Tous les comptes affichés (IMAP et démo). */
  accounts: Account[];
  /** Après ajout, modification ou suppression : recharger comptes et dossiers. */
  onChanged: () => void;
  notify: Notify;
  themeMode: ThemeMode;
  onThemeMode: (mode: ThemeMode) => void;
  /** Règle préremplie (clic droit sur un message) : ouvre « Règles » sur son éditeur. */
  ruleDraft?: Rule | null;
}

/** Fenêtre « Paramètres » : rubriques à gauche, contenu à droite. */
export function SettingsDialog({ open, onClose, accounts, onChanged, notify, themeMode, onThemeMode, ruleDraft }: Props) {
  const [section, setSection] = useState<Section>("accounts");
  useEffect(() => {
    if (open && ruleDraft) setSection("rules");
  }, [open, ruleDraft]);

  const nav: { id: Section; label: string; icon: JSX.Element; ready: boolean }[] = [
    { id: "accounts", label: t("Comptes"), icon: <PersonMail20Regular />, ready: true },
    { id: "general", label: t("Général"), icon: <Settings20Regular />, ready: true },
    { id: "mail", label: t("Courrier"), icon: <Mail20Regular />, ready: true },
    { id: "rules", label: t("Règles"), icon: <Filter20Regular />, ready: true },
    { id: "agenda", label: t("Agenda"), icon: <CalendarLtr20Regular />, ready: true },
    { id: "backup", label: t("Sauvegarde"), icon: <ArrowSync20Regular />, ready: true },
    { id: "about", label: t("À propos"), icon: <Info20Regular />, ready: true },
  ];

  return (
    <Dialog open={open} onOpenChange={(_, d) => !d.open && onClose()}>
      <DialogSurface className="settings-surface" aria-label={t("Paramètres")}>
        <div className="settings-layout">
          <nav className="settings-nav" aria-label={t("Rubriques")}>
            <h2 className="settings-title">{t("Paramètres")}</h2>
            {nav.map((n) => (
              <button
                key={n.id}
                type="button"
                className={n.id === section ? "settings-nav-item active" : "settings-nav-item"}
                disabled={!n.ready}
                title={n.ready ? undefined : t("Bientôt disponible")}
                onClick={() => setSection(n.id)}
              >
                {n.icon}
                <span>{n.label}</span>
              </button>
            ))}
          </nav>

          <section className="settings-main">
            <Button
              className="settings-close"
              appearance="subtle"
              icon={<Dismiss20Regular />}
              aria-label={t("Fermer")}
              title={t("Fermer")}
              onClick={onClose}
            />
            {section === "general" && <GeneralSection themeMode={themeMode} onThemeMode={onThemeMode} />}
            {section === "agenda" && <AgendaSection />}
            {section === "rules" && (
              <RulesSection key={ruleDraft?.id ?? "list"} accounts={accounts} notify={notify} initialRule={ruleDraft ?? null} />
            )}
            {section === "about" && <AboutSection />}
            {section === "backup" && <BackupSection accountCount={accounts.length} />}
            {section === "mail" && <MailSection accounts={accounts} />}
            {section === "accounts" && (
              <AccountsSection open={open} accounts={accounts} onChanged={onChanged} notify={notify} />
            )}
          </section>
        </div>
      </DialogSurface>
    </Dialog>
  );
}

// ------------------------------------------------------------------ rubrique Courrier

/** Mots qui déclenchent l'avertissement : saisie libre, enregistrée en quittant le champ. */
function AttachmentWords({ value, onSave }: { value: string; onSave: (words: string) => void }) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  return (
    <Field
      label={t("Mots recherchés (séparés par des virgules)")}
      hint={t("Recherchés dans l'objet et dans votre texte, sans le message cité ni la signature.")}
    >
      <Textarea
        value={text}
        rows={3}
        resize="vertical"
        onChange={(_, d) => setText(d.value)}
        onBlur={() => text !== value && onSave(text)}
      />
      <div className="mail-signature-actions">
        <Button size="small" appearance="subtle" onClick={() => onSave(DEFAULT_ATTACHMENT_WORDS)}>
          {t("Rétablir la liste par défaut")}
        </Button>
      </div>
    </Field>
  );
}

/** Synchronisation et signatures (une par compte). */
function MailSection({ accounts }: { accounts: Account[] }) {
  const settings = useMailSettings();
  const [accountId, setAccountId] = useState(accounts[0]?.id ?? "");
  const [text, setText] = useState(settings.signatures[accountId] ?? "");
  const [saved, setSaved] = useState(true);

  // Changement de compte : sa signature s'affiche.
  useEffect(() => {
    setText(loadSettings().signatures[accountId] ?? "");
    setSaved(true);
  }, [accountId]);

  const update = (patch: Partial<MailSettings>) => saveSettings({ ...loadSettings(), ...patch });

  const saveSignature = () => {
    const current = loadSettings();
    saveSettings({ ...current, signatures: { ...current.signatures, [accountId]: text } });
    setSaved(true);
  };

  return (
    <div className="settings-pane">
      <h3 className="settings-pane-title">{t("Courrier")}</h3>
      <div className="about-scroll">
        <fieldset className="account-fieldset">
          <legend>{t("Synchronisation")}</legend>
          <Field
            label={t("Vérifier les nouveaux messages toutes les")}
            hint={t("Les comptes compatibles reçoivent en plus les nouveaux messages instantanément.")}
          >
            <Select
              value={String(settings.syncMinutes)}
              onChange={(_, d) => update({ syncMinutes: Number(d.value) })}
              className="mail-sync-select"
            >
              {SYNC_CHOICES.map((m) => (
                <option key={m} value={m}>
                  {m > 1 ? t("{n} minutes", { n: m }) : t("1 minute")}
                </option>
              ))}
            </Select>
          </Field>
        </fieldset>

        <fieldset className="account-fieldset">
          <legend>{t("Pièces jointes")}</legend>
          <Checkbox
            checked={settings.attachmentReminder}
            onChange={(_, d) => update({ attachmentReminder: d.checked === true })}
            label={t("Avertir à l'envoi si le message parle d'une pièce jointe absente")}
          />
          {settings.attachmentReminder && (
            <AttachmentWords value={settings.attachmentWords} onSave={(w) => update({ attachmentWords: w })} />
          )}
        </fieldset>

        <fieldset className="account-fieldset">
          <legend>{t("Signature")}</legend>
          {accounts.length === 0 ? (
            <p className="muted">{t("Ajoutez d'abord un compte.")}</p>
          ) : (
            <>
              <Field label={t("Compte")}>
                <Select value={accountId} onChange={(_, d) => setAccountId(d.value)}>
                  {accounts.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.name} — {a.email}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field label={t("Signature ajoutée aux nouveaux messages de ce compte")}>
                <Textarea
                  value={text}
                  rows={6}
                  resize="vertical"
                  placeholder={t("Prénom Nom\n06 00 00 00 00")}
                  onChange={(_, d) => {
                    setText(d.value);
                    setSaved(false);
                  }}
                />
              </Field>
              <Checkbox
                checked={settings.signatureOnReplies}
                onChange={(_, d) => update({ signatureOnReplies: d.checked === true })}
                label={t("L'ajouter aussi aux réponses et transferts")}
              />
              <div className="mail-signature-actions">
                <Button appearance="primary" disabled={saved} onClick={saveSignature}>
                  {t("Enregistrer la signature")}
                </Button>
                {saved && (settings.signatures[accountId] ?? "") !== "" && <span className="muted">{t("Enregistrée")}</span>}
              </div>
            </>
          )}
        </fieldset>
        <fieldset className="account-fieldset">
          <legend>{t("Certificats des correspondants (S/MIME)")}</legend>
          <CorrespondentsSection />
        </fieldset>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ rubrique Comptes

/** Liste, assistant d'ajout (lot I2), formulaire complet prérempli par l'assistant, ou modification. */
type View = { kind: "list" } | { kind: "add" } | { kind: "manual"; config: AccountConfig } | { kind: "edit"; config: AccountConfig };

function AccountsSection({
  open,
  accounts,
  onChanged,
  notify,
}: {
  open: boolean;
  accounts: Account[];
  onChanged: () => void;
  notify: Notify;
}) {
  const [view, setView] = useState<View>({ kind: "list" });
  const [configs, setConfigs] = useState<AccountConfig[]>([]);
  const [confirmDelete, setConfirmDelete] = useState<AccountConfig | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = () =>
    api
      .listAccountConfigs()
      .then(setConfigs)
      .catch((e) => setError(String(e)));

  useEffect(() => {
    if (open) {
      setView({ kind: "list" });
      setConfirmDelete(null);
      setError(null);
      void reload();
    }
  }, [open]);

  const demoAccounts = accounts.filter((a) => !configs.some((c) => c.id === a.id));

  const remove = async (c: AccountConfig) => {
    setConfirmDelete(null);
    try {
      await api.removeAccount(c.id);
      notify(t("Compte supprimé"), t("{name} : les messages restent sur le serveur.", { name: c.name }), "info");
      onChanged();
      await reload();
    } catch (e) {
      setError(String(e));
    }
  };

  const done = async () => {
    onChanged();
    await reload();
    setView({ kind: "list" });
  };

  if (view.kind === "add") {
    return (
      <AccountWizard
        color={COLORS[configs.length % COLORS.length]}
        onManual={(config) => setView({ kind: "manual", config })}
        onCancel={() => setView({ kind: "list" })}
        onSaved={done}
        notify={notify}
      />
    );
  }

  if (view.kind !== "list") {
    return (
      <AccountForm
        initial={view.config}
        editing={view.kind === "edit"}
        onCancel={() => setView({ kind: "list" })}
        onSaved={done}
        notify={notify}
      />
    );
  }

  return (
    <div className="settings-pane">
      <h3 className="settings-pane-title">{t("Comptes de messagerie")}</h3>
      <p className="muted settings-intro">
        {t(
          "Les messages peuvent être déplacés d'un compte à l'autre par glisser-déposer ou avec « Déplacer vers ». Les mots de passe sont conservés dans le trousseau du système, jamais dans un fichier.",
        )}
      </p>

      {error && (
        <MessageBar intent="error">
          <MessageBarBody>{error}</MessageBarBody>
        </MessageBar>
      )}

      <div>
        <Button appearance="primary" icon={<Add20Regular />} onClick={() => setView({ kind: "add" })}>
          {t("Ajouter un compte")}
        </Button>
      </div>

      <div className="account-list">
        {configs.length === 0 && <p className="muted">{t("Aucun compte IMAP configuré pour l'instant.")}</p>}
        {configs.map((c) => (
          <div key={c.id}>
            <div className="account-row">
              <span className="account-avatar" style={{ background: c.color }}>
                {c.name.charAt(0).toUpperCase()}
              </span>
              <div className="account-row-text">
                <strong>{c.name}</strong>
                <span className="muted">{c.email}</span>
                <span className="muted small">
                  IMAP {c.imap.host}:{c.imap.port} · SMTP {c.smtp.host}:{c.smtp.port}
                </span>
              </div>
              <Menu positioning="below-end">
                <MenuTrigger disableButtonEnhancement>
                  <Button
                    appearance="subtle"
                    icon={<MoreHorizontal20Regular />}
                    aria-label={t("Actions pour {name}", { name: c.name })}
                    title={t("Plus d'actions")}
                  />
                </MenuTrigger>
                <MenuPopover>
                  <MenuList>
                    <MenuItem icon={<Edit20Regular />} onClick={() => setView({ kind: "edit", config: c })}>
                      {t("Modifier")}
                    </MenuItem>
                    <MenuItem icon={<Delete20Regular />} onClick={() => setConfirmDelete(c)}>
                      {t("Supprimer le compte")}
                    </MenuItem>
                  </MenuList>
                </MenuPopover>
              </Menu>
            </div>
            {confirmDelete?.id === c.id && (
              <MessageBar intent="warning" layout="multiline" className="account-confirm">
                <MessageBarBody>
                  <MessageBarTitle>{t("Supprimer « {name} » de Vortex Mail ?", { name: c.name })}</MessageBarTitle>
                  {t("Les messages restent sur le serveur ; seul le compte est retiré de l'application.")}
                </MessageBarBody>
                <MessageBarActions>
                  <Button appearance="primary" onClick={() => remove(c)}>
                    {t("Supprimer")}
                  </Button>
                  <Button onClick={() => setConfirmDelete(null)}>{t("Annuler")}</Button>
                </MessageBarActions>
              </MessageBar>
            )}
          </div>
        ))}
      </div>

      {demoAccounts.length > 0 && (
        <>
          <h4 className="settings-subtitle">{t("Comptes de démonstration")}</h4>
          <div className="account-list">
            {demoAccounts.map((a) => (
              <div key={a.id} className="account-row demo">
                <span className="account-avatar" style={{ background: a.color }}>
                  {a.name.charAt(0).toUpperCase()}
                </span>
                <div className="account-row-text">
                  <strong>{a.name}</strong>
                  <span className="muted">{a.email}</span>
                </div>
                <span className="demo-badge">{t("Démo")}</span>
              </div>
            ))}
          </div>
          <p className="muted small">
            {t(
              "Comptes fictifs en mémoire, pour essayer les déplacements entre comptes. Un message d'un vrai compte ne peut pas y être déplacé.",
            )}
          </p>
        </>
      )}
    </div>
  );
}

// ------------------------------------------------------------------ formulaire

function AccountForm({
  initial,
  editing,
  onCancel,
  onSaved,
  notify,
}: {
  initial: AccountConfig;
  editing: boolean;
  onCancel: () => void;
  onSaved: () => void;
  notify: Notify;
}) {
  const [form, setForm] = useState<AccountConfig>(initial);
  const [report, setReport] = useState<TestReport | null>(null);
  const [busy, setBusy] = useState<"test" | "save" | "oauth" | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** Connexion Outlook.com et ID d'application présents dans le trousseau (comptes OAuth2). */
  const [oauthState, setOauthState] = useState<{ signedIn: boolean; hasClientId: boolean } | null>(null);
  const signedIn = oauthState?.signedIn ?? null;
  const hasClientId = oauthState?.hasClientId ?? false;
  const oauthUser = (form.imap.username || form.email).trim();
  /** Agenda et contacts Gmail : connexion Google (jeton et application dans le trousseau). */
  const gmail = /(^|\.)(gmail|googlemail)\.com$/i.test(form.imap.host.trim());
  /** OAuth2 Outlook.com (bouton dans le cadre IMAP) ; pour Gmail, c'est la connexion Google. */
  const oauth = form.imap.auth === "oauth2" && !gmail;
  /** Gmail en OAuth2 : la connexion Google sert aussi au courrier. */
  const gmailOauth = form.imap.auth === "oauth2" && gmail;

  useEffect(() => {
    if (!oauth || !oauthUser.includes("@")) {
      setOauthState(null);
      return;
    }
    let cancelled = false;
    api.oauthStatus(oauthUser).then((st) => !cancelled && setOauthState(st)).catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [oauth, oauthUser]);

  /** Mailo / Netcourrier : agenda et contacts sur deux serveurs distincts, connus d'avance. */
  const mailo =
    /@(mailo\.com|netcourrier\.com|net-c\.[a-z]+)$/i.test(form.email.trim()) ||
    /(^|\.)(mailo|netcourrier)\.com$/i.test(form.imap.host.trim());
  const [googleState, setGoogleState] = useState<{ signedIn: boolean; hasClient: boolean } | null>(null);
  const [googleId, setGoogleId] = useState("");
  const [googleSecret, setGoogleSecret] = useState("");
  const [googleBusy, setGoogleBusy] = useState(false);
  const googleUser = oauthUser;

  useEffect(() => {
    if (!gmail || !googleUser.includes("@")) {
      setGoogleState(null);
      return;
    }
    let cancelled = false;
    api.googleStatus(googleUser).then((st) => !cancelled && setGoogleState(st)).catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [gmail, googleUser]);

  const googleSignIn = async () => {
    setGoogleBusy(true);
    setError(null);
    try {
      await api.googleSignIn(googleUser, googleId, googleSecret, gmailOauth);
      setGoogleState({ signedIn: true, hasClient: true });
      // ID et secret sont maintenant dans le trousseau : plus affichés.
      setGoogleId("");
      setGoogleSecret("");
      notify(gmailOauth ? t("Courrier, agenda et contacts Google connectés") : t("Agenda et contacts Google connectés"), googleUser);
      window.dispatchEvent(new Event("vortex:pim-sync"));
    } catch (e) {
      setError(String(e));
    } finally {
      setGoogleBusy(false);
    }
  };

  const googleSignOut = async () => {
    setGoogleBusy(true);
    setError(null);
    try {
      await api.googleSignOut(form.id, googleUser);
      setGoogleState((st) => ({ hasClient: st?.hasClient ?? false, signedIn: false }));
      notify(t("Agenda et contacts Google déconnectés"), googleUser);
      window.dispatchEvent(new Event("vortex:pim-sync"));
    } catch (e) {
      setError(String(e));
    } finally {
      setGoogleBusy(false);
    }
  };

  const signIn = async () => {
    setBusy("oauth");
    setError(null);
    setReport(null);
    try {
      const report = await api.oauthSignIn(oauthUser, form.oauthClientId);
      setOauthState({ signedIn: true, hasClientId: true });
      if (report.calendar) window.dispatchEvent(new Event("vortex:pim-sync"));
      // L'ID est maintenant dans le trousseau : il ne reste pas affiché.
      update({ oauthClientId: "" });
      notify(
        t("Connecté à Outlook.com"),
        report.calendar
          ? t("{user} : courrier, agenda et contacts", { user: oauthUser })
          : t("{user} : courrier (agenda et contacts non autorisés)", { user: oauthUser }),
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const update = (patch: Partial<AccountConfig>) => {
    setForm((f) => ({ ...f, ...patch }));
    setReport(null);
  };

  const updateServer = (kind: "imap" | "smtp", patch: Partial<ServerConfig>) => {
    setForm((f) => {
      const current = f[kind];
      const next = { ...current, ...patch };
      // Changement de chiffrement : le port suit, s'il était sur une valeur par défaut.
      if (patch.security && Object.values(DEFAULT_PORTS[kind]).includes(current.port)) {
        next.port = DEFAULT_PORTS[kind][patch.security];
      }
      return { ...f, [kind]: next };
    });
    setReport(null);
  };

  /** À la saisie de l'adresse : identifiant et nom (seulement les champs vides) ; les serveurs viennent de l'assistant. */
  const applyEmail = () => {
    const email = form.email.trim();
    const domain = email.split("@")[1]?.toLowerCase() ?? "";
    setForm((f) => {
      const next = { ...f, imap: { ...f.imap }, smtp: { ...f.smtp } };
      if (!next.imap.username) next.imap.username = email;
      if (!next.smtp.username) next.smtp.username = email;
      if (!next.name && domain) next.name = domain.split(".")[0].replace(/^./, (c) => c.toUpperCase());
      // Outlook.com : connexion OAuth2 obligatoire, mêmes identifiants pour l'envoi.
      if (isOutlookDomain(domain)) {
        next.imap.auth = "oauth2";
        next.smtp.auth = "oauth2";
        next.smtpSameCredentials = true;
      }
      return next;
    });
    // Nouveau compte Gmail : OAuth2 par défaut si une application Google est déjà
    // enregistrée sur ce PC (le mot de passe d'application reste au choix).
    if (!editing && (domain === "gmail.com" || domain === "googlemail.com")) {
      api
        .googleStatus(email)
        .then((st) => {
          if (!st.hasClient) return;
          setForm((f) =>
            f.imap.auth !== "normal" || f.imap.password
              ? f
              : {
                  ...f,
                  imap: { ...f.imap, auth: "oauth2" },
                  smtp: { ...f.smtp, auth: "oauth2" },
                  smtpSameCredentials: true,
                },
          );
        })
        .catch(() => {});
    }
  };

  const test = async () => {
    setBusy("test");
    setError(null);
    setReport(null);
    try {
      setReport(await api.testAccount(form));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const save = async () => {
    setBusy("save");
    setError(null);
    try {
      const account = editing ? await api.updateAccount(form) : await api.addAccount(form);
      notify(editing ? t("Compte modifié") : t("Compte ajouté"), `${account.name} (${account.email})`);
      onSaved();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const serverFields = (kind: "imap" | "smtp", label: string, showCredentials: boolean, header?: ReactNode) => {
    const s = form[kind];
    const oauthControls = (
      <>
        <Button
          appearance="primary"
          icon={busy === "oauth" ? <Spinner size="tiny" /> : undefined}
          disabled={busy !== null || !oauthUser.includes("@") || (form.oauthClientId.length < 30 && !hasClientId)}
          onClick={signIn}
        >
          {busy === "oauth" ? t("En attente du navigateur…") : signedIn ? t("Se reconnecter") : t("Se connecter à Outlook.com")}
        </Button>
        {signedIn === true && (
          <span className="oauth-status ok">
            <CheckmarkCircle16Filled /> {t("Connecté")}
          </span>
        )}
        {signedIn === false && <span className="oauth-status">{t("Non connecté")}</span>}
      </>
    );
    // Gmail en OAuth2 : même disposition que pour Outlook.com (bouton dans le cadre IMAP).
    const googleControls = (
      <>
        <Button
          appearance="primary"
          icon={googleBusy ? <Spinner size="tiny" /> : undefined}
          disabled={
            googleBusy ||
            busy !== null ||
            !googleUser.includes("@") ||
            (!googleState?.hasClient && (!googleId.endsWith(".apps.googleusercontent.com") || googleSecret.length < 10))
          }
          onClick={googleSignIn}
        >
          {googleBusy ? t("En attente du navigateur…") : googleState?.signedIn ? t("Se reconnecter") : t("Se connecter avec Google")}
        </Button>
        {googleState?.signedIn === true && (
          <>
            <span className="oauth-status ok">
              <CheckmarkCircle16Filled /> {t("Connecté")}
            </span>
            {editing && (
              <Button appearance="subtle" size="small" disabled={googleBusy} onClick={googleSignOut}>
                {t("Déconnecter")}
              </Button>
            )}
          </>
        )}
        {googleState?.signedIn === false && <span className="oauth-status">{t("Non connecté")}</span>}
      </>
    );
    const googleNeedsClient = gmailOauth && googleState !== null && !googleState.hasClient;
    return (
      <fieldset className="account-fieldset">
        <legend>{label}</legend>
        {header}
        {showCredentials && (
          <div className="form-row">
            <Field label={t("Nom d'utilisateur")} required>
              <Input value={s.username} onChange={(_, d) => updateServer(kind, { username: d.value })} />
            </Field>
            {kind === "imap" && gmailOauth && !googleNeedsClient ? (
              // Application Google déjà dans le trousseau : seulement le bouton de connexion.
              <div className="oauth-cell">{googleControls}</div>
            ) : kind === "imap" && gmailOauth ? (
              <Field label={t("ID client Google")} required>
                <Input
                  type="password"
                  autoComplete="off"
                  value={googleId}
                  placeholder="….apps.googleusercontent.com"
                  onChange={(_, d) => setGoogleId(d.value.trim())}
                />
              </Field>
            ) : kind === "imap" && oauth && hasClientId ? (
              // ID d'application déjà dans le trousseau : on ne l'affiche plus du tout.
              <div className="oauth-cell">{oauthControls}</div>
            ) : kind === "imap" && oauth ? (
              <Field label={t("ID d'application (client)")} required>
                <Input
                  type="password"
                  autoComplete="off"
                  value={form.oauthClientId}
                  placeholder={t("Copié depuis le portail d'administration O365")}
                  onChange={(_, d) => update({ oauthClientId: d.value.trim() })}
                />
              </Field>
            ) : (
              <Field label={t("Mot de passe")} required={!editing}>
                <Input
                  type="password"
                  value={s.password}
                  placeholder={editing ? t("Inchangé") : undefined}
                  onChange={(_, d) => updateServer(kind, { password: d.value })}
                />
              </Field>
            )}
          </div>
        )}
        {showCredentials && kind === "imap" && oauth && !hasClientId && <div className="oauth-row">{oauthControls}</div>}
        {showCredentials && kind === "imap" && googleNeedsClient && (
          <>
            <div className="form-row">
              <Field label={t("Code secret client")} required>
                <Input
                  type="password"
                  autoComplete="off"
                  value={googleSecret}
                  placeholder="GOCSPX-…"
                  onChange={(_, d) => setGoogleSecret(d.value.trim())}
                />
              </Field>
              <div className="oauth-cell">{googleControls}</div>
            </div>
            <div className="google-help">
              {t("Votre propre application Google (gratuite), à créer une seule fois :")}{" "}
              <Button
                appearance="transparent"
                size="small"
                className="inline-link"
                onClick={() => api.openUrl("https://console.cloud.google.com/apis/credentials").catch(() => {})}
              >
                {t("Ouvrir la console Google Cloud")}
              </Button>
            </div>
          </>
        )}
        <div className="form-row server-line">
          <Field label={t("Serveur")} required>
            <Input
              value={s.host}
              placeholder={kind === "imap" ? t("imap.exemple.com") : t("smtp.exemple.com")}
              onChange={(_, d) => updateServer(kind, { host: d.value.trim() })}
            />
          </Field>
          <Field label={t("Port")} required>
            <Input
              type="number"
              value={String(s.port)}
              onChange={(_, d) => updateServer(kind, { port: Number(d.value) || 0 })}
            />
          </Field>
          <Field label={t("Sécurité")}>
            <Select value={s.security} onChange={(_, d) => updateServer(kind, { security: d.value as Security })}>
              {SECURITY_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </Select>
          </Field>
          {kind === "imap" && (
            <Field label={t("Authentification")}>
              <Select value={s.auth} onChange={(_, d) => updateServer(kind, { auth: d.value as AuthMethod })}>
                {AUTH_OPTIONS.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </Select>
            </Field>
          )}
        </div>
        {s.security === "none" && (
          <MessageBar intent="warning">
            <MessageBarBody>{t("Sans chiffrement, le mot de passe circule en clair sur le réseau.")}</MessageBarBody>
          </MessageBar>
        )}
      </fieldset>
    );
  };

  const check = (label: string, r: { ok: boolean; message: string }) => (
    <div className={r.ok ? "check-line ok" : "check-line ko"}>
      {r.ok ? <CheckmarkCircle16Filled /> : <DismissCircle16Filled />}
      <strong>{label}</strong> {r.message}
    </div>
  );

  return (
    <div className="settings-pane">
      <div className="settings-breadcrumb">
        <Button appearance="subtle" icon={<ArrowLeft20Regular />} aria-label={t("Retour")} title={t("Retour")} onClick={onCancel} />
        <h3 className="settings-pane-title">{editing ? t("Modifier « {name} »", { name: initial.name }) : t("Ajouter un compte IMAP")}</h3>
      </div>

      <div className="settings-form">
        <fieldset className="account-fieldset">
          <legend>{t("Compte")}</legend>
          {(() => {
            const note = providerNote(form.email.split("@")[1]?.trim().toLowerCase() ?? "");
            return (
              note && (
                <MessageBar intent={note.intent} layout="multiline" className="provider-note">
                  <MessageBarBody>
                    <MessageBarTitle>{note.title}</MessageBarTitle> {note.body}{" "}
                    {note.link && (
                      <span>
                        {note.link[0]} <code className="provider-url">{note.link[1]}</code>
                      </span>
                    )}
                  </MessageBarBody>
                </MessageBar>
              )
            );
          })()}
          <div className="form-row account-row-fields">
            <Field label={t("Adresse e-mail")} required>
              <Input
                type="email"
                value={form.email}
                placeholder={t("prenom.nom@mailo.com")}
                onChange={(_, d) => update({ email: d.value })}
                onBlur={applyEmail}
              />
            </Field>
            <Field label={t("Nom affiché du compte")} required>
              <Input value={form.name} onChange={(_, d) => update({ name: d.value })} />
            </Field>
            <Field label={t("Votre nom")}>
              <Input
                value={form.displayName ?? ""}
                title={t("Nom vu par les destinataires de vos messages")}
                placeholder={t("Vu par vos destinataires")}
                onChange={(_, d) => update({ displayName: d.value })}
              />
            </Field>
            <Field label={t("Couleur")}>
              <div className="color-swatches">
                {COLORS.map((c) => (
                  <button
                    key={c}
                    type="button"
                    className={c === form.color ? "swatch selected" : "swatch"}
                    style={{ background: c }}
                    aria-label={colorName(c)}
                    title={colorName(c)}
                    onClick={() => update({ color: c })}
                  />
                ))}
              </div>
            </Field>
          </div>
          {/* Lot J11b : pour une très grosse boîte ou un réseau limité. */}
          <Checkbox
            checked={form.offlineBodies !== false}
            label={t("Télécharger le contenu des messages pour les lire hors connexion")}
            title={t("Décoché : seuls les en-têtes sont gardés ; un message est lu sur le serveur à son ouverture.")}
            onChange={(_, d) => update({ offlineBodies: d.checked === true })}
          />
        </fieldset>

        <div className="servers-grid">
          {serverFields("imap", t("Serveur entrant (IMAP)"), true)}
          {serverFields(
            "smtp",
            t("Serveur sortant (SMTP)"),
            !form.smtpSameCredentials,
            <Checkbox
              checked={form.smtpSameCredentials}
              label={t("Mêmes identifiants que le serveur entrant")}
              onChange={(_, d) => update({ smtpSameCredentials: d.checked === true })}
            />,
          )}
        </div>

        {/* Proton Mail Bridge sur cet ordinateur : certificat épinglé (lot I3). */}
        {isLoopback(form.imap.host) && (
          <fieldset className="account-fieldset">
            <legend>Proton Mail Bridge</legend>
            <p className="muted small">
              {t(
                "Bridge chiffre la connexion avec un certificat qui lui est propre. Vortex Mail retient son empreinte et refuse la connexion si elle change ; après une réinstallation de Bridge, relisez-le ici.",
              )}
            </p>
            <BridgeCertificate
              port={form.imap.port}
              security={form.imap.security}
              value={form.imap.pinnedCert ?? ""}
              onRead={(fingerprint) => {
                updateServer("imap", { pinnedCert: fingerprint });
                updateServer("smtp", { pinnedCert: fingerprint });
              }}
            />
          </fieldset>
        )}

        {/* Outlook.com : agenda et contacts connectés avec le courrier (bouton de connexion ci-dessus). */}
        {!oauth && !gmailOauth && (
        <fieldset className="account-fieldset">
          <legend>{t("Contacts et agenda")}</legend>
          {gmail ? (
            <>
              {googleState && !googleState.hasClient && (
                <>
                  <div className="google-help">
                    {t(
                      "Gmail n'accepte pas le mot de passe d'application pour l'agenda et les contacts : il faut votre propre application Google (gratuite), à créer une seule fois.",
                    )}{" "}
                    <Button
                      appearance="transparent"
                      size="small"
                      className="inline-link"
                      onClick={() => api.openUrl("https://console.cloud.google.com/apis/credentials").catch(() => {})}
                    >
                      {t("Ouvrir la console Google Cloud")}
                    </Button>
                  </div>
                  <div className="form-row">
                    <Field label={t("ID client Google")} required>
                      <Input
                        type="password"
                        autoComplete="off"
                        value={googleId}
                        placeholder="….apps.googleusercontent.com"
                        onChange={(_, d) => setGoogleId(d.value.trim())}
                      />
                    </Field>
                    <Field label={t("Code secret client")} required>
                      <Input
                        type="password"
                        autoComplete="off"
                        value={googleSecret}
                        placeholder="GOCSPX-…"
                        onChange={(_, d) => setGoogleSecret(d.value.trim())}
                      />
                    </Field>
                  </div>
                </>
              )}
              <div className="oauth-row">
                <Button
                  appearance={googleState?.signedIn ? "secondary" : "primary"}
                  icon={googleBusy ? <Spinner size="tiny" /> : undefined}
                  disabled={
                    googleBusy ||
                    busy !== null ||
                    !googleUser.includes("@") ||
                    (!googleState?.hasClient && (!googleId.endsWith(".apps.googleusercontent.com") || googleSecret.length < 10))
                  }
                  onClick={googleSignIn}
                >
                  {googleBusy ? t("En attente du navigateur…") : googleState?.signedIn ? t("Se reconnecter") : t("Se connecter avec Google")}
                </Button>
                {googleState?.signedIn === true && (
                  <>
                    <span className="oauth-status ok">
                      <CheckmarkCircle16Filled /> {t("Agenda et contacts connectés")}
                    </span>
                    {editing && (
                      <Button appearance="subtle" size="small" disabled={googleBusy} onClick={googleSignOut}>
                        {t("Déconnecter")}
                      </Button>
                    )}
                  </>
                )}
                {googleState?.signedIn === false && <span className="oauth-status">{t("Non connecté")}</span>}
              </div>
            </>
          ) : (
            <>
              <div className="google-help">
                {t("Vide : recherche automatique (mêmes identifiants que le serveur entrant). « - » pour désactiver.")}
              </div>
              <div className="form-row">
                <Field label={t("Agenda (CalDAV)")}>
                  <Input
                    value={form.davUrl ?? ""}
                    placeholder={mailo ? t("Automatique (Mailo / Netcourrier)") : t("Recherche automatique")}
                    onChange={(_, d) => update({ davUrl: d.value.trim() })}
                  />
                </Field>
                <Field label={t("Contacts (CardDAV)")}>
                  <Input
                    value={form.cardUrl ?? ""}
                    placeholder={mailo ? t("Automatique (Mailo / Netcourrier)") : t("Même serveur que l'agenda")}
                    onChange={(_, d) => update({ cardUrl: d.value.trim() })}
                  />
                </Field>
              </div>
            </>
          )}
        </fieldset>
        )}

        {/* Signature S/MIME (lot D2) : pour un compte déjà enregistré. */}
        {editing && initial.id && (
          <fieldset className="account-fieldset">
            <legend>{t("Signature S/MIME")}</legend>
            <SmimeSection accountId={initial.id} email={initial.email} />
          </fieldset>
        )}
      </div>

      {/* Zone toujours visible, juste au-dessus des boutons. */}
      {(busy === "test" || report || error) && (
        <div className="settings-status" aria-live="polite">
          {busy === "test" && <Spinner size="tiny" label={t("Connexion aux serveurs…")} labelPosition="after" />}
          {busy !== "test" && error && (
            <MessageBar intent="error" layout="multiline">
              <MessageBarBody>
                <MessageBarTitle>{t("Échec")}</MessageBarTitle>
                {error}
              </MessageBarBody>
            </MessageBar>
          )}
          {busy !== "test" && report && (
            <div className={report.imap.ok && report.smtp.ok ? "test-report success" : "test-report failure"}>
              {check(t("IMAP :"), report.imap)}
              {check(t("SMTP :"), report.smtp)}
            </div>
          )}
        </div>
      )}

      <div className="settings-actions">
        <Button onClick={test} disabled={busy !== null}>
          {t("Tester les paramètres")}
        </Button>
        <span className="spacer" />
        <Button appearance="primary" onClick={save} disabled={busy !== null}>
          {busy === "save" ? <Spinner size="tiny" label={t("Connexion…")} /> : editing ? t("Enregistrer") : t("Ajouter le compte")}
        </Button>
        <Button onClick={onCancel} disabled={busy !== null}>
          {t("Annuler")}
        </Button>
      </div>
    </div>
  );
}
