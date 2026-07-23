import { defineConfig } from "vite";

// Tauri expects a fixed port and to fail if it's already in use.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2022",
    outDir: "dist",
  },
});
