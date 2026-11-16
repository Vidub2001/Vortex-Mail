// Invités d'un rendez-vous et choix du compte qui répond (lot H1b) : partagés
// par la carte d'invitation d'un message et par les fiches de l'agenda, dans un
// fichier à part pour que la lecture d'un message ne charge pas l'agenda (lot E6).
import { Avatar, Select } from "@fluentui/react-components";
import { t } from "../i18n";
import type { Attendee, InvitationAnswer } from "../types";

const lower = (s: string) => s.trim().toLowerCase();

/**
 * Invités et leur réponse. `mine` : adresses des comptes, avec leur réponse
 * connue, qui l'emporte sur celle écrite dans l'invitation (envoyée avant
 * qu'ils ne répondent) ; elles sont marquées « vous ».
 */
export function GuestList({ attendees, mine = [] }: { attendees: Attendee[]; mine?: { email: string; answer: InvitationAnswer | null }[] }) {
  const partstat: Record<InvitationAnswer, string> = { accepted: "ACCEPTED", tentative: "TENTATIVE", declined: "DECLINED" };
  const ours = new Map(mine.map((m) => [lower(m.email), m.answer]));
  return (
    <ul className="invitation-guests">
      {attendees.map((a) => {
        const own = ours.has(lower(a.email));
        const answer = ours.get(lower(a.email));
        const shown = answer ? { ...a, partstat: partstat[answer] } : a;
        const named = !!a.name && a.name !== a.email;
        return (
          <li key={a.email} className={`guest guest-${shown.partstat.toLowerCase()}${own ? " guest-me" : ""}`}>
            <Avatar name={a.name || a.email} size={24} color="colorful" />
            <span className="guest-who">
              <span className="guest-name">
                {named ? a.name : a.email}
                {own && <span className="guest-you"> · {t("vous")}</span>}
              </span>
              {named && <span className="guest-mail">{a.email}</span>}
            </span>
            <span className="guest-status">{statusLabel(shown)}</span>
          </li>
        );
      })}
    </ul>
  );
}

/**
 * Choix du compte qui répond, quand l'invitation vise plusieurs adresses
 * gérées par Vortex Mail (lot H1b) ; rien s'il n'y en a qu'une.
 */
export function AnswerAs({
  addresses,
  value,
  onChange,
  disabled,
}: {
  addresses: { account: string; email: string }[];
  value: string;
  onChange: (account: string) => void;
  disabled?: boolean;
}) {
  if (addresses.length < 2) return null;
  return (
    <label className="answer-as">
      <span>{t("Répondre en tant que")}</span>
      <Select size="small" value={value} disabled={disabled} onChange={(_, d) => onChange(d.value)}>
        {addresses.map((a) => (
          <option key={a.account} value={a.account}>
            {a.email}
          </option>
        ))}
      </Select>
    </label>
  );
}

/** Réponse d'un invité, en mots. */
function statusLabel(a: Attendee): string {
  switch (a.partstat) {
    case "ACCEPTED":
      return t("Oui");
    case "TENTATIVE":
      return t("Peut-être");
    case "DECLINED":
      return t("Non");
    case "DELEGATED":
      return t("Délégué");
    default:
      return t("En attente");
  }
}
