// Rendez-vous où l'on est invité (lot H1b).
import { test } from "node:test";
import assert from "node:assert/strict";
import { allDeclined, cleanDescription, invitedAccounts, isInvited, meetingLink, myAnswer, newInvitations } from "../../src/invites.ts";

const event = (fields) => ({
  id: "cal|1",
  source: "mailo",
  calendar: "https://dav/cal/",
  uid: "u1",
  summary: "Point",
  location: "",
  description: "",
  start: { value: "20991120T100000Z", tzid: "" },
  end: { value: "20991120T110000Z", tzid: "" },
  duration: "",
  allDay: false,
  rrule: "",
  exdates: [],
  recurrenceId: "",
  organizer: "Marie",
  organizerEmail: "marie@exemple.fr",
  attendees: [{ email: "Moi@Mailo.com", name: "", partstat: "NEEDS-ACTION" }],
  ...fields,
});
const mine = ["moi@mailo.com"];

test("H1b : invité ou organisateur, et réponse donnée", () => {
  assert.equal(isInvited(event({}), mine), true, "invité par Marie");
  assert.equal(isInvited(event({ organizerEmail: "MOI@mailo.com" }), mine), false, "organisateur");
  assert.equal(isInvited(event({ organizerEmail: "" }), mine), false, "sans organisateur : rendez-vous personnel");
  assert.equal(isInvited(event({ attendees: [] }), mine), false, "pas dans la liste");
  assert.equal(myAnswer(event({}), mine), null, "en attente");
  assert.equal(myAnswer(event({ attendees: [{ email: "moi@mailo.com", name: "", partstat: "TENTATIVE" }] }), mine), "tentative");
});

test("H1b : invitation adressée à plusieurs comptes", () => {
  const accounts = [
    { id: "gmail", name: "Gmail", email: "moi@gmail.com", color: "" },
    { id: "mailo", name: "Mailo", email: "moi@mailo.com", color: "" },
    { id: "pro", name: "Pro", email: "moi@pro.fr", color: "" },
  ];
  const both = event({
    attendees: [
      { email: "Moi@Mailo.com", name: "", partstat: "DECLINED" },
      { email: "moi@gmail.com", name: "", partstat: "NEEDS-ACTION" },
    ],
  });
  assert.deepEqual(
    invitedAccounts(both, accounts).map((a) => a.id),
    ["gmail", "mailo"],
    "comptes invités, dans l'ordre des comptes",
  );
  const emails = accounts.map((a) => a.email);
  assert.equal(allDeclined(both, emails), false, "un compte n'a pas refusé : toujours affiché");
  const refused = event({ attendees: both.attendees.map((a) => ({ ...a, partstat: "DECLINED" })) });
  assert.equal(allDeclined(refused, emails), true, "tous refusés : masqué");
  assert.equal(allDeclined(event({ attendees: [] }), emails), false, "rendez-vous sans invités");
});

test("H1b : lien de visioconférence et description sans le bloc de Google", () => {
  const description =
    "Ordre du jour : budget\n\n-::~:~::~:~:~:~:~:~::~:~::-\nParticiper via Google Meet : https://meet.google.com/tsc-ycmp-ncv\n\nPour en savoir plus sur Meet : https://support.google.com/a/users/answer/9282720\n-::~:~::~:~:~:~:~:~::~:~::-\n";
  const e = event({ description });
  assert.deepEqual(meetingLink(e), { url: "https://meet.google.com/tsc-ycmp-ncv", service: "Google Meet" });
  assert.equal(cleanDescription(description), "Ordre du jour : budget", "bloc de Google retiré");
  assert.equal(cleanDescription("Texte\n----\nsuite"), "Texte\n----\nsuite", "texte ordinaire inchangé");
  assert.deepEqual(meetingLink(event({ location: "https://us02web.zoom.us/j/123456789?pwd=abc" })), {
    url: "https://us02web.zoom.us/j/123456789?pwd=abc",
    service: "Zoom",
  });
  assert.equal(meetingLink(event({ location: "Salle A" })), null, "pas de visio");
});

test("H1b : nouvelles invitations signalées une fois, rien au premier passage", () => {
  localStorage.removeItem("vortex.invitations.notified");
  const first = event({ uid: "a" });
  assert.deepEqual(newInvitations([first], mine), [], "premier passage : déjà là, pas signalée");
  const second = event({ uid: "b" });
  assert.deepEqual(newInvitations([first, second], mine).map((e) => e.uid), ["b"], "nouvelle invitation");
  assert.deepEqual(newInvitations([first, second], mine), [], "pas deux fois");
  const changed = event({ uid: "b", sequence: 1 });
  assert.deepEqual(newInvitations([first, changed], mine).map((e) => e.uid), ["b"], "modifiée par l'organisateur : signalée de nouveau");
  const past = event({ uid: "c", start: { value: "20001120T100000Z", tzid: "" } });
  assert.deepEqual(newInvitations([first, changed, past], mine), [], "rendez-vous passé : pas signalé");
});
