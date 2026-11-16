// Tests de l'interface (lot E4) : la version web en mode démonstration
// (src/mock.ts : comptes fictifs, aucun réseau), pilotée par un vrai Chromium.
// Lancement : make ui-test (ou npx playwright test).
import { defineConfig, devices } from "@playwright/test";

const PORT = 1430;

export default defineConfig({
  testDir: "tests/ui",
  fullyParallel: true,
  forbidOnly: true,
  retries: 0,
  reporter: "list",
  use: {
    baseURL: `http://localhost:${PORT}`,
    // Textes vérifiés en français, heures stables.
    locale: "fr-FR",
    timezoneId: "Europe/Paris",
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } } }],
  webServer: {
    command: `npx vite --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
