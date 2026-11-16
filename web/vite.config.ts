import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { msw } from "msw/vite";
import { defineConfig } from "vitest/config";

const repoRoot = fileURLToPath(new URL("..", import.meta.url));

// `--mode mock` serves the msw worker script; the mocks read fixtures/pr-482 outside web/.
export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss(), ...(mode === "mock" ? [msw({ mode: "worker-only" })] : [])],
  server: {
    fs: { allow: [repoRoot] },
    proxy:
      mode === "mock"
        ? undefined
        : {
            "/api": "http://localhost:3000",
            "/auth": "http://localhost:3000",
          },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["src/__tests__/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
}));
