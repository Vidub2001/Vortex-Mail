// Fichier de test des règles vortex-ts.yml (semgrep --test), jamais compilé.

function sinks(el: HTMLElement, html: string) {
  // ruleid: vortex-raw-html-sink
  el.innerHTML = html;
  // ruleid: vortex-raw-html-sink
  el.outerHTML = html;
  // ruleid: vortex-raw-html-sink
  el.insertAdjacentHTML("beforeend", html);
  // ruleid: vortex-raw-html-sink
  const props = { dangerouslySetInnerHTML: { __html: html } };
  // ruleid: vortex-raw-html-sink
  const fragment = document.createRange().createContextualFragment(html);
  // ruleid: vortex-raw-html-sink
  document.execCommand("insertHTML", false, html);
  // ruleid: vortex-raw-html-sink
  const frame = <iframe srcDoc={html} />;
  // ruleid: vortex-raw-html-sink
  (el as HTMLIFrameElement).srcdoc = html;
  // ruleid: vortex-raw-html-sink
  el.innerHTML += html;
  // ruleid: vortex-raw-html-sink
  el.setAttribute("srcdoc", html);
  // ruleid: vortex-raw-html-sink
  (el as HTMLIFrameElement).contentDocument?.write(html);
  // ruleid: vortex-raw-html-sink
  window.open("about:blank")?.document.write(html);
  // ok: vortex-raw-html-sink
  el.setAttribute("title", html);
  // ok: vortex-raw-html-sink
  document.execCommand("insertText", false, html);
  // ok: vortex-raw-html-sink
  el.textContent = html;
  // ok: vortex-raw-html-sink
  const copy = el.innerHTML;
  return [props, copy, fragment, frame];
}

function reparse(html: string) {
  // ruleid: vortex-html-reparse
  const doc = new DOMParser().parseFromString(html, "text/html");
  const parser = new DOMParser();
  // ruleid: vortex-html-reparse
  const again = parser.parseFromString(html, "text/html");
  return [doc, again];
}
