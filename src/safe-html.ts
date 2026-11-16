// Seul module autorisé à transformer du HTML en éléments de la page (règle
// Semgrep `vortex-raw-html-sink`, lot 4 de docs/plan-lots.md).
//
// Le HTML venant d'un mail arrive déjà nettoyé par le backend (liste blanche,
// `src-tauri/src/sanitize.rs`). Il est analysé UNE seule fois, dans un
// <template> : un document inerte, où rien ne s'exécute et où aucune image
// n'est chargée. Les éléments obtenus sont ensuite déplacés tels quels dans la
// page, sans repasser par une chaîne de caractères : pas de seconde analyse
// qui pourrait comprendre le HTML autrement que la première (mXSS, relecture S3).

/** HTML → fragment inerte, prêt à être inséré. */
export function parseHtml(html: string): DocumentFragment {
  const template = document.createElement("template");
  template.innerHTML = html;
  return template.content;
}

/** Remplace le contenu de `el` par ce HTML. */
export function setHtml(el: Element, html: string): void {
  el.replaceChildren(parseHtml(html));
}
