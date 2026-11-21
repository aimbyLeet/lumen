import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `vite --mode mock`: the hub and the overlays (overlay.html) in a browser, the app stood in for by src/mock.
// The real build never sees it. Nothing inlined as data: the app's CSP forbids it.
export default defineConfig(({ mode }) => ({
  plugins: [react()],
  build: {
    assetsInlineLimit: 0,
    rollupOptions: { input: ["index.html", "overlay.html"] },
  },
  resolve:
    mode === "mock"
      ? {
          alias: {
            "@tauri-apps/api/core": "/src/mock/core.ts",
            "@tauri-apps/api/window": "/src/mock/window.ts",
            "@tauri-apps/api/event": "/src/mock/event.ts",
          },
        }
      : {},
}));
