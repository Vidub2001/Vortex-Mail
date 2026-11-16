// Invitations selon leur méthode (relecture du 12 novembre, n° 7) : `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { planFor } from "../../src/invitation.ts";

const event = (fields) => ({
  id: "",
  source: "",
  calendar: "",
  uid: "reunion@exemple.fr",
  summary: "Réunion",
  location: "Salle A",
  description: "",
  start: { value: "20261120T090000", tzid: "Europe/Paris" },
  end: { value: "20261120T100000", tzid: "Europe/Paris" },
  duration: "",
  allDay: false,
  rrule: "",
  exdates: [],
  recurrenceId: "",
  organizer: "o@exemple.fr",
  sequence: 0,
  status: "",
  ...fields,
});
const invitation = (method, fields = {}) => ({ events: [event(fields)], raw: "", method });
const stored = event({ id: "cal|1", calendar: "cal", sequence: 1 });

test("n° 7 : demande, mise à jour, version plus ancienne", () => {
  assert.equal(planFor(invitation("REQUEST"), []).kind, "add");
  assert.equal(planFor(invitation(""), []).kind, "add", "simple publication");
  assert.equal(planFor(invitation("REQUEST", { sequence: 2, location: "Salle B" }), [stored]).kind, "update");
  assert.equal(planFor(invitation("REQUEST", { sequence: 1 }), [stored]).kind, "current");
  assert.equal(planFor(invitation("REQUEST", { sequence: 1, location: "Salle C" }), [stored]).kind, "update", "même version, contenu plus récent");
  assert.equal(planFor(invitation("REQUEST", { sequence: 0 }), [stored]).kind, "outdated");
  assert.equal(planFor(invitation("REQUEST", { sequence: 2 }), [stored]).existing, stored);
});

test("n° 7 : annulation et réponse d'invité", () => {
  assert.equal(planFor(invitation("CANCEL", { sequence: 2 }), [stored]).kind, "cancel");
  assert.equal(planFor(invitation("CANCEL"), []).kind, "gone");
  assert.equal(planFor(invitation("REQUEST", { status: "CANCELLED" }), [stored]).kind, "cancel");
  for (const method of ["REPLY", "COUNTER", "reply"]) assert.equal(planFor(invitation(method), [stored]).kind, "reply", method);
});
