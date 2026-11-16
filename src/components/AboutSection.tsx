// Paramètres › À propos : version, crédits et licences des composants tiers.
import { Button, SearchBox, Spinner } from "@fluentui/react-components";
import { ChevronDown16Regular, ChevronRight16Regular, Open16Regular } from "@fluentui/react-icons";
import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useMemo, useState } from "react";
import { api, runningInTauri } from "../api";
import logo from "../assets/logo.png";
import { t } from "../i18n";

interface Pkg {
  name: string;
  version: string;
  license: string;
  url: string;
  /** Index dans `texts`. */
  texts: number[];
}

interface Inventory {
  generated: string;
  npm: Pkg[];
  rust: Pkg[];
  texts: string[];
}

const CREDITS = [
  ["Tauri", t("fenêtres et pont avec le système"), "https://tauri.app"],
  ["Rust", t("moteur IMAP, SMTP, cache et sécurité"), "https://www.rust-lang.org"],
  ["React", t("interface"), "https://react.dev"],
  ["Fluent UI", t("composants et icônes (licence MIT)"), "https://react.fluentui.dev"],
  ["SQLite", t("cache local des messages"), "https://sqlite.org"],
] as const;

export function AboutSection() {
  const [version, setVersion] = useState<string | null>(null);
  const [inventory, setInventory] = useState<Inventory | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<string | null>(null);

  useEffect(() => {
    if (runningInTauri) getVersion().then(setVersion, () => setVersion("?"));
    else setVersion(t("aperçu navigateur"));
    // Chargé à la demande : l'inventaire n'alourdit pas le démarrage.
    import("../assets/licenses.json").then(
      (m) => setInventory(m.default as Inventory),
      (e) => setLoadError(String(e)),
    );
  }, []);

  const summary = useMemo(() => {
    if (!inventory) return [];
    const counts = new Map<string, number>();
    for (const p of [...inventory.npm, ...inventory.rust]) counts.set(p.license, (counts.get(p.license) ?? 0) + 1);
    return [...counts].sort((a, b) => b[1] - a[1]);
  }, [inventory]);

  const filter = (list: Pkg[]) => {
    const q = query.trim().toLowerCase();
    return q ? list.filter((p) => p.name.toLowerCase().includes(q) || p.license.toLowerCase().includes(q)) : list;
  };

  const openLink = (url: string) => {
    if (url) api.openUrl(url).catch(() => {});
  };

  const group = (title: string, list: Pkg[], kind: string) => {
    const shown = filter(list);
    if (shown.length === 0) return null;
    return (
      <div className="about-group">
        <h5>
          {title} <span className="muted">· {shown.length}</span>
        </h5>
        {shown.map((p) => {
          const key = `${kind}:${p.name}@${p.version}`;
          const expanded = open === key;
          return (
            <div key={key} className={expanded ? "about-pkg open" : "about-pkg"}>
              <button type="button" className="about-pkg-row" onClick={() => setOpen(expanded ? null : key)} aria-expanded={expanded}>
                {expanded ? <ChevronDown16Regular /> : <ChevronRight16Regular />}
                <span className="about-pkg-name">{p.name}</span>
                <span className="muted about-pkg-version">{p.version}</span>
                <span className="about-pkg-license">{p.license}</span>
              </button>
              {expanded && (
                <div className="about-pkg-detail">
                  {p.url && (
                    <Button size="small" appearance="transparent" icon={<Open16Regular />} onClick={() => openLink(p.url)}>
                      {p.url.replace(/^https?:\/\//, "")}
                    </Button>
                  )}
                  {p.texts.length > 0 ? (
                    p.texts.map((i) => (
                      <pre key={i} className="about-license-text">
                        {inventory?.texts[i]}
                      </pre>
                    ))
                  ) : (
                    <p className="muted">{t("Pas de fichier de licence fourni avec le paquet ; licence déclarée : {license}.", { license: p.license })}</p>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    );
  };

  return (
    <div className="settings-pane about-pane">
      <h3 className="settings-pane-title">{t("À propos")}</h3>

      <div className="about-scroll">
        <div className="about-hero">
          <img src={logo} alt="" className="about-logo" />
          <div>
            <div className="about-name">Vortex Mail</div>
            <div className="muted">{t("Version {version}", { version: version ?? "…" })}</div>
            <div className="about-tagline">{t("Client de messagerie multi-comptes")}</div>
            <div className="muted about-copyright">{t("© {year} Fabrice Joffre. Tous droits réservés.", { year: new Date().getFullYear() })}</div>
          </div>
        </div>

        <fieldset className="account-fieldset">
          <legend>{t("Construit avec")}</legend>
          <ul className="about-credits">
            {CREDITS.map(([name, role, url]) => (
              <li key={name}>
                <button type="button" className="about-link" onClick={() => openLink(url)}>
                  {name}
                </button>{" "}
                <span className="muted">— {role}</span>
              </li>
            ))}
          </ul>
        </fieldset>

        <fieldset className="account-fieldset">
          <legend>{t("Composants tiers")}</legend>
          {loadError ? (
            <p className="muted">
              {t("Liste des licences indisponible ({error}). Le fichier THIRD_PARTY_LICENSES.txt la contient aussi.", {
                error: loadError,
              })}
            </p>
          ) : !inventory ? (
            <Spinner size="tiny" />
          ) : (
            <>
              <p className="about-intro">
                {t(
                  "Vortex Mail utilise {n} bibliothèques open source, sous les licences suivantes. Le texte de chaque licence s'affiche en cliquant sur la bibliothèque.",
                  { n: inventory.npm.length + inventory.rust.length },
                )}
              </p>
              <div className="about-badges">
                {summary.map(([license, n]) => (
                  <button key={license} type="button" className="about-badge" onClick={() => setQuery(license)} title={t("Filtrer")}>
                    {license} <b>{n}</b>
                  </button>
                ))}
              </div>
              <SearchBox
                size="small"
                placeholder={t("Rechercher une bibliothèque ou une licence")}
                value={query}
                onChange={(_, d) => setQuery(d.value)}
                className="about-search"
              />
              {group(t("Application (Rust)"), inventory.rust, "rust")}
              {group(t("Interface (JavaScript)"), inventory.npm, "npm")}
              {inventory.rust.length === 0 && (
                <p className="muted">{t("La liste des bibliothèques Rust sera complétée par « npm run licenses ».")}</p>
              )}
              <p className="muted about-generated">{t("Inventaire du {date}.", { date: inventory.generated })}</p>
            </>
          )}
        </fieldset>
      </div>
    </div>
  );
}
