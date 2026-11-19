import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

/** Rule A7 (plan §4.1): block components get props and return callbacks; no fetching, routing or screens. */
export const componentRules = {
  "no-restricted-imports": [
    "error",
    {
      patterns: [
        {
          group: ["react-router", "react-router/*", "@tanstack/*"],
          message: "Components do not route or query (rule A7): take props, return callbacks.",
        },
        {
          // Any extension or alias prefix: `../../api/client.js`, `@/screens/x`.
          regex: "(^|/)(api/(client|queries)|screens|app|mocks)(/|\\.|$)",
          message: "Components do not fetch or know their screen (rule A7).",
        },
      ],
    },
  ],
  "no-restricted-syntax": [
    "error",
    { selector: "ImportExpression", message: "Components do not load modules at runtime (rule A7)." },
  ],
  // `checkGlobalObject` also catches `window.fetch`, `globalThis["fetch"]`, `new self.EventSource(...)`.
  "no-restricted-globals": [
    "error",
    {
      checkGlobalObject: true,
      globals: ["fetch", "XMLHttpRequest", "EventSource", "WebSocket"].map((name) => ({
        name,
        message: "Components do not fetch (rule A7).",
      })),
    },
  ],
  "no-restricted-properties": [
    "error",
    { property: "sendBeacon", message: "Components do not send requests (rule A7)." },
  ],
};

export default tseslint.config(
  {
    ignores: ["dist/", "dist-diagram-preview/", "test-results/", "playwright-report/", "src/api/generated/"],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    plugins: { "react-hooks": reactHooks },
    languageOptions: { globals: { ...globals.browser } },
    rules: { ...reactHooks.configs.recommended.rules },
  },
  { files: ["src/components/**/*.{ts,tsx}"], ignores: ["**/*.test.{ts,tsx}"], rules: componentRules },
  {
    files: ["*.config.{js,ts}", "tests/**/*.ts", "src/**/*.test.{ts,tsx}", "src/__tests__/**"],
    languageOptions: { globals: { ...globals.node } },
  },
);
