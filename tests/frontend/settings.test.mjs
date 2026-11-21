// Réglages de « Paramètres › Courrier » : signatures et pièce jointe oubliée.
// Hors de Tauri, le stockage sûr passe par `localStorage` (setup.mjs).
import { test } from "node:test";
import assert from "node:assert/strict";

globalThis.window ??= new EventTarget();
const { attachmentMention, loadSettings, ownText, saveSettings, signatureBlock, signatureContent } = await import("../../src/settings.ts");

test("réglages par défaut, puis enregistrés et signalés aux autres vues", () => {
  localStorage.removeItem("vortex.mail");
  assert.equal(loadSettings().signatureOnReplies, true);
  let changed = 0;
  window.addEventListener("vortex-settings", () => (changed += 1));
  saveSettings({ ...loadSettings(), signatureOnReplies: false });
  assert.equal(loadSettings().signatureOnReplies, false);
  assert.equal(changed, 1, "les autres vues sont prévenues");
  localStorage.setItem("vortex.mail", "{pas du json");
  assert.equal(loadSettings().signatureOnReplies, true, "réglages illisibles : valeurs par défaut");
});

test("signature échappée, lignes vides gardées, absente des réponses si demandé", () => {
  localStorage.removeItem("vortex.mail");
  saveSettings({ ...loadSettings(), signatures: { perso: "Fabrice <F>\n\nVortex & co\n\n" } });
  assert.equal(signatureContent("perso"), "Fabrice &lt;F&gt;<br><br><br>Vortex &amp; co");
  assert.equal(signatureContent("pro"), "", "compte sans signature");
  assert.match(signatureBlock("perso", false), /^<p><br><\/p><div data-signature="1">Fabrice/);
  assert.equal(signatureBlock("pro", false), "");
  assert.notEqual(signatureBlock("perso", true), "", "réponses signées par défaut");
  saveSettings({ ...loadSettings(), signatureOnReplies: false });
  assert.equal(signatureBlock("perso", true), "");
});

test("pièce jointe annoncée : mots entiers, expressions longues d'abord", () => {
  const words = "joint, pièce jointe, PJ";
  assert.equal(attachmentMention("Voici la pièce jointe demandée", words), "pièce jointe");
  assert.equal(attachmentMention("Ci-joint le devis", words), "joint");
  assert.equal(attachmentMention("Un pjotr, disjoint", words), null, "pas au milieu d'un mot");
  assert.equal(attachmentMention("voir pj", words), "pj", "sans tenir compte de la casse");
  assert.equal(attachmentMention("rien", " , "), null, "liste vide");
});

test("texte écrit en mode texte brut : sans la citation", () => {
  const editor = (innerText) => ({ innerText });
  assert.equal(ownText(editor("Bonjour\n> cité\nMerci\nLe 12/11, Alice a écrit :\nancien"), true), "Bonjour\nMerci");
  assert.equal(ownText(editor("Voici\n-----\nDe : Bob"), true), "Voici");
  assert.equal(ownText(editor("Hi\nFrom: Bob"), true), "Hi");
});
