// Opérations sur du texte en temps linéaire (lot L2, alertes SonarQube S8786).
import { test } from "node:test";
import assert from "node:assert/strict";
import { angleAddress, isPlausibleEmail, randomId, trimEndOf } from "../../src/text.ts";

test("L2 : fin de chaîne retirée comme les expressions remplacées", () => {
  assert.equal(trimEndOf("https://x.fr/a).", ").,;"), "https://x.fr/a");
  assert.equal(trimEndOf("ligne\n\n\n", "\n"), "ligne");
  assert.equal(trimEndOf(").,;", ").,;"), "");
  assert.equal(trimEndOf("", "\n"), "");
  assert.equal(trimEndOf("a)b", ")"), "a)b", "seulement à la fin");
});

test("L2 : longues suites sans fin attendue traitées en temps linéaire", () => {
  const hostile = ")".repeat(200_000) + "x";
  const started = performance.now();
  assert.equal(trimEndOf(hostile, ").,;"), hostile);
  assert.equal(angleAddress("<".repeat(200_000)), null);
  assert.equal(isPlausibleEmail("a@" + ".".repeat(200_000)), false);
  assert.ok(performance.now() - started < 500, "pas de temps quadratique");
});

test("L2 : forme d'une adresse saisie dans l'assistant", () => {
  for (const ok of ["marie@x.fr", "a.b+c@sous.domaine.example"]) assert.equal(isPlausibleEmail(ok), true, ok);
  for (const bad of ["", "marie", "@x.fr", "marie@", "marie@x", "marie@x.", "marie@.fr", "ma rie@x.fr", "a@b@c.fr"])
    assert.equal(isPlausibleEmail(bad), false, bad);
});

test("L2 : adresse entre chevrons, comme /<([^>]+)>\\s*$/", () => {
  assert.equal(angleAddress('"Dupont, Marie" <m@x.fr>'), "m@x.fr");
  assert.equal(angleAddress("Paul <p@y.fr>  "), "p@y.fr");
  assert.equal(angleAddress("p@y.fr"), null);
  assert.equal(angleAddress("a<b>c>"), null, "un > dans l'adresse");
  assert.equal(angleAddress("vide <>"), null);
});

test("L2 : identifiants tirés par le générateur du système", () => {
  assert.match(randomId(), /^[0-9a-f]{32}$/);
  assert.match(randomId(4), /^[0-9a-f]{8}$/);
  assert.notEqual(randomId(), randomId());
});
