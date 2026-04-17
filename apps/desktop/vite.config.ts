import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev-server port so its WebView can connect.
const DEFAULT_PORT = 1420;

export default defineConfig(async () => ({
  plugins: [react()],

  clearScreen: false,
  server: {
    port: DEFAULT_PORT,
    strictPort: true,
    host: "localhost",
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2022",
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
}));
