import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import pkg from "./package.json";

// `npm run dev`: the API goes to a local lumen-server's admin (HTTPS, dev certificate).
// `npm run dev:mock`: no server, src/mock stands in for it (`?scene=...`); the real build never sees it.
// It lives under /admin: the domain's root is the landing page.
// Nothing inlined as data: the server's CSP forbids it. Lumen Client's styles and small parts are used as
// they are (web/client/src): one React for both, this app's.

export default defineConfig({
  base: "/admin/",
  plugins: [react()],
  define: { __VERSION__: JSON.stringify(pkg.version) },
  build: { assetsInlineLimit: 0 },
  resolve: { dedupe: ["react", "react-dom"] },
  server: {
    port: 5176,
    strictPort: true,
    fs: { allow: [".."] },
    proxy: { "/admin/api": { target: "https://127.0.0.1:7444", secure: false } },
  },
});
