// Vues du corps calculées à la demande (audit P7).
import { test } from "node:test";
import assert from "node:assert/strict";
import { lacksViews, needsViews, withViews } from "../../src/body-views.ts";

const message = (id, bodyHtml, extra = {}) => ({ id, bodyHtml, bodySimple: "", bodyText: "", flags: { seen: false }, ...extra });

function counter() {
  const calls = [];
  const compute = async (html) => {
    calls.push(html);
    return { simple: `[${html}]`, text: html.replace(/<[^>]*>/g, "") };
  };
  return { calls, compute };
}

test("P7 : vues demandées seulement hors du mode Original", () => {
  const m = message(1, "<p>Bonjour</p>");
  assert.equal(needsViews(m, "original"), false, "Original : le corps d'origine suffit");
  assert.equal(needsViews(m, "simple"), true);
  assert.equal(needsViews(m, "text"), true);
  assert.equal(lacksViews(message(2, "")), false, "corps vide : rien à calculer");
  assert.equal(lacksViews(message(3, "<p>x</p>", { bodySimple: "<p>x</p>", bodyText: "x" })), false, "vues déjà là");
});

test("P7 : une seule demande par corps, gardée quand un indicateur change", async () => {
  const { calls, compute } = counter();
  const m = message(10, "<p>Salut</p>");
  const [a, b] = await Promise.all([withViews(m, compute), withViews(m, compute)]);
  assert.equal(calls.length, 1, "affichage et réponse en même temps : une demande");
  assert.equal(a.bodySimple, "[<p>Salut</p>]");
  assert.equal(b.bodyText, "Salut");
  const seen = await withViews({ ...m, flags: { seen: true } }, compute);
  assert.equal(calls.length, 1, "même corps, nouvel objet : pas de nouvelle demande");
  assert.equal(seen.flags.seen, true, "le message rendu est celui demandé");
  await withViews({ ...m, bodyHtml: "<p>Salut</p><img src=data:x>" }, compute);
  assert.equal(calls.length, 2, "images affichées, autre corps : nouvelle demande");
});

test("P7 : un échec n'est pas gardé", async () => {
  let fail = true;
  const compute = async (html) => {
    if (fail) throw new Error("backend indisponible");
    return { simple: html, text: html };
  };
  const m = message(20, "<p>Encore</p>");
  await assert.rejects(withViews(m, compute));
  fail = false;
  assert.equal((await withViews(m, compute)).bodySimple, "<p>Encore</p>", "nouvel essai après l'échec");
});
