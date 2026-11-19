// Sélection de messages et arborescence des dossiers (audit P3, B3, P7) : `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { pickById, whenAccepted, withFlag } from "../../src/selection.ts";
import { folderTree } from "../../src/folder-tree.ts";

const message = (id, seen = false) => ({
  id,
  folderId: "a:INBOX",
  accountId: "a",
  fromName: "",
  fromEmail: "x@example.com",
  subject: `n° ${id}`,
  preview: "",
  date: id,
  flags: { seen, flagged: false, answered: false },
  hasAttachments: false,
});

test("P3 : tout sélectionner sur une grande boîte, en temps linéaire", () => {
  const list = Array.from({ length: 50_000 }, (_, i) => message(i));
  const ids = list.map((m) => m.id);
  const started = performance.now();
  const picked = pickById(list, new Set(ids));
  const marked = withFlag(list, ids, "seen", true);
  const elapsed = performance.now() - started;
  assert.equal(picked.length, 50_000);
  assert.ok(marked.every((m) => m.flags.seen));
  // En n², 50 000 messages font 2,5 milliards de comparaisons : plusieurs secondes.
  assert.ok(elapsed < 1_000, `${elapsed} ms`);
});

test("P3 : seuls les messages changés sont de nouveaux objets", () => {
  const list = [message(1), message(2, true), message(3)];
  const next = withFlag(list, [1, 2], "seen", true);
  assert.notEqual(next[0], list[0], "changé");
  assert.equal(next[1], list[1], "déjà lu : même objet");
  assert.equal(next[2], list[2], "pas sélectionné : même objet");
  assert.deepEqual(pickById(list, new Set([3, 1])).map((m) => m.id), [1, 3], "ordre de la liste");
});

test("B3 : l'affichage ne change pas si le serveur refuse", async () => {
  const seen = [];
  let pending = 0;
  await whenAccepted(Promise.reject(new Error("refusé")), () => seen.push("appliqué"), (e) => seen.push(`erreur : ${e.message}`), () => pending++);
  assert.deepEqual(seen, ["erreur : refusé"]);
  await whenAccepted(Promise.resolve(), () => seen.push("appliqué"), () => seen.push("erreur"), () => pending++);
  assert.deepEqual(seen, ["erreur : refusé", "appliqué"]);
  assert.equal(pending, 2, "compteur des actions en attente relu dans les deux cas");
});

test("P7 : arborescence construite une fois, dossiers abonnés et leurs parents", () => {
  const folder = (id, parentId, subscribed = false) => ({ id, parentId, subscribed });
  const tree = folderTree([
    folder("a:Clients", null),
    folder("a:Clients/2026", "a:Clients"),
    folder("a:Clients/2026/Devis", "a:Clients/2026", true),
    folder("a:Archives", null),
  ]);
  assert.deepEqual([...tree.holdsSubscribed].sort(), ["a:Clients", "a:Clients/2026"]);
  assert.deepEqual(tree.children.get("a:Clients").map((f) => f.id), ["a:Clients/2026"]);
  assert.equal(tree.children.get("a:Archives"), undefined);
  assert.equal(tree.byId.get("a:Clients/2026").parentId, "a:Clients");
});
