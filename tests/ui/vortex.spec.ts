// Parcours principaux de l'interface (lot E4), sur les comptes fictifs de
// src/mock.ts. Chaque test part d'une page neuve : stockage local vide.
import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/** Ouvre une rédaction par « Nouveau courrier » et rend sa fenêtre. */
async function newMail(context: BrowserContext, page: Page): Promise<Page> {
  const [compose] = await Promise.all([
    context.waitForEvent("page"),
    page.getByRole("button", { name: "Nouveau courrier" }).click(),
  ]);
  await expect(compose.getByLabel("Objet")).toBeVisible();
  return compose;
}

/**
 * Clique sur « Envoyer ». La fenêtre se ferme dès l'envoi lancé (lot E1) :
 * un clic Playwright, qui attend encore la page après l'action, échouait
 * parfois sur « page fermée ». L'événement est donc envoyé sans attente.
 */
async function send(compose: Page) {
  await compose.getByRole("button", { name: "Envoyer", exact: true }).dispatchEvent("click");
}

/** Remplit destinataire, objet et texte. */
async function write(compose: Page, to: string, subject: string, body: string) {
  await compose.getByPlaceholder("Nom ou adresse").fill(to);
  await compose.getByLabel("Objet").fill(subject);
  await compose.getByLabel("Corps du message").click();
  await compose.keyboard.type(body);
}

test.beforeEach(async ({ page }) => {
  await page.goto("/");
});

test("démarrage : boîte de réception unifiée, dossiers et barre d'état", async ({ page }) => {
  await expect(page.getByText("Pièces manquantes pour la clôture")).toBeVisible();
  await expect(page.getByText("Votre relevé d'octobre est disponible")).toBeVisible();
  // Lot E3 : dossiers spéciaux dans la langue de l'interface.
  await expect(page.getByText("Éléments envoyés").first()).toBeVisible();
  // Lot E2 : pas d'indicateur d'attente, l'heure de la dernière synchronisation.
  await expect(page.getByText(/À jour à \d/)).toBeVisible();
  await expect(page.getByText("Synchronisation…")).toHaveCount(0);
});

test("lecture : le corps s'affiche dans un cadre sans script", async ({ page }) => {
  await page.getByText("Repas de dimanche").click();
  const frame = page.locator("iframe").first();
  // Défense du modèle de menace : le cadre du message n'exécute jamais de script.
  await expect(frame).toHaveAttribute("sandbox", /^(?!.*allow-scripts).*$/);
  await expect(page.frameLocator("iframe").first().locator("body")).toContainText("On se retrouve à 12 h 30 dimanche ?");
});

test("lecture : un lien trompeur est réécrit avant l'affichage (audit S2)", async ({ page }) => {
  await page.getByText("Votre relevé d'octobre est disponible").click();
  const link = page.frameLocator("iframe").first().getByText("www.banque.example.com");
  await expect(link).toHaveAttribute("href", /^https:\/\/[^/]+\/suspect\?/);
  await expect(link).toHaveAttribute("title", "https://banque-exemple.example.net/connexion");
});

test("envoi : la fenêtre se ferme aussitôt, le résultat arrive dans la fenêtre principale", async ({ context, page }) => {
  const compose = await newMail(context, page);
  await write(compose, "marie.dupont@example.com", "Dimanche", "C'est noté !");
  const closed = compose.waitForEvent("close");
  await send(compose);
  await closed;
  await expect(page.getByText("Message envoyé", { exact: true })).toBeVisible();
  // Rien ne reste à reprendre : la copie de secours est retirée.
  await expect(page.getByText("Brouillon non enregistré retrouvé")).toHaveCount(0);
});

test("P7 : texte brut et citation calculés à la demande", async ({ context, page }) => {
  await page.getByText("Repas de dimanche").click();
  await page.getByTitle("Affichage du message").click();
  await page.getByRole("menuitemradio", { name: "Texte brut" }).click();
  await expect(page.frameLocator("iframe").first().locator("body")).toContainText("On se retrouve à 12 h 30 dimanche ?");
  const [reply] = await Promise.all([
    context.waitForEvent("page"),
    page.getByRole("button", { name: "Répondre", exact: true }).first().click(),
  ]);
  await expect(reply.getByLabel("Corps du message")).toContainText("On se retrouve à 12 h 30 dimanche ?");
});

test("J13 : destinataire d'un message envoyé proposé, puis écarté", async ({ context, page }) => {
  const compose = await newMail(context, page);
  const to = compose.getByPlaceholder("Nom ou adresse");
  await to.fill("jules");
  const option = compose.getByRole("option", { name: /jules\.bernard@partenaire\.example\.org/ });
  await expect(option).toBeVisible();
  await compose.getByRole("button", { name: "Ne plus proposer jules.bernard@partenaire.example.org" }).click();
  await expect(option).toHaveCount(0);
  await to.fill("");
  await to.fill("jules");
  // Délai de la complétion (120 ms) passé : toujours rien (les options du choix du compte ne comptent pas).
  await compose.waitForTimeout(400);
  await expect(compose.locator(".address-suggestions [role=option]")).toHaveCount(0);
});

test("envoi refusé : la rédaction se rouvre avec le message et l'erreur", async ({ context, page }) => {
  const compose = await newMail(context, page);
  await write(compose, "echec@example.com", "Message à renvoyer", "Texte gardé");
  const [reopened] = await Promise.all([context.waitForEvent("page"), send(compose)]);
  await expect(page.getByText("Message non envoyé", { exact: true })).toBeVisible();
  await expect(reopened.getByLabel("Objet")).toHaveValue("Message à renvoyer");
  await expect(reopened.getByText(/550 destinataire refusé/)).toBeVisible();
  await expect(reopened.getByLabel("Corps du message")).toContainText("Texte gardé");
});

test("envoi sans destinataire : la fenêtre reste ouverte", async ({ context, page }) => {
  const compose = await newMail(context, page);
  await compose.getByLabel("Objet").fill("Sans destinataire");
  await compose.getByRole("button", { name: "Envoyer", exact: true }).click();
  await expect(compose.getByLabel("Objet")).toBeVisible();
  expect(compose.isClosed()).toBe(false);
});

test("F5 : la touche Suppr dans Contacts ne touche pas au message sélectionné", async ({ page }) => {
  await page.getByText("Repas de dimanche").click();
  await page.getByRole("button", { name: "Contacts" }).click();
  // La vue Courrier reste dans la page, cachée : seul le contact visible compte.
  await expect(page.getByRole("button", { name: "Nouveau contact" })).toBeVisible();
  await page.getByText("Sophie Bernard").filter({ visible: true }).first().click();
  await page.keyboard.press("Delete");
  await page.getByRole("button", { name: "Boîte de réception unifiée" }).click();
  // Toujours dans la liste (et encore ouvert dans le volet de lecture).
  await expect(page.getByText("Repas de dimanche").first()).toBeVisible();
  await expect(page.getByText("Déplacé vers la corbeille")).toHaveCount(0);
});

test("F4 : un rendez-vous de plus de 24 h modifié garde ses heures", async ({ page }) => {
  await page.getByRole("button", { name: "Agenda" }).click();
  // Un rendez-vous existant s'ouvre sur sa fiche ; « Modifier » mène à l'éditeur (lot H1b).
  await page.getByText("Conférence DevOps").first().click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "Modifier" }).click();
  await dialog.getByLabel("Lieu").fill("Lyon, salle Rhône");
  await dialog.getByRole("button", { name: "Enregistrer" }).click();
  await expect(dialog).toHaveCount(0);
  await page.getByText("Conférence DevOps").first().click();
  await expect(dialog.getByText("Lyon, salle Rhône")).toBeVisible();
  await dialog.getByRole("button", { name: "Modifier" }).click();
  await expect(dialog.getByLabel("Lieu")).toHaveValue("Lyon, salle Rhône");
  await expect(dialog.getByLabel("Heure de début")).toHaveValue("09:00");
  await expect(dialog.getByLabel("Heure de fin")).toHaveValue("17:00");
  await expect(dialog.getByLabel("Toute la journée")).not.toBeChecked();
});

test("F12 : abandonner pendant un enregistrement lent ne laisse aucun brouillon", async ({ context, page }) => {
  // Serveur lent : l'enregistrement du brouillon dure deux secondes.
  await page.evaluate(() => localStorage.setItem("vortex.demo.lenteur", "2000"));
  const compose = await newMail(context, page);
  await write(compose, "marie.dupont@example.com", "Brouillon abandonné", "À jeter");
  await compose.getByRole("button", { name: "Enregistrer", exact: true }).click();
  await compose.getByRole("button", { name: "Abandonner", exact: true }).click();
  const closed = compose.waitForEvent("close");
  await compose.getByRole("dialog").getByRole("button", { name: "Supprimer" }).click();
  await closed;
  const onServer = await page.evaluate(() => JSON.parse(localStorage.getItem("vortex.demo.brouillons") ?? "[]"));
  expect(onServer).toEqual([]);
});

test("G1 : un expéditeur refusé par DMARC est signalé", async ({ page }) => {
  await page.getByText("Action requise : vérifiez votre compte").click();
  await expect(page.getByRole("alert").filter({ hasText: "Expéditeur non vérifié" })).toContainText("banque.example.com");
  await expect(page.getByRole("alert").filter({ hasText: "Expéditeur non vérifié" })).toContainText("(DMARC)");
  // Un expéditeur authentifié n'a pas de bandeau.
  await page.getByText("Repas de dimanche").click();
  await expect(page.getByText("Expéditeur non vérifié")).toHaveCount(0);
});

test("E6 : recherche dans tous les dossiers, puis retour à la boîte", async ({ page }) => {
  await expect(page.getByText("Votre relevé d'octobre est disponible")).toBeVisible();
  const search = page.getByPlaceholder("Rechercher partout");
  // Mot du corps du message seulement, pas de son objet.
  await search.fill("justificatifs");
  await search.press("Enter");
  const list = page.getByRole("listbox", { name: "Messages" });
  await expect(list.getByText("Pièces manquantes pour la clôture")).toBeVisible();
  await expect(list.getByText("Votre relevé d'octobre est disponible")).toHaveCount(0);
  await search.press("Escape");
  await expect(page.getByText("Votre relevé d'octobre est disponible")).toBeVisible();
});

test("E6 : déplacer un message vers un dossier d'un autre nom", async ({ page }) => {
  const list = page.getByRole("listbox", { name: "Messages" });
  await list.getByText("Pièces manquantes pour la clôture").click();
  await page.getByTitle("Déplacer vers").click();
  await page.getByRole("menuitem", { name: "Clients", exact: true }).click();
  // Titre de la notification (le texte est aussi lu par la zone annoncée aux lecteurs d'écran).
  await expect(page.getByText("1 élément déplacé vers « Clients » (OVH pro)", { exact: true })).toBeVisible();
  // Plus dans la boîte de réception unifiée.
  await expect(list.getByText("Pièces manquantes pour la clôture")).toHaveCount(0);
});

test("Q : clic droit › Déplacer, recherche parmi les dossiers puis dossier récent", async ({ page }) => {
  const list = page.getByRole("listbox", { name: "Messages" });
  await list.getByText("Pièces manquantes pour la clôture").click({ button: "right" });
  // Survol de « Déplacer » : le sous-menu s'ouvre à droite, sans clic ;
  // aucun dossier récent au départ.
  await page.getByRole("menuitem", { name: "Déplacer", exact: true }).hover();
  const folders = page.getByRole("listbox", { name: "Dossiers" });
  await expect(folders.getByText("Tapez le nom d'un dossier pour le trouver.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Créer un dossier…" })).toBeVisible();
  // Survol d'une autre entrée : le sous-menu disparaît, puis revient.
  await page.getByRole("menuitem", { name: "Enregistrer (.eml)" }).hover();
  await expect(folders).toHaveCount(0);
  await page.getByRole("menuitem", { name: "Déplacer", exact: true }).hover();
  await page.getByRole("textbox", { name: "Rechercher un dossier" }).fill("LIENTS");
  // « Client A » ne contient pas « lients » : un seul dossier reste.
  await expect(folders.getByRole("option")).toHaveCount(1);
  await page.keyboard.press("Enter");
  await expect(page.getByText("1 élément déplacé vers « Clients » (OVH pro)", { exact: true })).toBeVisible();
  await expect(list.getByText("Pièces manquantes pour la clôture")).toHaveCount(0);
  // Au clic droit suivant, « Clients » est dans les récents.
  await list.getByText("Repas de dimanche").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Déplacer", exact: true }).hover();
  await expect(folders.getByRole("option", { name: /^Clients/ })).toBeVisible();
});

test("Q : Créer un dossier depuis Déplacer, nom repris de la recherche et compte au choix", async ({ page }) => {
  const list = page.getByRole("listbox", { name: "Messages" });
  await list.getByText("Repas de dimanche").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Déplacer", exact: true }).hover();
  await page.getByRole("textbox", { name: "Rechercher un dossier" }).fill("Projet X");
  await page.getByRole("button", { name: "Créer un dossier…" }).click();
  const dialog = page.getByRole("dialog", { name: "Nouveau dossier" });
  await expect(dialog.getByRole("textbox", { name: "Nom du dossier" })).toHaveValue("Projet X");
  // Compte du message par défaut, un autre au choix.
  const account = dialog.getByRole("combobox", { name: "Dans le compte" });
  await expect(account).toHaveValue("perso");
  await account.selectOption({ label: "OVH pro" });
  await expect(account).toHaveValue("pro");
  await dialog.getByRole("button", { name: "Annuler" }).click();
  await expect(dialog).toHaveCount(0);
});

test("E6 : paramètres et licences chargés à la demande", async ({ page }) => {
  await page.getByRole("button", { name: "Comptes et paramètres" }).click();
  const settings = page.getByRole("dialog", { name: "Paramètres" });
  await expect(settings.getByRole("heading", { name: "Paramètres" })).toBeVisible();
  await settings.getByRole("button", { name: "À propos" }).click();
  await expect(settings.getByText(/Vortex Mail utilise \d+ bibliothèques open source/)).toBeVisible();
  await settings.getByRole("button", { name: "Fermer" }).click();
  await expect(settings).toHaveCount(0);
  // Rouverts : la fenêtre déjà chargée revient aussitôt.
  await page.getByRole("button", { name: "Comptes et paramètres" }).click();
  await expect(page.getByRole("dialog", { name: "Paramètres" })).toBeVisible();
});

/** Ouvre l'assistant d'ajout de compte (lot I2) et cherche les réglages de l'adresse. */
async function wizard(page: Page, email: string) {
  await page.getByRole("button", { name: "Comptes et paramètres" }).click();
  const settings = page.getByRole("dialog", { name: "Paramètres" });
  await settings.getByRole("button", { name: "Ajouter un compte" }).click();
  await settings.getByLabel("Votre nom").fill("Camille Martin");
  await settings.getByLabel("Adresse e-mail").fill(email);
  await settings.getByRole("button", { name: "Continuer" }).click();
  return settings;
}

test("I2 : fournisseur trouvé, compte testé puis ajouté", async ({ page }) => {
  const settings = await wizard(page, "camille@exemple.fr");
  await expect(settings.getByText("Source : base des fournisseurs de Thunderbird (copie intégrée)")).toBeVisible();
  await expect(settings.getByText("imap.exemple.fr:993 (SSL/TLS)")).toBeVisible();
  await settings.getByLabel("Mot de passe").fill("secret");
  await settings.getByRole("button", { name: "Se connecter" }).click();
  await expect(page.getByText("Compte ajouté", { exact: true })).toBeVisible();
  // Retour à la liste, avec le nouveau compte.
  await expect(settings.getByRole("heading", { name: "Comptes de messagerie" })).toBeVisible();
  await expect(settings.getByText("camille@exemple.fr")).toBeVisible();
});

test("I2 : mot de passe refusé, l'erreur s'affiche et rien n'est ajouté", async ({ page }) => {
  const settings = await wizard(page, "camille@exemple.fr");
  await settings.getByLabel("Mot de passe").fill("faux");
  await settings.getByRole("button", { name: "Se connecter" }).click();
  await expect(settings.getByText("Identifiant ou mot de passe refusé (AUTHENTICATIONFAILED)").first()).toBeVisible();
  await expect(settings.getByRole("button", { name: "Se connecter" })).toBeEnabled();
  await expect(page.getByText("Compte ajouté", { exact: true })).toHaveCount(0);
});

test("I2 : serveurs hors du domaine signalés, domaine inconnu vers le formulaire complet", async ({ page }) => {
  const settings = await wizard(page, "camille@hebergeur.example");
  await expect(settings.getByText("Serveurs hors de votre domaine")).toBeVisible();
  await settings.getByLabel("Adresse e-mail").fill("camille@inconnu.example");
  await settings.getByRole("button", { name: "Continuer" }).click();
  await expect(settings.getByText("Réglages introuvables")).toBeVisible();
  await settings.getByRole("button", { name: "Modifier à la main" }).click();
  await expect(settings.getByRole("heading", { name: "Ajouter un compte IMAP" })).toBeVisible();
  // Adresse, nom et identifiant repris de l'assistant.
  await expect(settings.getByLabel("Adresse e-mail")).toHaveValue("camille@inconnu.example");
  await expect(settings.getByLabel("Votre nom")).toHaveValue("Camille Martin");
});

test("I3 : Proton Mail par Bridge, certificat confirmé avant le mot de passe", async ({ page }) => {
  const settings = await wizard(page, "camille@proton.me");
  await expect(settings.getByText("Proton Mail passe par Proton Mail Bridge")).toBeVisible();
  await expect(settings.getByRole("button", { name: "Se connecter" })).toBeDisabled();
  await settings.getByRole("button", { name: "Lire le certificat de Bridge" }).click();
  await expect(settings.getByText(/^([0-9A-F]{2}:){31}[0-9A-F]{2}$/)).toBeVisible();
  // Le mot de passe de Bridge n'est demandé qu'une fois le certificat accepté.
  await expect(settings.getByLabel("Mot de passe de Bridge")).toHaveCount(0);
  await settings.getByRole("checkbox").check();
  await settings.getByLabel("Mot de passe de Bridge").fill("secret");
  await settings.getByRole("button", { name: "Se connecter" }).click();
  await expect(page.getByText("Compte ajouté", { exact: true })).toBeVisible();
  // Le compte enregistré garde l'empreinte, montrée dans son formulaire.
  await settings.getByRole("button", { name: "Actions pour Proton Mail" }).click();
  await page.getByRole("menuitem", { name: "Modifier" }).click();
  await expect(settings.getByRole("group", { name: "Proton Mail Bridge" })).toBeVisible();
  await expect(settings.getByText(/^([0-9A-F]{2}:){31}[0-9A-F]{2}$/)).toBeVisible();
});

test("Gmail : mot de passe d'application dans l'assistant, application Google par le formulaire complet", async ({ page }) => {
  const settings = await wizard(page, "camille@gmail.com");
  await expect(settings.getByLabel("Mot de passe d'application")).toBeVisible();
  await settings.getByLabel("Mot de passe d'application").fill("secret");
  await expect(settings.getByRole("button", { name: "Se connecter", exact: true })).toBeEnabled();
  await settings.getByRole("button", { name: "Se connecter avec Google" }).click();
  await expect(settings.getByRole("heading", { name: "Ajouter un compte IMAP" })).toBeVisible();
  await expect(settings.getByLabel("Adresse e-mail")).toHaveValue("camille@gmail.com");
});

// Suppression d'une sélection de plusieurs messages (signalé par un testeur le
// 17 novembre 2026) : touche Suppr, bouton de la barre et clic droit › Supprimer
// doivent tous envoyer toute la sélection, de deux comptes différents, aux
// éléments supprimés.
const twoAccounts = ["Repas de dimanche", "Pièces manquantes pour la clôture"];

async function selectTwo(page: Page) {
  const list = page.getByRole("listbox", { name: "Messages" });
  await list.getByText(twoAccounts[0]).click();
  await list.getByText(twoAccounts[1]).click({ modifiers: ["Control"] });
  await expect(list.locator(".msg.selected")).toHaveCount(2);
  return list;
}

async function bothGone(page: Page) {
  const list = page.getByRole("listbox", { name: "Messages" });
  for (const subject of twoAccounts) await expect(list.getByText(subject)).toHaveCount(0);
  await expect(page.getByText("Votre relevé d'octobre est disponible")).toBeVisible();
}

test("sélection supprimée par la touche Suppr", async ({ page }) => {
  await selectTwo(page);
  await page.keyboard.press("Delete");
  await bothGone(page);
});

test("sélection supprimée par le bouton de la barre", async ({ page }) => {
  await selectTwo(page);
  await page.getByRole("toolbar", { name: "Actions sur la sélection" }).getByRole("button", { name: "Supprimer" }).click();
  await bothGone(page);
});

test("sélection supprimée par clic droit › Supprimer", async ({ page }) => {
  const list = await selectTwo(page);
  await list.getByText(twoAccounts[0]).click({ button: "right" });
  await page.getByRole("menuitem", { name: "Supprimer" }).click();
  await bothGone(page);
});

test("Restaurer : un message supprimé revient dans son dossier d'origine", async ({ page }) => {
  const list = page.getByRole("listbox", { name: "Messages" });
  await list.getByText("Repas de dimanche").click();
  await page.keyboard.press("Delete");
  await expect(list.getByText("Repas de dimanche")).toHaveCount(0);
  await page.getByRole("treeitem").filter({ hasText: "Éléments supprimés" }).first().click();
  await list.getByText("Repas de dimanche").click();
  await page.getByRole("toolbar", { name: "Actions sur la sélection" }).getByRole("button", { name: "Restaurer" }).click();
  await expect(page.getByText("1 élément restauré", { exact: true })).toBeVisible();
  await expect(list.getByText("Repas de dimanche")).toHaveCount(0);
  await page.getByRole("treeitem").filter({ hasText: "Boîte de réception" }).first().click();
  await expect(list.getByText("Repas de dimanche")).toBeVisible();
});

test("menu de sélection : non lus, aucun, tous", async ({ page }) => {
  const list = page.getByRole("listbox", { name: "Messages" });
  await expect(list.locator(".msg").first()).toBeVisible();
  const unread = await list.locator(".msg.unread").count();
  expect(unread).toBeGreaterThan(1);
  await page.getByRole("checkbox", { name: "Sélectionner", exact: true }).click();
  await page.getByRole("menuitem", { name: "Non lus", exact: true }).click();
  await expect(list.locator(".msg.selected")).toHaveCount(unread);
  await expect(list.locator(".msg.selected:not(.unread)")).toHaveCount(0);
  await page.getByRole("checkbox", { name: "Sélectionner", exact: true }).click();
  await page.getByRole("menuitem", { name: "Aucun", exact: true }).click();
  await expect(list.locator(".msg.selected")).toHaveCount(0);
  await page.getByRole("checkbox", { name: "Sélectionner", exact: true }).click();
  await page.getByRole("menuitem", { name: "Tous", exact: true }).click();
  await expect(list.locator(".msg.selected")).toHaveCount(await list.locator(".msg").count());
});

test("clic droit › Marquer comme lu sur toute la sélection", async ({ page }) => {
  const list = page.getByRole("listbox", { name: "Messages" });
  await expect(list.locator(".msg").first()).toBeVisible();
  await page.getByRole("checkbox", { name: "Sélectionner", exact: true }).click();
  await page.getByRole("menuitem", { name: "Non lus", exact: true }).click();
  await list.locator(".msg.selected").first().click({ button: "right" });
  await page.getByRole("menuitem", { name: "Marquer comme lu" }).click();
  await expect(list.locator(".msg.unread")).toHaveCount(0);
});
