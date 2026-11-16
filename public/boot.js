// Squelette affiché pendant le chargement de l'interface (lot E2) : thème choisi
// (même clé que src/theme.ts) et type de fenêtre, lus avant le premier affichage.
// Fichier à part : la politique de sécurité refuse les scripts écrits dans la page.
try {
  var root = document.documentElement;
  var mode = localStorage.getItem("vortex.theme");
  if (mode === "light" || mode === "dark") root.setAttribute("data-boot", mode);
  if (new URLSearchParams(location.search).has("view")) root.setAttribute("data-boot-view", "secondary");
} catch (e) {
  // stockage indisponible : thème du système
}
