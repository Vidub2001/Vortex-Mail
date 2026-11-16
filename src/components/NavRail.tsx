import { Button } from "@fluentui/react-components";
import {
  CalendarLtr24Regular,
  Mail24Filled,
  PeopleList24Regular,
  Settings24Regular,
  WeatherMoon20Regular,
  WeatherSunny20Regular,
} from "@fluentui/react-icons";
import logo from "../assets/logo.png";
import type { Account } from "../types";
import { t } from "../i18n";

interface Props {
  accounts: Account[];
  /** Compte du dossier affiché ; null pour la vue unifiée. */
  activeAccountId: string | null;
  unifiedActive: boolean;
  dark: boolean;
  onUnified: () => void;
  onAccount: (accountId: string) => void;
  onToggleTheme: () => void;
  onSettings: () => void;
  /** Vue affichée : courrier, agenda ou contacts. */
  view: "mail" | "calendar" | "contacts";
  onView: (view: "calendar" | "contacts") => void;
}

/** Deux initiales : « Mailo 1 » → « M1 », « Gmail perso » → « GP ». */
function initials(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  const letters = words.length > 1 ? words[0][0] + words[1][0] : (words[0] ?? "?").slice(0, 2);
  return letters.toUpperCase();
}

/** Barre verticale de gauche : logo, boîte unifiée, un avatar par compte, réglages. */
export function NavRail(p: Props) {
  return (
    <nav className="navrail" aria-label={t("Comptes")}>
      <img className="navrail-logo" src={logo} alt="Vortex Mail" title="Vortex Mail" />
      <Button
        appearance="subtle"
        className={p.view === "mail" && p.unifiedActive ? "navrail-btn active" : "navrail-btn"}
        icon={<Mail24Filled />}
        title={t("Boîte de réception unifiée")}
        aria-label={t("Boîte de réception unifiée")}
        onClick={p.onUnified}
      />
      <div className="navrail-accounts">
        {p.accounts.map((a) => (
          <button
            key={a.id}
            type="button"
            className={p.view === "mail" && a.id === p.activeAccountId ? "navrail-account active" : "navrail-account"}
            title={`${a.name} — ${a.email}`}
            aria-label={a.name}
            onClick={() => p.onAccount(a.id)}
          >
            <span className="navrail-avatar" style={{ background: a.color }}>
              {initials(a.name)}
            </span>
          </button>
        ))}
      </div>
      <span className="navrail-spacer" />
      <Button
        appearance="subtle"
        className={p.view === "calendar" ? "navrail-btn active" : "navrail-btn"}
        icon={<CalendarLtr24Regular />}
        title={t("Agenda")}
        aria-label={t("Agenda")}
        onClick={() => p.onView("calendar")}
      />
      <Button
        appearance="subtle"
        className={p.view === "contacts" ? "navrail-btn active" : "navrail-btn"}
        icon={<PeopleList24Regular />}
        title={t("Contacts")}
        aria-label={t("Contacts")}
        onClick={() => p.onView("contacts")}
      />
      <Button
        appearance="subtle"
        className="navrail-btn"
        icon={p.dark ? <WeatherSunny20Regular /> : <WeatherMoon20Regular />}
        title={p.dark ? t("Thème clair") : t("Thème sombre")}
        aria-label={p.dark ? t("Thème clair") : t("Thème sombre")}
        onClick={p.onToggleTheme}
      />
      <Button
        appearance="subtle"
        className="navrail-btn"
        icon={<Settings24Regular />}
        title={t("Comptes et paramètres")}
        aria-label={t("Comptes et paramètres")}
        onClick={p.onSettings}
      />
    </nav>
  );
}
