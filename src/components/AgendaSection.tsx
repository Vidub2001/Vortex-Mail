// Paramètres › Agenda : semaine, échelle de la grille, numéros de semaine, rappels.
import { Checkbox, Field, Radio, RadioGroup, Select } from "@fluentui/react-components";
import { weekNumber } from "../calendar";
import { REMINDER_CHOICES, saveGeneral, useGeneral, type FirstWeek, type TimeScale, type WeekDay } from "../general";
import { t } from "../i18n";

const DAYS: { value: WeekDay; label: string }[] = [
  { value: 1, label: t("Lundi") },
  { value: 0, label: t("Dimanche") },
  { value: 6, label: t("Samedi") },
];

const SCALES: TimeScale[] = [10, 15, 30, 60];

const FIRST_WEEKS: { value: FirstWeek; label: string }[] = [
  { value: "iso", label: t("Première semaine de 4 jours (ISO 8601, usage européen)") },
  { value: "jan1", label: t("Semaine du 1er janvier") },
  { value: "full", label: t("Première semaine complète") },
];

export function AgendaSection() {
  const g = useGeneral();
  const today = weekNumber(Date.now(), g.weekStart, g.firstWeek);
  return (
    <div className="settings-pane">
      <h3 className="settings-pane-title">{t("Agenda")}</h3>

      <fieldset className="account-fieldset">
        <legend>{t("Semaine")}</legend>
        <div className="settings-row">
          <Field label={t("Premier jour de la semaine")}>
            <Select value={String(g.weekStart)} onChange={(_, d) => saveGeneral({ weekStart: Number(d.value) as WeekDay })}>
              {DAYS.map((d) => (
                <option key={d.value} value={d.value}>
                  {d.label}
                </option>
              ))}
            </Select>
          </Field>
          <Field label={t("Échelle de la grille")}>
            <Select value={String(g.timeScale)} onChange={(_, d) => saveGeneral({ timeScale: Number(d.value) as TimeScale })}>
              {SCALES.map((s) => (
                <option key={s} value={s}>
                  {s === 60 ? t("1 heure") : t("{n} minutes", { n: s })}
                </option>
              ))}
            </Select>
          </Field>
        </div>
        <p className="muted settings-hint">
          {t("L'échelle règle les cases de la vue semaine : un double-clic crée le rendez-vous à la case visée.")}
        </p>
      </fieldset>

      <fieldset className="account-fieldset">
        <legend>{t("Numéros de semaine")}</legend>
        <Checkbox
          checked={g.weekNumbers}
          onChange={(_, d) => saveGeneral({ weekNumbers: d.checked === true })}
          label={t("Afficher les numéros de semaine")}
        />
        <Field label={t("Première semaine de l'année")}>
          <RadioGroup value={g.firstWeek} onChange={(_, d) => saveGeneral({ firstWeek: d.value as FirstWeek })}>
            {FIRST_WEEKS.map((f) => (
              <Radio key={f.value} value={f.value} label={f.label} />
            ))}
          </RadioGroup>
        </Field>
        <p className="muted settings-hint">{t("Nous sommes en semaine {n}.", { n: today })}</p>
      </fieldset>

      <fieldset className="account-fieldset">
        <legend>{t("Rappels")}</legend>
        <Field label={t("Rappel des nouveaux rendez-vous")}>
          <Select value={String(g.defaultReminder)} onChange={(_, d) => saveGeneral({ defaultReminder: Number(d.value) })}>
            {REMINDER_CHOICES.map((c) => (
              <option key={c.value} value={c.value}>
                {c.label}
              </option>
            ))}
          </Select>
        </Field>
        <p className="muted settings-hint">
          {t(
            "Chaque rendez-vous garde son propre rappel, modifiable dans sa fiche. Les rappels s'affichent en notification tant que Vortex Mail est ouvert ; un rappel manqué est montré au démarrage suivant.",
          )}
        </p>
      </fieldset>
    </div>
  );
}
