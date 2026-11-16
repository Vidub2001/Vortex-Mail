import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const host = process.env.TAURI_DEV_HOST;

// Configuration recommandée par Tauri 2.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    // Le fuzzing (fuzz/) écrit des milliers de fichiers dans son corpus :
    // les surveiller épuiserait la limite du système (ENOSPC).
    watch: { ignored: ["**/src-tauri/**", "**/crates/**", "**/target/**", "**/fuzz/**"] },
  },
});
