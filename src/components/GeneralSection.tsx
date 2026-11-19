// Paramètres › Général : apparence, disposition, dates et heures, rédaction, contacts.
import { Field, Radio, RadioGroup, Select, Switch } from "@fluentui/react-components";
import type { ReactNode } from "react";
import {
  DATE_CHOICES,
  TIME_CHOICES,
  saveGeneral,
  useGeneral,
  type DateFormat,
  type Layout,
  type NameOrder,
  type TimeFormat,
} from "../general";
import type { ThemeMode } from "../theme";
import { LANGUAGES, t, type Lang } from "../i18n";

interface Props {
  themeMode: ThemeMode;
  onThemeMode: (mode: ThemeMode) => void;
}

/** Vignette de disposition : dossiers à gauche, liste (lignes), volet de lecture. */
function LayoutPicture({ layout }: { layout: Layout }) {
  const rows = (x: number, y: number, w: number, n: number) =>
    Array.from({ length: n }, (_, i) => <rect key={i} x={x} y={y + i * 7} width={w} height={3} rx={1.5} className="lp-line" />);
  let body: ReactNode;
  if (layout === "right") {
    body = (
      <>
        <rect x={24} y={4} width={34} height={52} rx={3} className="lp-list" />
        {rows(28, 10, 26, 6)}
        <rect x={62} y={4} width={50} height={52} rx={3} className="lp-reading" />
        {rows(67, 12, 34, 4)}
      </>
    );
  } else if (layout === "bottom") {
    body = (
      <>
        <rect x={24} y={4} width={88} height={22} rx={3} className="lp-list" />
        {rows(28, 9, 76, 3)}
        <rect x={24} y={30} width={88} height={26} rx={3} className="lp-reading" />
        {rows(29, 36, 60, 3)}
      </>
    );
  } else {
    body = (
      <>
        <rect x={24} y={4} width={88} height={52} rx={3} className="lp-list" />
        {rows(28, 10, 80, 6)}
      </>
    );
  }
  return (
    <svg viewBox="0 0 116 60" width={116} height={60} aria-hidden="true" className="layout-picture">
      <rect x={4} y={4} width={16} height={52} rx={3} className="lp-folders" />
      {body}
    </svg>
  );
}

const LAYOUTS: { value: Layout; label: string }[] = [
  { value: "right", label: t("À droite") },
  { value: "bottom", label: t("En bas") },
  { value: "hidden", label: t("Masqué") },
];

export function GeneralSection({ themeMode, onThemeMode }: Props) {
  const g = useGeneral();
  return (
    <div className="settings-pane">
      <h3 className="settings-pane-title">{t("Général")}</h3>

      <fieldset className="account-fieldset">
        <legend>{t("Langue")}</legend>
        <Field label={t("Langue de l'interface")} hint={t("Vortex Mail redémarre l'affichage pour appliquer la langue.")}>
          <Select
            value={g.language}
            onChange={(_, d) => {
              saveGeneral({ language: d.value as Lang });
              // Tous les textes et formats sont relus dans la nouvelle langue.
              window.location.reload();
            }}
          >
            {LANGUAGES.map((l) => (
              <option key={l.value} value={l.value}>
                {l.label}
              </option>
            ))}
          </Select>
        </Field>
      </fieldset>

      <fieldset className="account-fieldset">
        <legend>{t("Apparence")}</legend>
        <RadioGroup value={themeMode} onChange={(_, d) => onThemeMode(d.value as ThemeMode)}>
          <Radio value="system" label={t("Comme le système")} />
          <Radio value="light" label={t("Clair")} />
          <Radio value="dark" label={t("Sombre")} />
        </RadioGroup>
      </fieldset>

      <fieldset className="account-fieldset">
        <legend>{t("Volet de lecture")}</legend>
        <div className="layout-choices" role="radiogroup" aria-label={t("Volet de lecture")}>
          {LAYOUTS.map((l) => (
            <button
              key={l.value}
              type="button"
              role="radio"
              aria-checked={g.layout === l.value}
              className={g.layout === l.value ? "layout-choice on" : "layout-choice"}
              onClick={() => saveGeneral({ layout: l.value })}
            >
              <LayoutPicture layout={l.value} />
              <span>{l.label}</span>
            </button>
          ))}
        </div>
        {g.layout === "hidden" && (
          <p className="muted settings-hint">{t("Double-cliquez sur un message pour l'ouvrir dans une fenêtre.")}</p>
        )}
      </fieldset>

      <fieldset className="account-fieldset">
        <legend>{t("Date et heure")}</legend>
        <div className="settings-row">
          <Field label={t("Format de la date")}>
            <Select value={g.dateFormat} onChange={(_, d) => saveGeneral({ dateFormat: d.value as DateFormat })}>
              {DATE_CHOICES.map((c) => (
                <option key={c.value} value={c.value}>
                  {c.label}
                </option>
              ))}
            </Select>
          </Field>
          <Field label={t("Format de l'heure")}>
            <Select value={g.timeFormat} onChange={(_, d) => saveGeneral({ timeFormat: d.value as TimeFormat })}>
              {TIME_CHOICES.map((c) => (
                <option key={c.value} value={c.value}>
                  {c.label}
                </option>
              ))}
            </Select>
          </Field>
        </div>
      </fieldset>

      <fieldset className="account-fieldset">
        <legend>{t("Rédaction")}</legend>
        <RadioGroup
          value={g.composeFormat}
          onChange={(_, d) => saveGeneral({ composeFormat: d.value as "html" | "text" })}
        >
          <Radio value="html" label={t("HTML (mise en forme) par défaut")} />
          <Radio value="text" label={t("Texte simple par défaut (sans barre de mise en forme)")} />
        </RadioGroup>
        <p className="muted settings-hint">{t("Le format peut toujours être changé dans chaque message.")}</p>
      </fieldset>

      <fieldset className="account-fieldset">
        <legend>{t("Arrière-plan")}</legend>
        <Switch
          checked={g.keepInTray}
          onChange={(_, d) => saveGeneral({ keepInTray: d.checked })}
          label={t("Garder Vortex Mail dans la zone de notification quand la fenêtre est fermée")}
        />
        <p className="muted settings-hint">
          {t("Les nouveaux messages, les règles et les rappels continuent. Pour quitter : menu de l'icône, « Quitter ».")}
        </p>
      </fieldset>

      <fieldset className="account-fieldset">
        <legend>{t("Mises à jour")}</legend>
        <Switch
          checked={g.checkUpdates}
          onChange={(_, d) => saveGeneral({ checkUpdates: d.checked })}
          label={t("Signaler les nouvelles versions")}
        />
        <p className="muted settings-hint">
          {t("Une fois par jour, Vortex Mail demande à GitHub le numéro de la dernière version ; GitHub voit alors votre adresse IP. Rien n'est téléchargé ni installé sans vous.")}
        </p>
      </fieldset>

      <fieldset className="account-fieldset">
        <legend>{t("Contacts")}</legend>
        <RadioGroup value={g.nameOrder} onChange={(_, d) => saveGeneral({ nameOrder: d.value as NameOrder })}>
          <Radio value="first-last" label={t("Prénom Nom (Marie Dupont)")} />
          <Radio value="last-first" label={t("Nom Prénom (Dupont Marie)")} />
        </RadioGroup>
        <p className="muted settings-hint">{t("Pour l'affichage et le tri de la liste des contacts.")}</p>
      </fieldset>
    </div>
  );
}
