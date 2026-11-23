import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";

startMockServer();
const PR = "/pr/acme/node-client/482";

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
  window.localStorage.clear();
});
afterEach(() => vi.unstubAllGlobals());

// Plan §3.2 item 9: "Skip for now moves on the same way" as Decide: the entry after the skipped one in the list.
describe("V1 Questions: Skip for now goes to the next entry (plan §3.2 item 9)", () => {
  it("Skip on a question picked from the middle of the list opens the next one, not the first", async () => {
    renderRoute(`${PR}/comments`);
    const queue = await screen.findByRole("navigation", { name: "Queue" });
    await userEvent.click(await within(queue).findByRole("button", { name: /^C15(?!\d)/ }));
    expect(await screen.findByRole("heading", { name: /C15 · Debug log prints/ })).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: /Skip for now/ }));
    expect(
      await screen.findByRole("heading", { name: /C11 · Backoff cap is not configurable/ }),
    ).toBeVisible();
    // The skipped question is last of Needs you, before the PR-level section.
    await waitFor(() => {
      const rows = within(queue)
        .getAllByRole("button")
        .map((b) => b.textContent ?? "")
        .filter((t) => /^(C|G|P)\d*/.test(t));
      const c15 = rows.findIndex((t) => t.startsWith("C15"));
      const p24 = rows.findIndex((t) => t.startsWith("P24"));
      expect(c15).toBeGreaterThan(0);
      expect(p24).toBeGreaterThan(c15);
    });
  });
});
