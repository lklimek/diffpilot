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
          group: ["**/api/client", "**/api/queries", "**/screens/**", "**/app/**", "**/mocks/**"],
          message: "Components do not fetch or know their screen (rule A7).",
        },
      ],
    },
  ],
  "no-restricted-globals": [
    "error",
    ...["fetch", "XMLHttpRequest", "EventSource", "WebSocket"].map((name) => ({
      name,
      message: "Components do not fetch (rule A7).",
    })),
  ],
  "no-restricted-properties": [
    "error",
    ...["window", "globalThis", "self"].map((object) => ({
      object,
      property: "fetch",
      message: "Components do not fetch (rule A7).",
    })),
  ],
};

export default tseslint.config(
  { ignores: ["dist/", "test-results/", "playwright-report/", "src/api/generated/"] },
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
