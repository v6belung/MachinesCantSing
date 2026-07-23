import { defineConfig } from "vite";

// Tauri expects a fixed port and to fail if it's already in use.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Vite has no reason to watch Rust build output; without this, its watcher can
      // race Cargo writing the .exe mid-compile and crash with EBUSY on Windows.
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2022",
    outDir: "dist",
  },
});
