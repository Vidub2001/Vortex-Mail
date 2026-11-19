// Nouvelle version publiée (lot K1) : rythme des vérifications et version écartée.
import { test } from "node:test";
import assert from "node:assert/strict";
import { checkDue, worthShowing } from "../../src/updates.ts";

const HOUR = 3600_000;

test("K1 : GitHub interrogé au plus une fois par jour", () => {
  const now = Date.UTC(2026, 10, 18, 15);
  assert.equal(checkDue(now, null), true, "jamais vérifié");
  assert.equal(checkDue(now, now - 23 * HOUR), false);
  assert.equal(checkDue(now, now - 24 * HOUR), true);
  assert.equal(checkDue(now, now + 2 * HOUR), true, "horloge reculée : on vérifie quand même");
});

test("K1 : version écartée jamais resignalée, la suivante si", () => {
  const found = { version: "0.3.5", url: "https://github.com/Vidub2001/Vortex-Mail/releases/tag/v0.3.5" };
  assert.equal(worthShowing(found, null), true);
  assert.equal(worthShowing(found, "0.3.5"), false);
  assert.equal(worthShowing({ ...found, version: "0.3.6" }, "0.3.5"), true);
  assert.equal(worthShowing(null, null), false);
});
