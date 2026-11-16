// Préparation des tests du frontend sans navigateur (`npm test`) :
// - fuseau fixé (Europe/Paris), pour tester les changements d'heure ;
// - stockage local factice (réglages) ;
// - imports sans extension (`./locales/en`) résolus vers les fichiers `.ts`.
import * as nodeModule from "node:module";

process.env.TZ = "Europe/Paris";

// Remplacé sans être lu : les versions récentes de Node avertissent dès
// qu'on lit leur `localStorage` sans fichier de stockage.
const store = new Map();
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  writable: true,
  value: {
    getItem: (k) => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
    removeItem: (k) => store.delete(k),
  },
});

const relative = (s) => s.startsWith("./") || s.startsWith("../");
const hasExtension = (s) => /\.[cm]?[jt]sx?$/.test(s);

if (typeof nodeModule.registerHooks === "function") {
  // Node récent : crochets synchrones.
  nodeModule.registerHooks({
    resolve(specifier, context, next) {
      try {
        return next(specifier, context);
      } catch (error) {
        if (error?.code !== "ERR_MODULE_NOT_FOUND" || !relative(specifier) || hasExtension(specifier)) throw error;
        for (const ext of [".ts", ".tsx"]) {
          try {
            return next(specifier + ext, context);
          } catch {
            // extension suivante
          }
        }
        throw error;
      }
    },
  });
} else {
  nodeModule.register("./resolve.mjs", import.meta.url);
}
