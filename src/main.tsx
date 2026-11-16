import React from "react";
import ReactDOM from "react-dom/client";
import "./styles.css";
import { api, runningInTauri } from "./api";
import { UnlockScreen } from "./components/UnlockScreen";
import { lang } from "./i18n";
import { bootstrapSecondaryWindow } from "./windows";

// Fenêtre principale, ou fenêtre secondaire (message, rédaction) selon l'adresse.
const params = new URLSearchParams(window.location.search);

// Réglages des versions précédentes (« fluxmail.* ») : repris sous « vortex.* ».
try {
  for (const key of Object.keys(localStorage)) {
    if (!key.startsWith("fluxmail.")) continue;
    const value = localStorage.getItem(key);
    const renamed = `vortex.${key.slice("fluxmail.".length)}`;
    if (value !== null && key !== "fluxmail.microsoftClientId" && localStorage.getItem(renamed) === null) {
      localStorage.setItem(renamed, value);
    }
    localStorage.removeItem(key);
  }
} catch {
  // stockage indisponible : réglages par défaut
}

// Langue de l'interface : pour le moteur web (césure, correcteur), les notifications
// et les noms des dossiers spéciaux, donnés par le backend dans cette langue (lot E3).
let languageSet: Promise<void> = Promise.resolve();
if (!params.has("view")) {
  document.documentElement.lang = lang();
  languageSet = api.setLanguage(lang()).catch(() => {});
}

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);
// Chaque fenêtre ne charge que son code (lot E6) : la rédaction et la lecture
// d'un message n'attendent pas celui de la fenêtre principale, et inversement.
// Le chargement commence tout de suite, en même temps que les échanges avec le backend.
const view = params.has("view")
  ? import("./Popup").then(({ default: Popup }) => <Popup params={params} />)
  : import("./App").then(({ default: App }) => <App />);
const start = () => {
  document.documentElement.lang = lang();
  void view.then((element) => root.render(<React.StrictMode>{element}</React.StrictMode>));
};

// Mode portable : mot de passe maître avant tout (fenêtre principale seulement).
if (runningInTauri && !params.has("view")) {
  languageSet
    .then(() => api.portableStatus())
    .then((st) => {
      if (st.portable && !st.unlocked) {
        root.render(<UnlockScreen creating={!st.hasVault} onUnlocked={start} />);
      } else {
        start();
      }
    })
    .catch(start);
} else if (runningInTauri && params.has("view")) {
  // Fenêtre secondaire : réglages et brouillon de départ, avant le premier affichage.
  bootstrapSecondaryWindow(params)
    .catch(() => {})
    .finally(start);
} else {
  start();
}
