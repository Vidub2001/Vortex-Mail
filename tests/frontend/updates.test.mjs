// Nouvelle version publiée (lot K1) : rythme des vérifications et version écartée.
import { test } from "node:test";
import assert from "node:assert/strict";
import { checkDue, checkOnce, skipVersion, worthShowing } from "../../src/updates.ts";

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

test("K1 : un passage interroge GitHub une fois par jour, et note seulement les réponses", async () => {
  localStorage.removeItem("vortex.update.lastCheck");
  localStorage.removeItem("vortex.update.skipped");
  const now = Date.UTC(2026, 10, 19, 10);
  const release = { version: "0.3.6", url: "https://github.com/Vidub2001/Vortex-Mail/releases/tag/v0.3.6" };
  let calls = 0;
  const github = (answer) => () => {
    calls += 1;
    return answer instanceof Error ? Promise.reject(answer) : Promise.resolve(answer);
  };
  await assert.rejects(checkOnce(now, github(new Error("hors ligne"))), /hors ligne/);
  assert.deepEqual(await checkOnce(now + HOUR, github(release)), release, "échec non noté : nouvel essai à l'heure suivante");
  assert.equal(await checkOnce(now + 2 * HOUR, github(release)), null, "déjà vérifié aujourd'hui");
  assert.equal(calls, 2, "GitHub interrogé deux fois seulement");
  skipVersion("0.3.6");
  assert.equal(await checkOnce(now + 26 * HOUR, github(release)), null, "version écartée");
  assert.equal(await checkOnce(now + 51 * HOUR, github(null)), null, "rien de plus récent");
  assert.equal(calls, 4);
});
