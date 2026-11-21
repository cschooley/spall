import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// spall serves the built UI from the Rust binary. In dev, Vite serves it and
// forwards the API to a running spall (default 127.0.0.1:3000, override
// with SPALL_URL). spall's Host check allows localhost, so keep the target
// on loopback.
const spall = process.env.SPALL_URL ?? "http://127.0.0.1:3000";

export default defineConfig({
  plugins: [react()],
  server: {
    host: "127.0.0.1",
    proxy: {
      "/events": { target: spall, changeOrigin: true },
      "/api": { target: spall, changeOrigin: true },
      "/healthz": { target: spall, changeOrigin: true },
    },
  },
  build: {
    outDir: "dist",
    assetsDir: "assets",
    sourcemap: false,
    // Keep everything as files: data: URIs for scripts would need a CSP hole.
    assetsInlineLimit: 0,
  },
});
