// Pastilles de couleur nommées en clair (relecture complète n° 35) : `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { colorName } from "../../src/colors.ts";
import { t } from "../../src/i18n.ts";

test("n° 35 : couleurs des comptes et des agendas nommées", () => {
  assert.equal(colorName("#c62828"), t("Rouge"));
  assert.equal(colorName("#0f6cbd"), t("Bleu"));
  assert.equal(colorName("#107c10"), t("{color} foncé", { color: t("Vert") }));
  assert.equal(colorName("#6b7280"), t("Gris"));
  assert.equal(colorName("#000000"), t("Noir"));
  assert.equal(colorName("#ffffff"), t("Blanc"));
  assert.equal(colorName("transparent"), t("Aucune"));
  assert.equal(colorName("pas une couleur"), "pas une couleur");
});
