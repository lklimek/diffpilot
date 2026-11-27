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

describe("Skipping a question", () => {
  it("moves it to Skipped, lowers Needs you and opens the next active question", async () => {
    renderRoute(`${PR}/comments`);
    const queue = await screen.findByRole("navigation", { name: "Queue" });
    await userEvent.click(await within(queue).findByRole("button", { name: /^C15(?!\d)/ }));
    expect(await screen.findByRole("heading", { name: /C15 · Debug log prints/ })).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: /Skip for now/ }));
    expect(
      await screen.findByRole("heading", { name: /C11 · Backoff cap is not configurable/ }),
    ).toBeVisible();
    await waitFor(() =>
      expect(within(queue).getByRole("button", { name: /^Status: Needs you · 10/ })).toBeVisible(),
    );
    const skipped = within(queue).getByRole("group", { name: "Skipped" });
    await userEvent.click(within(skipped).getByText("Skipped (1)"));
    await userEvent.click(within(skipped).getByRole("button", { name: /^C15(?!\d)/ }));
    expect(await screen.findByRole("heading", { name: /C15 · Debug log prints/ })).toBeVisible();
    expect(screen.queryByRole("button", { name: /Skip for now/ })).toBeNull();
    expect(screen.getByRole("button", { name: /^Decide$/ })).toBeVisible();
  });
});
