// Signature S/MIME d'un message reçu (lot D1) : `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fingerprintText, notExamined, signatureOf, signerLabel, tone, verdictText, whyText } from "../../src/signature.ts";
import { t } from "../../src/i18n.ts";

const json = (o) => JSON.stringify({ why: null, name: "", addresses: [], fingerprint: "", issuer: "", notBefore: 0, notAfter: 0, signedAt: null, whole: true, ...o });

test("D1 : résultat du backend lu, message non signé ou pas encore examiné", () => {
  assert.equal(signatureOf({ signature: "" }), null);
  assert.equal(signatureOf({}), null);
  assert.equal(signatureOf({ signature: "?" }), null);
  assert.ok(notExamined({ signature: "?" }));
  assert.equal(signatureOf({ signature: "{pas du json" }), null);
  assert.equal(signatureOf({ signature: json({ verdict: "autre" }) }), null);
  const s = signatureOf({ signature: json({ verdict: "verified", name: "Alice", addresses: ["alice@x.fr", 3] }) });
  assert.deepEqual(s.addresses, ["alice@x.fr"], "valeurs d'un autre type écartées");
  assert.equal(signerLabel(s), "Alice <alice@x.fr>");
  assert.equal(verdictText(s), t("Signé par {who} : signature vérifiée.", { who: "Alice <alice@x.fr>" }));
  assert.equal(tone(s), "good");
});

test("D1 : signature non vérifiée, modifiée, impossible à vérifier", () => {
  const not = signatureOf({ signature: json({ verdict: "notVerified", why: "unknownIssuer", addresses: ["bob@y.fr"] }) });
  assert.equal(signerLabel(not), "bob@y.fr");
  assert.equal(tone(not), "warning");
  assert.notEqual(whyText(not.why), "");
  assert.equal(tone(signatureOf({ signature: json({ verdict: "changed", why: "altered" }) })), "danger");
  const unchecked = signatureOf({ signature: json({ verdict: "unchecked", why: "weakAlgorithm" }) });
  assert.equal(signerLabel(unchecked), t("signataire inconnu"));
  assert.equal(whyText("raison inconnue"), "");
});

test("D1 : chaque raison du backend a son texte", () => {
  // Noms de `Why::as_str` (crates/smime/src/signature.rs), sauf ceux
  // qu'OpenPGP seul produit ou que Vortex Mail ne produit pas.
  const rust = readFileSync(new URL("../../crates/smime/src/signature.rs", import.meta.url), "utf8");
  const names = [...rust.matchAll(/Why::\w+ => "(\w+)",/g)].map((m) => m[1]);
  assert.ok(names.length >= 15, `${names.length} raisons lues`);
  for (const why of names.filter((n) => !["keyNotVerified", "noOriginal"].includes(n))) {
    assert.notEqual(whyText(why), "", why);
  }
});

test("D1 : empreinte lisible", () => {
  assert.equal(fingerprintText("ab:0c:FF"), "AB:0C:FF");
  assert.equal(fingerprintText("ab0cff"), "AB:0C:FF");
});
