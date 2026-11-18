// Rédaction : adresses saisies, préfixes, liens mailto (relecture complète
// n° 12, 23 et 24) : `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mailboxEntry, mailtoDraft, parseAddresses, replyDraft } from "../../src/compose.ts";

test("n° 12 : nom avec virgule, entre guillemets", () => {
  assert.equal(mailboxEntry("Dupont, Marie", "m@x.fr"), '"Dupont, Marie" <m@x.fr>');
  assert.equal(mailboxEntry("Paul", "p@x.fr"), "Paul <p@x.fr>");
  assert.equal(mailboxEntry('Le "chef"', "c@x.fr"), '"Le \\"chef\\"" <c@x.fr>');
  assert.deepEqual(parseAddresses('"Dupont, Marie" <m@x.fr>; b@y.fr, Paul <p@x.fr>; '), ["m@x.fr", "b@y.fr", "p@x.fr"]);
  assert.deepEqual(parseAddresses('"Le \\"chef\\", RH" <c@x.fr>'), ["c@x.fr"]);
});

const message = {
  id: 7,
  accountId: "a",
  subject: "Devis",
  fromName: "Jean",
  fromEmail: "jean@x.fr",
  date: 0,
  to: "moi@x.fr",
  toList: ["moi@x.fr"],
  ccList: [],
  replyTo: [],
  messageId: "<p@x>",
  references: "<root@x>",
  bodySimple: "",
  attachments: [],
};

test("n° 24 : préfixes sans espace avant les deux-points", () => {
  assert.equal(replyDraft("reply", message, []).subject, "RE: Devis");
  assert.match(replyDraft("forward", message, []).subject, /^(TR|FW|RV): Devis$/, "langue de l'interface");
  assert.equal(replyDraft("reply", { ...message, subject: "Re : Devis" }, []).subject, "Re : Devis", "pas de second préfixe");
  // Transfert : le message d'origine sera marqué « transféré ».
  assert.equal(replyDraft("forward", message, []).answeredId, 7);
  assert.equal(replyDraft("forward", message, []).inReplyTo, null);
});

test("n° 23 : rédaction ouverte par mailto, texte échappé", () => {
  const init = mailtoDraft("a", { to: "a@x.fr", cc: "c@z.fr", subject: "Hi", body: "1\n<b>2</b>" });
  assert.equal(init.cc, "c@z.fr");
  assert.ok(init.html.startsWith("<p>1<br>&lt;b&gt;2&lt;/b&gt;</p>"), init.html);
});

test("S4 : un message à chiffrer ne part jamais en clair sans le demander", async () => {
  const { encryptionCheck, secureFlags } = await import("../../src/compose.ts");
  assert.equal(encryptionCheck(false, false, true), "ok", "rien à chiffrer");
  assert.equal(encryptionCheck(true, true, true), "ok", "certificat prêt");
  assert.equal(encryptionCheck(true, false, false), "waiting", "certificats pas encore lus");
  assert.equal(encryptionCheck(true, false, true), "unavailable", "certificat absent, illisible ou autre compte");
  assert.deepEqual(secureFlags(true, true, false), { sign: false, encrypt: true }, "« Chiffrer » gardé sans certificat");
  assert.deepEqual(secureFlags(true, false, true), { sign: true, encrypt: false });
});
