// @vitest-environment node
import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";

// Rule A7 (plan §4.1): components in src/components take props and return callbacks.
const eslint = new ESLint({ cwd: new URL("..", import.meta.url).pathname });
const lintComponent = async (code: string, file = "src/components/x/X.tsx") => {
  const [result] = await eslint.lintText(code, { filePath: file });
  return result!.messages.map((m) => m.ruleId);
};

describe("ESLint rule A7", () => {
  it.each([
    ['import { useNavigate } from "react-router";', "no-restricted-imports"],
    ['import { Link } from "react-router/dom";', "no-restricted-imports"],
    ['import { useQuery } from "@tanstack/react-query";', "no-restricted-imports"],
    ['import { api } from "../../api/client";', "no-restricted-imports"],
    ['import { useTour } from "../../api/queries";', "no-restricted-imports"],
    ['import { TourScreen } from "../../screens/tour/TourScreen";', "no-restricted-imports"],
    ['import { handlers } from "../../mocks/handlers";', "no-restricted-imports"],
    ['export const load = () => fetch("/api/v1/me");', "no-restricted-globals"],
    ['import { api } from "../../api/client.js";', "no-restricted-imports"],
    ['import { api } from "../../api/client.ts";', "no-restricted-imports"],
    ['import { useTour } from "../../api/queries.js";', "no-restricted-imports"],
    ['import { api } from "@/api/client";', "no-restricted-imports"],
    ['export const load = () => import("../../api/client");', "no-restricted-syntax"],
    ['export const load = () => import("./other");', "no-restricted-syntax"],
    ['export const load = () => window.fetch("/api/v1/me");', "no-restricted-globals"],
    ['export const load = () => globalThis["fetch"]("/api/v1/me");', "no-restricted-globals"],
    ["export const x = () => new window.XMLHttpRequest();", "no-restricted-globals"],
    ['export const s = new self.EventSource("/x");', "no-restricted-globals"],
    ['export const w = new globalThis.WebSocket("ws://x");', "no-restricted-globals"],
    ['export const s = new EventSource("/x");', "no-restricted-globals"],
    ['export const b = () => navigator.sendBeacon("/x");', "no-restricted-properties"],
    ['export const b = () => window.navigator.sendBeacon("/x");', "no-restricted-properties"],
  ])("rejects %s in a component", async (code, rule) => {
    expect(await lintComponent(code)).toContain(rule);
  });

  it("allows contract types and other components", async () => {
    const code =
      'import type { Tour } from "../../api/generated/types";\nimport { Tooltip } from "../tooltip/Tooltip";\nexport type T = Tour;\nexport const C = Tooltip;\n';
    expect(await lintComponent(code)).toEqual([]);
  });

  it("does not restrict screens", async () => {
    expect(
      await lintComponent(
        'import { useParams } from "react-router";\nexport const p = useParams;\n',
        "src/screens/x/X.tsx",
      ),
    ).toEqual([]);
  });
});
