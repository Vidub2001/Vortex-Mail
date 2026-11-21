// « Déplacer vers… » du clic droit (lot Q) : dossiers proposés et recherche.
import { test } from "node:test";
import assert from "node:assert/strict";
import { folded, moveTargets, recentMoveIds, recentTargets, rememberMoveTarget } from "../../src/move-targets.ts";

const accounts = [
  { id: "perso", name: "Perso", email: "moi@x.fr", color: "#123" },
  { id: "pro", name: "Pro", email: "moi@y.fr", color: "#456" },
  { id: "vide", name: "Vide", email: "v@z.fr", color: "#789" },
];
const folder = (accountId, id, name, parentId = null, depth = 0) => ({
  id: `${accountId}:${id}`,
  accountId,
  name,
  path: id,
  parentId: parentId && `${accountId}:${parentId}`,
  depth,
  role: "custom",
  unread: 0,
  total: 0,
  subscribed: true,
});
const folders = [
  folder("perso", "INBOX", "Boîte de réception"),
  folder("perso", "Factures", "Factures"),
  folder("perso", "Factures/2026", "2026", "Factures", 1),
  folder("perso", "Recus", "Reçus"),
  folder("pro", "INBOX", "Boîte de réception"),
  folder("pro", "Clients", "Clients"),
  folder("pro", "Clients/Factures", "Factures clients", "Clients", 1),
];
const names = (groups) => groups.map((g) => [g.account.id, g.targets.map((x) => x.folder.name)]);

test("Q : sans recherche, tous les dossiers par compte, sauf celui des messages", () => {
  const groups = moveTargets(accounts, folders, "  ", new Set(["perso:INBOX"]));
  assert.deepEqual(names(groups), [
    ["perso", ["Factures", "2026", "Reçus"]],
    ["pro", ["Boîte de réception", "Clients", "Factures clients"]],
  ]);
  assert.ok(groups.every((g) => g.targets.every((x) => x.parents === "")), "pas de chemin sans recherche");
});

test("Q : la recherche garde les noms qui correspondent, sans casse ni accents", () => {
  assert.deepEqual(names(moveTargets(accounts, folders, "FACT")), [
    ["perso", ["Factures"]],
    ["pro", ["Factures clients"]],
  ]);
  const recus = moveTargets(accounts, folders, "recus");
  assert.deepEqual(names(recus), [["perso", ["Reçus"]]]);
  const year = moveTargets(accounts, folders, "2026")[0].targets[0];
  assert.equal(year.parents, "Factures", "résultat situé par ses parents");
  assert.deepEqual(moveTargets(accounts, folders, "introuvable"), []);
  assert.equal(folded("Éléments Envoyés"), "elements envoyes");
});

test("Q : dossiers récents, du plus récent au plus ancien, cinq au plus", () => {
  localStorage.removeItem("vortex.move.recent");
  assert.deepEqual(recentMoveIds(), []);
  for (const id of ["perso:Factures", "pro:Clients", "perso:Recus", "pro:Clients"]) rememberMoveTarget(id);
  assert.deepEqual(recentMoveIds(), ["pro:Clients", "perso:Recus", "perso:Factures"], "déjà présent : remonté en tête");
  for (const id of ["a", "b", "c", "d"]) rememberMoveTarget(id);
  assert.equal(recentMoveIds().length, 5);
  localStorage.setItem("vortex.move.recent", "{pas une liste");
  assert.deepEqual(recentMoveIds(), [], "illisible : aucun");
});

test("Q : récents disparus ou d'origine écartés, chacun situé", () => {
  const ids = ["pro:Clients/Factures", "disparu:X", "perso:INBOX", "perso:Factures/2026"];
  const recents = recentTargets(accounts, folders, ids, new Set(["perso:INBOX"]));
  assert.deepEqual(
    recents.map((r) => [r.folder.name, r.parents, r.account.id]),
    [
      ["Factures clients", "Clients", "pro"],
      ["2026", "Factures", "perso"],
    ],
  );
});
