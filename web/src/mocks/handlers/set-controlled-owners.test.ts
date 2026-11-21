import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import type { ControlledOwners } from "../../api/generated/types";
import { handlers } from "./index";
import { resetControlledOwners } from "./set-controlled-owners";

const BASE = location.origin;
const HEADERS = { "X-CSRF-Token": "fixture-csrf-token-482", "Content-Type": "application/json" };
const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
afterEach(resetControlledOwners);
afterAll(() => server.close());

const PATH = "/api/v1/settings/controlled-owners";

async function put(body: unknown): Promise<{ status: number; body: unknown }> {
  const response = await fetch(`${BASE}${PATH}`, {
    method: "PUT",
    headers: HEADERS,
    body: JSON.stringify(body),
  });
  return { status: response.status, body: await response.json() };
}

describe("msw controlled owners (R16) behave like the server", () => {
  it("trims, keeps the first spelling of case-only duplicates and sorts", async () => {
    const saved = await put({ owners: [" Acme ", "dashpay", "DASHPAY", "acme"] });
    expect(saved.status).toBe(200);
    expect((saved.body as ControlledOwners).owners).toEqual(["Acme", "dashpay"]);
  });

  it("answers 400 invalid_request for a non-login or more than 50 owners", async () => {
    for (const owners of [
      ["-bad"],
      ["a/b"],
      [""],
      ["x".repeat(40)],
      Array.from({ length: 51 }, (_, n) => `o${n}`),
    ]) {
      expect(await put({ owners })).toMatchObject({ status: 400, body: { code: "invalid_request" } });
    }
    expect(await put({})).toMatchObject({ status: 400, body: { code: "invalid_request" } });
  });
});
