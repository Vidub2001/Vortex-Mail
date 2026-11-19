// Listes des dossiers déjà reçues (audit P1, P8) : `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { applySync, knownRevisions } from "../../src/lists.ts";

const message = (id) => ({ id, folderId: "a:INBOX", accountId: "a", fromName: "", fromEmail: "", subject: "", preview: "", date: id, flags: { seen: false, flagged: false, answered: false }, hasAttachments: false });

test("P1 : une liste inchangée n'est ni renvoyée ni redessinée", () => {
  const known = new Map();
  assert.deepEqual(knownRevisions(known, ["a:INBOX", "b:INBOX"]), [null, null], "premier passage : tout est demandé");

  const first = applySync(known, [
    { folderId: "a:INBOX", messages: [message(1)], revision: 10, error: null },
    { folderId: "b:INBOX", messages: [message(2)], revision: 20, error: null },
  ]);
  assert.equal(first.changed, true);
  assert.deepEqual(knownRevisions(known, ["a:INBOX", "b:INBOX"]), [10, 20]);

  const idle = applySync(known, [
    { folderId: "a:INBOX", messages: null, revision: 10, error: null },
    { folderId: "b:INBOX", messages: null, revision: 20, error: null },
  ]);
  assert.equal(idle.changed, false, "rien à redessiner");
  assert.deepEqual(idle.lists.map((l) => l.map((m) => m.id)), [[1], [2]], "listes gardées");

  const one = applySync(known, [
    { folderId: "a:INBOX", messages: [message(3), message(1)], revision: 11, error: null },
    { folderId: "b:INBOX", messages: null, revision: 20, error: null },
  ]);
  assert.equal(one.changed, true);
  assert.deepEqual(one.lists.map((l) => l.map((m) => m.id)), [[3, 1], [2]], "la liste inchangée reprise de la mémoire");
  assert.deepEqual(knownRevisions(known, ["a:INBOX"]), [11]);
});
