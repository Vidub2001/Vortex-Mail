// Tests de src/links.ts (lot 4b, relecture S6) : `npm test`, lancé par `make audit`.
// Node 22.18 ou plus récent lit directement le TypeScript.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { LINK_HOST, linkAction, linkRequest, rewriteLink, shownHost } from "../../src/links.ts";

test("lien trompeur malgré des caractères invisibles ou un point final", () => {
  for (const text of ["www.banque\u200b.fr", "www.banque\u00ad.fr", "www.banque.fr\u200e", "www.banque.fr.", "ｗｗｗ.banque.fr"]) {
    assert.equal(shownHost(text), "banque.fr", JSON.stringify(text));
    assert.equal(linkAction("https://evil.example/", text).kind, "confirm", JSON.stringify(text));
  }
});

test("texte qui ressemble à une adresse", () => {
  assert.equal(shownHost("https://www.banque.fr/compte"), "banque.fr");
  assert.equal(shownHost("www.banque.fr"), "banque.fr");
  assert.equal(shownHost("banque.fr"), "banque.fr");
  assert.equal(shownHost("Voir le document"), null);
  assert.equal(shownHost("rapport.pdf"), null);
  assert.equal(shownHost("Cliquez ici : banque.fr"), null);
});

test("liens ordinaires et liens trompeurs", () => {
  assert.deepEqual(linkAction("https://banque.fr/x", "Mon compte"), { kind: "open", url: "https://banque.fr/x" });
  assert.equal(linkAction("https://www.client.banque.fr/x", "banque.fr").kind, "open");
  assert.equal(linkAction("https://BANQUE.fr/x", "https://www.banque.fr").kind, "open");
  const a = linkAction("https://evil.example/login", "https://www.banque.fr");
  assert.equal(a.kind, "confirm");
  assert.equal(a.shown, "banque.fr");
  assert.equal(a.real, "evil.example");
  // Le site affiché est un sous-domaine du site réel : trompeur.
  assert.equal(linkAction("https://example.invalid/", "banque.example.invalid").kind, "confirm");
  // Homographe : affiché en Unicode, réel en Punycode différent.
  assert.equal(linkAction("https://xn--bnque-6qa.fr/", "banque.fr").kind, "confirm");
});

test("schémas", () => {
  assert.equal(linkAction("javascript:alert(1)", "x").kind, "ignore");
  assert.equal(linkAction("data:text/html,x", "x").kind, "ignore");
  assert.equal(linkAction("file:///etc/passwd", "x").kind, "ignore");
  assert.equal(linkAction("pas une adresse", "x").kind, "ignore");
  assert.deepEqual(linkAction("mailto:Jean%20Dupont%20%3Cjean@exemple.fr%3E?subject=Bonjour", "écrire"), {
    kind: "mailto",
    to: "Jean Dupont <jean@exemple.fr>",
    cc: "",
    subject: "Bonjour",
    body: "",
  });
});

test("n° 23 : mailto avec to=, cc et body ; bcc ignoré", () => {
  const a = linkAction("mailto:?to=a@x.fr&to=b@y.fr&cc=c@z.fr&bcc=espion@w.fr&subject=Hi&body=Ligne%201%0D%0A%3Cb%3E2%3C%2Fb%3E", "écrire");
  assert.deepEqual(a, { kind: "mailto", to: "a@x.fr, b@y.fr", cc: "c@z.fr", subject: "Hi", body: "Ligne 1\n<b>2</b>" });
  assert.deepEqual(linkRequest(rewriteLink(a).href), a, "aller et retour par le backend");
});

test("corpus S6-a : le lien trompeur est signalé", () => {
  const eml = readFileSync(new URL("../corpus/attaques/S6-a-lien-navigation.eml", import.meta.url), "utf8");
  const links = [...eml.matchAll(/<a href="([^"]+)"[^>]*>([^<]*)<\/a>/g)].map(([, href, text]) => linkAction(href, text));
  assert.equal(links.length, 2);
  assert.equal(links[0].kind, "open");
  assert.equal(links[1].kind, "confirm");
});

test("liens réécrits : aller et retour par le backend", () => {
  const ok = rewriteLink(linkAction("https://banque.fr/x", "Mon compte"));
  assert.deepEqual(ok, { href: "https://banque.fr/x", title: "https://banque.fr/x" });

  const suspect = linkAction("https://evil.example/login?a=1&b=2", "www.banque.fr");
  const rewritten = rewriteLink(suspect);
  assert.equal(new URL(rewritten.href).hostname, LINK_HOST);
  assert.equal(rewritten.title, "https://evil.example/login?a=1&b=2");
  assert.deepEqual(linkRequest(rewritten.href), suspect);

  const mail = linkAction("mailto:jean@exemple.fr?subject=Re%20%3A%20devis", "écrire");
  assert.deepEqual(linkRequest(rewriteLink(mail).href), mail);

  assert.equal(rewriteLink(linkAction("javascript:alert(1)", "x")), null);
});

test("un message ne fabrique pas de lien interne", () => {
  assert.equal(linkAction(`https://${LINK_HOST}/mailto?to=x@y.z`, "x").kind, "ignore");
  assert.equal(linkRequest("https://evil.example/mailto?to=x@y.z").kind, "ignore");
  assert.equal(linkRequest(`https://${LINK_HOST}/suspect?url=javascript:alert(1)`).kind, "ignore");
});
