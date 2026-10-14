import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5183,
    fs: { allow: [".."] },
    proxy: { "/api": { target: "http://127.0.0.1:7400", changeOrigin: false } },
  },
  build: { target: "es2022", assetsInlineLimit: 0 },
});
