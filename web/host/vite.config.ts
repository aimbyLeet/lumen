import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `vite --mode mock`: the window in a browser, the service stood in for by src/mock (`?scene=...`).
// The real build never sees it. Nothing inlined as data: the app's CSP forbids it. Lumen Client's styles and
// small parts are used as they are (web/client/src): one React for both, this app's.
export default defineConfig(({ mode }) => ({
  plugins: [react()],
  build: { assetsInlineLimit: 0 },
  resolve: {
    dedupe: ["react", "react-dom"],
    alias: (mode === "mock"
      ? {
          "@tauri-apps/api/core": "/src/mock/core.ts",
          "@tauri-apps/api/window": "/src/mock/window.ts",
        }
      : {}) as Record<string, string>,
  },
  server: { fs: { allow: [".."] } },
}));
