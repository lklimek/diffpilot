import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

// Static page with the flow diagram alone (fictional fixture graphs), opened from disk or any static server.
export default defineConfig({
  root: fileURLToPath(new URL("preview/diagram", import.meta.url)),
  base: "./",
  plugins: [react(), tailwindcss()],
  build: { outDir: fileURLToPath(new URL("dist-diagram-preview", import.meta.url)), emptyOutDir: true },
});
