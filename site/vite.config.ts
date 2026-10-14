import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { content } from "./build/content.ts";

const repo = fileURLToPath(new URL("..", import.meta.url));

export default defineConfig({
  plugins: [react(), content(repo)],
  server: { port: 5181, fs: { allow: [repo] } },
  preview: { port: 5182 },
  build: { target: "es2022", assetsInlineLimit: 0 },
});
