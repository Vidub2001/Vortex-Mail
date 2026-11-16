// Tests des règles de classement (lot B1) : `npm test`, lancé par `make audit`.
// Hors de Tauri, `api` passe par la maquette `src/mock.ts` (deux comptes, perso et pro).
import { test } from "node:test";
import assert from "node:assert/strict";

// `saveRules` prévient l'interface par un événement de la fenêtre : une cible
// d'événements suffit (sans `__TAURI_INTERNALS__`, donc maquette).
globalThis.window ??= new EventTarget();
const { api } = await import("../../src/api.ts");
const { doneKey, messageKey, newRule, runInboxRules, saveRules } = await import("../../src/rules.ts");

const DONE_KEY = "vortex.rules.done.v3";
const PERSO = "perso:INBOX";
const PRO = "pro:INBOX";

const folders = async () => (await api.listFolders()).folders;
const inbox = (id) => api.listMessages(id);
const setDone = (done) => localStorage.setItem(DONE_KEY, JSON.stringify(done));
const done = () => JSON.parse(localStorage.getItem(DONE_KEY) ?? "{}");

function moveRule(account, fromEmail, target) {
  const r = newRule(account);
  r.name = `${account} vers ${target}`;
  r.conditions = [{ field: "from", op: "is", value: fromEmail }];
  r.actions = [{ type: "move", folderId: target }];
  return r;
}

test("B1 : un mail arrivé pendant la fermeture passe par les règles au premier tour", async () => {
  localStorage.removeItem("vortex.rules.moved");
  const list = await inbox(PERSO);
  assert.ok(list.length >= 2, "la maquette a des messages dans la boîte perso");
  const arrived = list[0];
  // La boîte avait été vue avec tous ses messages sauf celui-ci : il est arrivé depuis.
  setDone({ [PERSO]: list.slice(1).map(doneKey), [PRO]: (await inbox(PRO)).map(doneKey) });
  saveRules([moveRule("perso", arrived.fromEmail, "perso:Factures")]);

  const report = await runInboxRules(await folders());
  assert.equal(report?.matched, 1, "seul le message arrivé est examiné");
  assert.ok(!(await inbox(PERSO)).some((m) => messageKey(m) === messageKey(arrived)), "il a quitté la boîte");
  assert.ok((await inbox("perso:Factures")).some((m) => messageKey(m) === messageKey(arrived)), "il est classé");
});

test("B1 : deux règles qui se renvoient un message ne le font pas circuler", async () => {
  localStorage.removeItem("vortex.rules.moved");
  const sender = (await inbox(PERSO))[0];
  const key = messageKey(sender);
  // Perso → Pro et Pro → Perso pour le même expéditeur : une boucle.
  saveRules([moveRule("perso", sender.fromEmail, PRO), moveRule("pro", sender.fromEmail, PERSO)]);
  // Le message est nouveau dans la boîte perso ; la boîte pro est déjà connue.
  setDone({ [PERSO]: [], [PRO]: (await inbox(PRO)).map(doneKey) });

  await runInboxRules(await folders());
  assert.ok((await inbox(PRO)).some((m) => messageKey(m) === key), "premier tour : déplacé vers pro");

  for (let tour = 2; tour <= 4; tour++) {
    const report = await runInboxRules(await folders());
    assert.equal(report?.moved ?? 0, 0, `tour ${tour} : plus aucun déplacement`);
    assert.ok((await inbox(PRO)).some((m) => messageKey(m) === key), `tour ${tour} : le message reste dans pro`);
  }
  assert.ok(done()[PRO].includes(key), "noté comme examiné dans la boîte pro");
});

test("B4 : seuls les nouveaux non-lus restés dans la boîte sont annoncés", async () => {
  localStorage.removeItem("vortex.rules.moved");
  const list = await inbox(PRO);
  const unread = list.filter((m) => !m.flags.seen);
  assert.ok(unread.length >= 2, "la maquette a au moins deux non-lus dans la boîte pro");
  const [spam, kept] = unread;
  // Les deux sont arrivés depuis le dernier passage ; une règle met le premier à la corbeille.
  const rule = newRule("pro");
  rule.conditions = [{ field: "subject", op: "is", value: spam.subject }];
  rule.actions = [{ type: "delete" }];
  saveRules([rule]);
  const others = list.filter((m) => m.id !== spam.id && m.id !== kept.id).map(doneKey);
  setDone({ [PERSO]: (await inbox(PERSO)).map(doneKey), [PRO]: others });

  const report = await runInboxRules(await folders());
  const announced = report?.arrivals.map(doneKey) ?? [];
  assert.ok(!announced.includes(messageKey(spam)), "le message mis à la corbeille n'est pas annoncé");
  assert.ok(announced.includes(messageKey(kept)), "l'autre nouveau message l'est");
  assert.ok(report?.arrivals.every((m) => !m.flags.seen), "rien de déjà lu");
});

test("n° 50 : deux alertes identiques à la même seconde restent deux messages", () => {
  const alert = { id: 1, folderId: "a:INBOX", accountId: "a", fromName: "", fromEmail: "sup@x.fr", subject: "Disque plein", preview: "", date: 100, flags: { seen: false, flagged: false, answered: false }, hasAttachments: false };
  assert.equal(messageKey({ ...alert, uid: 7 }), messageKey({ ...alert, uid: 8 }), "même empreinte de déplacement");
  assert.notEqual(doneKey({ ...alert, uid: 7 }), doneKey({ ...alert, uid: 8 }), "examinés séparément");
  assert.equal(doneKey(alert), messageKey(alert), "sans UID connu");
});

test("F3 : sur une grande boîte, un ancien message redevenu non lu n'est pas repris par les règles", async () => {
  localStorage.removeItem("vortex.rules.moved");
  const list = await inbox(PRO);
  const old = list[list.length - 1];
  // Boîte déjà vue, et liste « examinés » de plus de 5 000 entrées (limite de l'ancien code).
  const filler = Array.from({ length: 6000 }, (_, i) => `ancien-${i}`);
  setDone({ [PERSO]: (await inbox(PERSO)).map(doneKey), [PRO]: [...filler, ...list.map(doneKey)] });
  const rule = newRule("pro");
  rule.conditions = [{ field: "from", op: "contains", value: "@" }];
  rule.actions = [{ type: "markRead" }];
  saveRules([rule]);
  await api.setSeen([old.id], false);

  for (let tour = 1; tour <= 2; tour++) {
    const report = await runInboxRules(await folders());
    assert.equal(report?.matched ?? 0, 0, `tour ${tour} : aucun message déjà vu n'est réexaminé`);
  }
  const after = (await inbox(PRO)).find((m) => m.id === old.id);
  assert.equal(after?.flags.seen, false, "le message remis en non lu le reste");
  assert.ok(done()[PRO].includes(doneKey(old)), "toujours noté comme examiné");
  await api.setSeen([old.id], true);
});

test("F9 : règle « destinataires ne contiennent pas » sur un message illisible, retentée plus tard", async () => {
  localStorage.removeItem("vortex.rules.moved");
  const list = await inbox(PERSO);
  const fresh = list[0];
  setDone({ [PERSO]: list.slice(1).map(doneKey), [PRO]: (await inbox(PRO)).map(doneKey) });
  const rule = newRule("perso");
  rule.conditions = [{ field: "recipients", op: "notContains", value: "moi@exemple.fr" }];
  rule.actions = [{ type: "delete" }];
  saveRules([rule]);
  // Téléchargement du message complet impossible (délai dépassé).
  const getMessage = api.getMessage;
  api.getMessage = () => Promise.reject("délai dépassé");
  try {
    const report = await runInboxRules(await folders());
    assert.equal(report?.matched ?? 0, 0, "la règle n'est pas appliquée");
  } finally {
    api.getMessage = getMessage;
  }
  assert.ok((await inbox(PERSO)).some((m) => m.id === fresh.id), "le message reste dans la boîte");
  assert.ok(!done()[PERSO].includes(doneKey(fresh)), "il sera examiné au prochain passage");
  // Prochain passage, message lisible : la règle s'applique (il n'est pas adressé à moi@exemple.fr).
  const report = await runInboxRules(await folders());
  assert.equal(report?.matched, 1, "retenté et appliqué");
  saveRules([]);
});
