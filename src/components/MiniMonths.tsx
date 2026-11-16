// Colonne de gauche de l'agenda : trois petits mois (le mois en cours et les
// deux suivants), avec les numéros de semaine. Un clic sur un jour ou sur un
// numéro de semaine affiche cette semaine dans le planning.
import { Button } from "@fluentui/react-components";
import { ChevronLeft16Regular, ChevronRight16Regular } from "@fluentui/react-icons";
import { addDays, monthWeeks } from "../calendar";
import { locale, t } from "../i18n";

interface Props {
  /** Premier jour de chacun des mois affichés. */
  months: number[];
  /** Premier jour de la semaine (0 = dimanche). */
  firstDay: number;
  /** Noms courts des jours, dans l'ordre de la semaine. */
  weekdays: string[];
  weekNo: (day: number) => number;
  now: number;
  /** Période affichée dans le planning (vue semaine), mise en évidence ; null sinon. */
  shown: [number, number] | null;
  /** Jours qui ont au moins un rendez-vous (`Date.toDateString()`). */
  busy: Set<string>;
  onPrevious: () => void;
  onNext: () => void;
  onPick: (day: number) => void;
}

const monthYear = new Intl.DateTimeFormat(locale(), { month: "long", year: "numeric" });
const fullDate = new Intl.DateTimeFormat(locale(), { weekday: "long", day: "numeric", month: "long", year: "numeric" });

export function MiniMonths({ months, firstDay, weekdays, weekNo, now, shown, busy, onPrevious, onNext, onPick }: Props) {
  const today = new Date(now).toDateString();
  return (
    <nav className="cal-minis" aria-label={t("Mois à venir")}>
      {months.map((first, index) => {
        const month = new Date(first).getMonth();
        return (
          <div key={first} className="cal-minimonth">
            <div className="cal-minimonth-head">
              <span className="cal-minimonth-title">{monthYear.format(first)}</span>
              {index === 0 && (
                <span className="cal-minimonth-nav">
                  <Button size="small" appearance="subtle" icon={<ChevronLeft16Regular />} aria-label={t("Mois précédent")} title={t("Mois précédent")} onClick={onPrevious} />
                  <Button size="small" appearance="subtle" icon={<ChevronRight16Regular />} aria-label={t("Mois suivant")} title={t("Mois suivant")} onClick={onNext} />
                </span>
              )}
            </div>
            <div className="cal-mini-grid with-weeks cal-minimonth-grid">
              <span className="cal-mini-wd" aria-hidden />
              {weekdays.map((w) => (
                <span key={w} className="cal-mini-wd" aria-hidden>
                  {w.charAt(0).toUpperCase()}
                </span>
              ))}
              {monthWeeks(first, firstDay).flatMap((week) => [
                <button
                  key={`w${week}`}
                  type="button"
                  className="cal-mini-weekno"
                  onClick={() => onPick(week)}
                  title={t("Voir la semaine")}
                  aria-label={t("Semaine {n}", { n: weekNo(week) })}
                >
                  {weekNo(week)}
                </button>,
                ...Array.from({ length: 7 }, (_, k) => {
                  const day = addDays(week, k);
                  const inMonth = new Date(day).getMonth() === month;
                  const key = new Date(day).toDateString();
                  const classes = [
                    "cal-mini-day",
                    !inMonth && "outside",
                    key === today && "today",
                    busy.has(key) && "has-events",
                    shown && day >= shown[0] && day < shown[1] && "in-view",
                  ];
                  return (
                    <button
                      key={day}
                      type="button"
                      className={classes.filter(Boolean).join(" ")}
                      onClick={() => onPick(day)}
                      tabIndex={inMonth ? 0 : -1}
                      aria-hidden={!inMonth}
                      aria-label={fullDate.format(day)}
                    >
                      {new Date(day).getDate()}
                    </button>
                  );
                }),
              ])}
            </div>
          </div>
        );
      })}
    </nav>
  );
}
